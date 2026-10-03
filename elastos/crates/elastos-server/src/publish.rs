use anyhow::Context;
use elastos_common::localhost::{publisher_publish_state_path, publisher_root_path};
use elastos_common::{CapsuleManifest, RequirementKind};
use sha2::Digest;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const HOME_PUBLISH_CAPSULES: &[&str] = &[
    "shell",
    "localhost-provider",
    "did-provider",
    "chain-provider",
    "net-provider",
    "exit-provider",
    "browser-engine-adapter",
    "webspace-provider",
    "wallet-provider",
    "object-provider",
    "content-block-graph-provider",
    "ipfs-provider",
    "home-cli",
    "home-gui",
    "home",
    "system",
    "services",
    "people",
    "wallet-metamask",
    "wallet-unisat",
    "wallet-walletconnect",
    "wallet",
    "browser",
    "documents",
    "library",
    "marketplace",
    "archive-manager",
    "inbox",
    "assistant",
    "elacity-player",
    "model-provider",
];
const DEFAULT_PUBLISH_CAPSULES: &[&str] = HOME_PUBLISH_CAPSULES;
const DEMO_PUBLISH_CAPSULES: &[&str] =
    &["gba-emulator", "gba-ucity", "chat-room", "tunnel-provider"];
const RETIRED_PRODUCT_CAPSULES: &[&str] = &["agent", "chat", "home-agent"];
const REQUIRED_SUPPORTED_PUBLISH_CAPSULES: &[&str] = &[
    "shell",
    "localhost-provider",
    "did-provider",
    "chain-provider",
    "net-provider",
    "exit-provider",
    "browser-engine-adapter",
    "webspace-provider",
    "wallet-provider",
    "object-provider",
    "content-block-graph-provider",
    "ipfs-provider",
    "home-cli",
    "home-gui",
    "home",
    "system",
    "services",
    "people",
    "wallet-metamask",
    "wallet-unisat",
    "wallet-walletconnect",
    "wallet",
    "browser",
    "documents",
    "library",
    "marketplace",
    "archive-manager",
    "inbox",
    "assistant",
    "elacity-player",
    "model-provider",
];
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
    pub(crate) platform_inputs: Vec<String>,
    pub(crate) preview_platform: Option<String>,
    pub(crate) prepare_only: Option<PathBuf>,
    pub(crate) signed_publication: Option<PathBuf>,
    pub(crate) publisher_did: Option<String>,
    pub(crate) allow_signer_rotation: bool,
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
    publisher_did: Option<String>,
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
    #[serde(default)]
    release_object_cid: Option<String>,
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
    #[serde(default)]
    release_object_cid: Option<String>,
}

pub(crate) async fn run_publish_release(mut options: PublishReleaseOptions) -> anyhow::Result<()> {
    let workspace_root = workspace_root();
    validate_prepare_options(&options)?;
    validate_platform_input_options(&options)?;
    if options.signed_publication.is_some() {
        return run_signed_publication(options, &workspace_root).await;
    }
    if options.prepare_only.is_none() && !options.dry_run && !options.preflight_only {
        anyhow::bail!("Publish the separately signed set with --signed-publication and --publisher-did, or prepare inert inputs with --prepare-only");
    }
    if !options.platform_inputs.is_empty() {
        let caller_dir =
            std::env::current_dir().context("Failed to resolve the caller directory")?;
        resolve_platform_input_paths(&mut options.platform_inputs, &caller_dir)?;
        if let Some(output) = &mut options.prepare_only {
            *output = caller_dir.join(&*output);
        }
    }
    let (available_capsules, selected_capsules) = if options.platform_inputs.is_empty() {
        let manifests = load_capsule_manifests(&workspace_root)?;
        let available = manifests.keys().cloned().collect::<Vec<_>>();
        let selected = select_capsules(&options.profile, &options.capsules, &manifests)?;
        (available, selected)
    } else {
        // The admitted inputs own the complete native Home component inventory.
        (Vec::new(), Vec::new())
    };
    validate_publish_inputs(&options, &workspace_root, &selected_capsules)?;
    let data_dir = elastos_server::sources::default_data_dir();
    let state_path = publish_state_path(&data_dir);
    let previous_state = load_publish_state(&state_path)?;

    if options.dry_run {
        print_publish_plan(
            &options,
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

    if let Some(output) = &options.prepare_only {
        if output.exists() || output.symlink_metadata().is_ok() {
            anyhow::bail!("Unsigned signing-input output already exists");
        }
        let mut command = Command::new("bash");
        command
            .arg(workspace_root.join("scripts/publish-release.sh"))
            .arg("--version")
            .arg(&options.version)
            .arg("--channel")
            .arg(&options.channel)
            .arg("--prepare-only")
            .arg(output)
            .arg("--publisher-did")
            .arg(
                options
                    .publisher_did
                    .as_deref()
                    .context("Public signer DID required")?,
            )
            .env("ELASTOS_PUBLISH_STATE_DIR", publish_state_dir(&data_dir))
            .current_dir(&workspace_root);
        append_publish_selection_args(&mut command, &options, &selected_capsules);
        if let Some(provider) = &options.ipfs_provider_bin {
            command.arg("--ipfs-provider-bin").arg(provider);
        }
        let status = command
            .status()
            .context("Failed to prepare unsigned signing input")?;
        if !status.success() {
            anyhow::bail!(
                "Unsigned signing-input preparation exited with status {}",
                status
            );
        }
        return Ok(());
    }

    anyhow::bail!("The custodian tool owns release signing; publish its frozen output")
}

async fn run_signed_publication(
    options: PublishReleaseOptions,
    workspace_root: &Path,
) -> anyhow::Result<()> {
    use elastos_server::release_publication::Publication;

    validate_release_channel(&options.channel)?;
    let caller_dir = std::env::current_dir()?;
    let input = caller_dir.join(
        options
            .signed_publication
            .as_ref()
            .context("Signed publication required")?,
    );
    let signer = options
        .publisher_did
        .as_deref()
        .context("Public signer DID required")?;
    let publication = Publication::open_flat(&input, signer)?;
    anyhow::ensure!(
        publication.version() == options.version && publication.channel() == options.channel,
        "The signed set differs from the requested version or channel"
    );
    let data_dir = elastos_server::sources::default_data_dir();
    let state_path = publish_state_path(&data_dir);
    let previous = load_publish_state(&state_path)?;
    let needs_confirmation =
        publication_pin_change(&previous, signer, options.allow_signer_rotation)?;
    validate_publication_chain(
        &previous,
        publication.head_bytes(),
        publication.release_bytes(),
        publication.release_cid(),
    )?;
    if options.dry_run {
        println!(
            "Frozen signed publication {} on {} as {}",
            options.version, options.channel, signer
        );
        println!(
            "{} admitted artifacts; CID import and publication are pending",
            publication.artifacts().len()
        );
        if needs_confirmation {
            println!("The complete public DID must be confirmed before changing the saved pin");
        }
        return Ok(());
    }
    let provider = options
        .ipfs_provider_bin
        .clone()
        .or_else(|| find_ipfs_provider_binary(workspace_root))
        .context("A qualified ipfs-provider binary is required; pass --ipfs-provider-bin")?;
    anyhow::ensure!(
        is_executable_file(&provider),
        "ipfs-provider must be an executable file"
    );
    let script = workspace_root.join("scripts/publish-release.sh");
    anyhow::ensure!(script.is_file(), "The publication helper is required");
    if options.preflight_only {
        println!("Frozen signed publication admission and provider preflight passed");
        return Ok(());
    }
    if needs_confirmation {
        use std::io::Write;
        print!("Change the saved release signer to {signer}. Type this complete DID to confirm, or press Enter to cancel: ");
        std::io::stdout().flush()?;
        confirm_publication_pin(signer, &mut std::io::stdin().lock())?;
    }

    // Candidate files are read only through held descriptors. The separate
    // publication host executes its provider against this verified snapshot.
    std::fs::create_dir_all(&data_dir)?;
    let data_dir = data_dir.canonicalize()?;
    let budget = publication_storage_budget(&publication)?;
    let mut preflight = PublicationProvider::start(&provider, &data_dir).await?;
    let imported: anyhow::Result<_> = async {
        preflight
            .request(serde_json::json!({"op":"runtime_prepare_backend"}))
            .await?;
        preflight
            .check_capacity(&data_dir, budget.repo_bytes, budget.local_bytes)
            .await?;
        let scratch = tempfile::Builder::new()
            .prefix(".release-import-")
            .tempdir_in(&data_dir)?;
        let snapshot = scratch.path().join("snapshot");
        publication.snapshot_into(&snapshot)?;
        let frozen = Publication::open_published(&snapshot, signer)?;
        for artifact in frozen.artifacts() {
            let actual = preflight
                .import_file(&snapshot.join("artifacts").join(&artifact.name))
                .await?;
            if actual != artifact.cid && cid::Cid::try_from(artifact.cid.as_str())?.codec() == 0x55
            {
                // The catalogue's signed pin is a raw block. The existing provider
                // imports files as UnixFS; publish the bounded raw block through
                // that provider's own local Kubo endpoint as well.
                anyhow::ensure!(
                    artifact.size <= 2 * 1024 * 1024,
                    "Raw publication block exceeds its bounded import size"
                );
                let bytes = frozen.read_verified_artifact(&artifact.name)?;
                let raw = preflight.import_raw_block(&artifact.name, bytes).await?;
                require_import_cid(&raw, &artifact.cid)?;
            } else {
                require_import_cid(&actual, &artifact.cid)?;
            }
        }
        // Directory blocks, rather than just CAR file bytes, must be retained
        // before this generation can become the advertised release head.
        import_publication_models_with_session(
            &mut preflight,
            workspace_root,
            &data_dir,
            &snapshot.join("artifacts"),
            frozen.model_retention(),
        )
        .await?;
        let release_cid = preflight
            .import_file(&snapshot.join("release.json"))
            .await?;
        require_import_cid(&release_cid, publication.release_cid())?;
        let head_cid = preflight
            .import_file(&snapshot.join("release-head.json"))
            .await?;
        elastos_server::update::verify_release_metadata_cid(&head_cid, publication.head_bytes())?;
        Ok((scratch, snapshot, release_cid, head_cid))
    }
    .await;
    let finished = preflight.finish().await;
    if finished.is_err() {
        let _ = preflight.child.kill().await;
    }
    let (scratch, snapshot, release_cid, head_cid) = imported?;
    finished?;
    let state = PublishState {
        publisher_did: Some(signer.to_owned()),
        last_release_cid: Some(release_cid.clone()),
        last_head_cid: Some(head_cid.clone()),
        last_version: Some(options.version.clone()),
        last_published_at: Some(now_unix()?),
    };
    let state_file = scratch.path().join("publish-state.json");
    std::fs::write(&state_file, serde_json::to_vec_pretty(&state)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&state_file, std::fs::Permissions::from_mode(0o600))?;
    }
    let checked = Publication::open_published(&snapshot, signer)?;
    anyhow::ensure!(
        checked.head_bytes() == publication.head_bytes()
            && checked.release_bytes() == publication.release_bytes(),
        "Frozen metadata changed during import"
    );
    let status = Command::new("bash")
        .arg("-c")
        .arg("source \"$1\"; export_release_publication \"$2\" \"$3/release-head.json\" \"$3/release.json\" \"$3/install.sh\" \"$3/artifacts\" \"$4\"")
        .arg("publish-signed-set")
        .arg(&script)
        .arg(publish_state_dir(&data_dir))
        .arg(&snapshot)
        .arg(&state_file)
        .current_dir(workspace_root)
        .status().context("Failed to commit the frozen publication")?;
    anyhow::ensure!(
        status.success(),
        "Frozen publication promotion failed; inspect the helper's recovery result"
    );

    // The public pin and CID receipt are part of the head-last publication
    // transaction. Ledger and gossip are derived, retryable work after commit.
    let entry = build_release_ledger_entry(&snapshot, &release_cid, &head_cid, &[])?;
    let ledger_path = release_ledger_path(&data_dir);
    let mut ledger = load_release_ledger(&ledger_path)?;
    let earlier = ledger
        .entries
        .iter()
        .rev()
        .find(|item| item.channel == entry.channel && item.head_cid != entry.head_cid)
        .cloned();
    ledger.upsert(entry.clone());
    save_release_ledger(&ledger_path, &ledger)
        .context("The signed set is committed; retry publication to record its ledger")?;
    print_release_diff_summary(&entry, earlier.as_ref(), &ledger_path);
    match announce_release_head(&entry).await {
        Ok(topics) => println!("Release head announced on {}", topics.join(", ")),
        Err(error) => anyhow::bail!(
            "The signed set is committed; retry publication to announce its head: {error}"
        ),
    }
    Ok(())
}

fn publication_pin_change(
    state: &PublishState,
    candidate: &str,
    allow: bool,
) -> anyhow::Result<bool> {
    validate_public_signer_did(candidate)?;
    let changing = match state.publisher_did.as_deref() {
        Some(previous) => previous != candidate,
        None => state.last_release_cid.is_some() || state.last_head_cid.is_some(),
    };
    anyhow::ensure!(!changing || allow, "Saved public signer pin differs or legacy publication lacks a pin; use --allow-signer-rotation for an approved change");
    Ok(changing)
}

fn confirm_publication_pin(signer: &str, input: &mut impl std::io::BufRead) -> anyhow::Result<()> {
    let mut answer = String::new();
    let mut bounded = std::io::Read::take(input, 512);
    std::io::BufRead::read_line(&mut bounded, &mut answer)?;
    anyhow::ensure!(
        answer.trim_end_matches(['\r', '\n']) == signer,
        "Public signer change cancelled; the saved pin and publication stay intact"
    );
    Ok(())
}

fn validate_publication_chain(
    state: &PublishState,
    head_bytes: &[u8],
    release_bytes: &[u8],
    release_cid: &str,
) -> anyhow::Result<()> {
    let head: serde_json::Value = serde_json::from_slice(head_bytes)?;
    let release: serde_json::Value = serde_json::from_slice(release_bytes)?;
    // Repeating the exact committed release is allowed so the operator can
    // finish derived ledger/gossip work after a transport failure.
    if state.last_release_cid.as_deref() == Some(release_cid) {
        let saved_head = state
            .last_head_cid
            .as_deref()
            .context("Committed release lacks its head CID receipt")?;
        elastos_server::update::verify_release_metadata_cid(saved_head, head_bytes)?;
        return Ok(());
    }
    anyhow::ensure!(
        head["payload"]["prev_head_cid"].as_str() == state.last_head_cid.as_deref(),
        "Signed head does not continue the saved publication receipt"
    );
    anyhow::ensure!(
        release["payload"]["prev_release_cid"].as_str() == state.last_release_cid.as_deref(),
        "Signed release does not continue the saved publication receipt"
    );
    Ok(())
}

fn require_import_cid(actual: &str, expected: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        actual == expected,
        "Imported CID differs from its signed reference"
    );
    Ok(())
}

#[cfg(test)]
async fn import_publication_file(provider: &Path, file: &Path) -> anyhow::Result<String> {
    let response = publication_provider_request(
        provider,
        serde_json::json!({"op":"add_path", "path":file, "pin":true}),
    )
    .await?;
    let cid = response["data"]["cid"]
        .as_str()
        .context("IPFS import receipt lacks a CID")?;
    let _ = cid::Cid::try_from(cid).context("IPFS import receipt has an invalid CID")?;
    Ok(cid.to_owned())
}

#[cfg(test)]
async fn import_publication_raw_block(
    provider: &Path,
    name: &str,
    bytes: Vec<u8>,
) -> anyhow::Result<String> {
    anyhow::ensure!(
        bytes.len() <= 2 * 1024 * 1024,
        "Raw publication block exceeds its bounded import size"
    );
    let status = publication_provider_request(provider, serde_json::json!({"op":"status"})).await?;
    publication_raw_block_put(status, name, bytes).await
}

async fn publication_raw_block_put(
    status: serde_json::Value,
    name: &str,
    bytes: Vec<u8>,
) -> anyhow::Result<String> {
    anyhow::ensure!(
        bytes.len() <= 2 * 1024 * 1024,
        "Raw publication block exceeds its bounded import size"
    );
    let endpoint = status["data"]["api_endpoint"]
        .as_str()
        .context("Qualified Kubo API endpoint missing")?;
    let base = elastos_server::local_http::LoopbackHttpBaseUrl::parse(endpoint)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let part = reqwest::multipart::Part::bytes(bytes).file_name(name.to_owned());
    let mut response = client
        .post(base.join("/api/v0/block/put")?)
        .query(&[
            ("cid-codec", "raw"),
            ("mhtype", "sha2-256"),
            ("pin", "true"),
        ])
        .multipart(reqwest::multipart::Form::new().part("file", part))
        .send()
        .await?
        .error_for_status()?;
    let mut receipt_bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            receipt_bytes.len() + chunk.len() <= 64 * 1024,
            "Raw block import receipt exceeds its bound"
        );
        receipt_bytes.extend_from_slice(&chunk);
    }
    let receipt: serde_json::Value = serde_json::from_slice(&receipt_bytes)?;
    Ok(receipt["Key"]
        .as_str()
        .context("Raw block import receipt lacks its CID")?
        .to_owned())
}

#[cfg(test)]
async fn publication_provider_request(
    provider: &Path,
    operation: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new(provider)
        .env(
            "ELASTOS_DATA_DIR",
            elastos_server::sources::default_data_dir(),
        )
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("Cannot start the qualified IPFS import provider")?;
    let request = format!("{{\"op\":\"init\",\"config\":{{}}}}\n{operation}\n");
    let mut input = child.stdin.take().context("Provider stdin missing")?;
    input.write_all(request.as_bytes()).await?;
    input.shutdown().await?;
    drop(input);
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        child.wait_with_output(),
    )
    .await
    .context("IPFS import exceeded its five-minute limit")??;
    anyhow::ensure!(output.status.success(), "IPFS provider import failed");
    let last = output
        .stdout
        .split(|byte| *byte == b'\n')
        .rev()
        .find(|line| !line.is_empty())
        .context("IPFS provider returned no import receipt")?;
    let response: serde_json::Value = serde_json::from_slice(last)?;
    anyhow::ensure!(
        response["status"].as_str() == Some("ok"),
        "IPFS provider refused import"
    );
    Ok(response)
}

struct PublicationProvider {
    child: tokio::process::Child,
    input: tokio::process::ChildStdin,
    output: tokio::io::BufReader<tokio::process::ChildStdout>,
}

impl PublicationProvider {
    async fn start(provider: &Path, data_dir: &Path) -> anyhow::Result<Self> {
        let mut child = tokio::process::Command::new(provider)
            .env("ELASTOS_DATA_DIR", data_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("Cannot start the qualified model import provider")?;
        let input = child.stdin.take().context("Model provider stdin missing")?;
        let output = tokio::io::BufReader::new(
            child
                .stdout
                .take()
                .context("Model provider stdout missing")?,
        );
        let mut session = Self {
            child,
            input,
            output,
        };
        if let Err(error) = session
            .request(serde_json::json!({"op":"init","config":{}}))
            .await
        {
            let _ = session.child.kill().await;
            return Err(error);
        }
        Ok(session)
    }

    async fn request(&mut self, request: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
        let operation = async {
            let line = format!("{request}\n");
            self.input.write_all(line.as_bytes()).await?;
            let mut response = Vec::new();
            (&mut self.output)
                .take(64 * 1024 + 1)
                .read_until(b'\n', &mut response)
                .await?;
            anyhow::ensure!(
                !response.is_empty() && response.len() <= 64 * 1024 && response.ends_with(b"\n"),
                "Model provider response exceeds its bound or ended early"
            );
            let value: serde_json::Value = serde_json::from_slice(&response)?;
            anyhow::ensure!(
                value["status"] == "ok",
                "Qualified model provider refused readiness"
            );
            Ok(value)
        };
        tokio::time::timeout(std::time::Duration::from_secs(300), operation)
            .await
            .context("Model provider readiness exceeded five minutes")?
    }

    async fn import_file(&mut self, file: &Path) -> anyhow::Result<String> {
        let response = self
            .request(serde_json::json!({"op":"add_path", "path":file, "pin":true}))
            .await?;
        let cid = response["data"]["cid"]
            .as_str()
            .context("IPFS import receipt lacks a CID")?;
        cid::Cid::try_from(cid).context("IPFS import receipt has an invalid CID")?;
        Ok(cid.to_owned())
    }

    async fn import_raw_block(&mut self, name: &str, bytes: Vec<u8>) -> anyhow::Result<String> {
        let status = self.request(serde_json::json!({"op":"status"})).await?;
        publication_raw_block_put(status, name, bytes).await
    }

    async fn check_capacity(
        &mut self,
        data_dir: &Path,
        repo_bytes: u64,
        local_bytes: u64,
    ) -> anyhow::Result<(u64, elastos_server::local_http::LoopbackHttpBaseUrl)> {
        // The private provider probe validates the actual API repository, its
        // pinned datastore layout and all datastore volumes. Bind that probe
        // to the selected Runtime repository before using its free-space data.
        let status = self.request(serde_json::json!({"op":"status"})).await?;
        let endpoint = status["data"]["api_endpoint"]
            .as_str()
            .context("Model repository API endpoint missing")?;
        let base = elastos_server::local_http::LoopbackHttpBaseUrl::parse(endpoint)?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let mut response = client
            .post(base.join("/api/v0/repo/stat")?)
            .send()
            .await?
            .error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            anyhow::ensure!(
                bytes.len() + chunk.len() <= 64 * 1024,
                "Model repository status exceeds its bound"
            );
            bytes.extend_from_slice(&chunk);
        }
        let repo_status: serde_json::Value = serde_json::from_slice(&bytes)?;
        let repo = data_dir.join("ipfs-repo");
        require_selected_repository(&repo_status, &repo)?;
        // The existing private probe admits up to 64 GiB per request. Larger
        // publication reservations use the same observed volume and capacity;
        // the publisher checks their entire budget below without truncation.
        let probe_bytes = repo_bytes.clamp(1, 64 * 1024 * 1024 * 1024);
        let observation = self
            .request(
                serde_json::json!({"op":"runtime_check_capacity","required_bytes":probe_bytes}),
            )
            .await?;
        let local = storage_observation(data_dir)?;
        let selected = storage_observation(&repo)?;
        let floor = validate_publication_capacity(
            &observation["data"],
            probe_bytes,
            selected,
            local,
            repo_bytes,
            local_bytes,
        )?;
        Ok((floor, base))
    }

    async fn finish(&mut self) -> anyhow::Result<()> {
        use tokio::io::AsyncWriteExt;
        self.request(serde_json::json!({"op":"shutdown"})).await?;
        self.input.shutdown().await?;
        let status = tokio::time::timeout(std::time::Duration::from_secs(5), self.child.wait())
            .await
            .context("Model import provider did not exit")??;
        anyhow::ensure!(status.success(), "Model import provider exit failed");
        Ok(())
    }
}

struct PublicationStorageBudget {
    local_bytes: u64,
    repo_bytes: u64,
}

fn model_storage_bytes(
    records: &[elastos_server::release_publication::ModelRetention],
) -> anyhow::Result<u64> {
    records.iter().try_fold(0_u64, |total, record| {
        total
            .checked_add(
                record
                    .car
                    .size
                    .checked_mul(2)
                    .context("Model storage budget overflow")?,
            )
            .and_then(|n| n.checked_add(1024 * 1024))
            .context("Model storage budget overflow")
    })
}

fn publication_storage_budget(
    publication: &elastos_server::release_publication::Publication,
) -> anyhow::Result<PublicationStorageBudget> {
    let metadata = (publication.head_bytes().len()
        + publication.release_bytes().len()
        + publication.installer_bytes().len()) as u64;
    let files = publication
        .artifacts()
        .iter()
        .try_fold(metadata, |total, artifact| {
            total
                .checked_add(artifact.size)
                .context("Publication storage budget overflow")
        })?;
    storage_budget(
        files,
        publication.artifacts().len() + 3,
        model_storage_bytes(publication.model_retention())?,
    )
}

fn storage_budget(
    files: u64,
    count: usize,
    models: u64,
) -> anyhow::Result<PublicationStorageBudget> {
    // Two full local copies (immutable snapshot and promotion stage), two
    // payload budgets for UnixFS file blocks and filesystem overhead, then the
    // retained model directory blocks. One MiB per file covers control files,
    // allocation rounding and publication bookkeeping. Existing pins may make
    // the real write smaller; preflight reserves a cold repository.
    let overhead = u64::try_from(count)?
        .checked_mul(1024 * 1024)
        .context("Publication storage budget overflow")?;
    let twice = files
        .checked_mul(2)
        .context("Publication storage budget overflow")?;
    Ok(PublicationStorageBudget {
        local_bytes: twice
            .checked_add(overhead)
            .context("Publication storage budget overflow")?,
        repo_bytes: twice
            .checked_add(overhead)
            .and_then(|n| n.checked_add(models))
            .context("Publication storage budget overflow")?,
    })
}

#[derive(Clone, Copy)]
struct StorageObservation {
    volume: u64,
    capacity: u64,
    free: u64,
}

fn storage_observation(path: &Path) -> anyhow::Result<StorageObservation> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let metadata = directory.metadata()?;
    anyhow::ensure!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o022 == 0,
        "Publication storage directory must be owned and protected"
    );
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    anyhow::ensure!(
        unsafe { libc::fstatvfs(directory.as_raw_fd(), stats.as_mut_ptr()) } == 0,
        "Publication storage observation failed"
    );
    let stats = unsafe { stats.assume_init() };
    let capacity = u64::try_from(u128::from(stats.f_blocks) * u128::from(stats.f_frsize))?;
    let free = u64::try_from(u128::from(stats.f_bavail) * u128::from(stats.f_frsize))?;
    anyhow::ensure!(
        capacity > 0
            && free <= capacity
            && stats.f_frsize > 0
            && stats.f_flag & libc::ST_RDONLY == 0,
        "Publication storage observation refused"
    );
    Ok(StorageObservation {
        volume: metadata.dev(),
        capacity,
        free,
    })
}

fn require_selected_repository(value: &serde_json::Value, repo: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        value["RepoPath"].as_str() == repo.to_str() && value["RepoSize"].as_u64().is_some(),
        "Model API repository differs from the selected Runtime repository"
    );
    Ok(())
}

fn validate_publication_capacity(
    value: &serde_json::Value,
    probe_bytes: u64,
    repo: StorageObservation,
    local: StorageObservation,
    repo_bytes: u64,
    local_bytes: u64,
) -> anyhow::Result<u64> {
    let fields = value
        .as_object()
        .context("Publication capacity response required")?;
    anyhow::ensure!(
        fields.len() == 4
            && fields.keys().all(|key| matches!(
                key.as_str(),
                "volume_id" | "capacity_bytes" | "available_bytes" | "required_bytes"
            )),
        "Publication capacity fields refused"
    );
    let free = value["available_bytes"]
        .as_u64()
        .context("Publication capacity free bytes required")?;
    anyhow::ensure!(
        repo.volume > 0
            && value["volume_id"].as_u64() == Some(repo.volume)
            && value["capacity_bytes"].as_u64() == Some(repo.capacity)
            && value["required_bytes"].as_u64() == Some(probe_bytes)
            && free <= repo.capacity,
        "Publication capacity differs from the selected Runtime repository"
    );
    let repo_free = free.min(repo.free);
    let floor = u64::try_from(u128::from(repo.capacity).saturating_mul(15).div_ceil(100))?;
    let shared = repo.volume == local.volume;
    anyhow::ensure!(
        !shared || repo.capacity == local.capacity,
        "Publication volume observations disagree"
    );
    let required = u128::from(repo_bytes) + if shared { u128::from(local_bytes) } else { 0 };
    anyhow::ensure!(
        u128::from(repo_free.min(if shared { local.free } else { repo_free }))
            >= u128::from(floor) + required,
        "Publication writes would cross the 15 percent free-space floor"
    );
    if !shared {
        let local_floor = u128::from(local.capacity).saturating_mul(15).div_ceil(100);
        anyhow::ensure!(
            local.capacity > 0
                && local.free <= local.capacity
                && u128::from(local.free) >= local_floor + u128::from(local_bytes),
            "Publication local copies would cross the 15 percent free-space floor"
        );
    }
    Ok(floor)
}

fn model_import_space_guard(
    repo: &Path,
    records: &[elastos_server::release_publication::ModelRetention],
) -> anyhow::Result<u64> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(repo)?;
    let metadata = directory.metadata()?;
    anyhow::ensure!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o022 == 0,
        "Model repository must be owned and protected"
    );
    let mut observation = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    anyhow::ensure!(
        unsafe { libc::fstatvfs(directory.as_raw_fd(), observation.as_mut_ptr()) } == 0,
        "Model repository capacity observation failed"
    );
    let observation = unsafe { observation.assume_init() };
    let capacity = u128::from(observation.f_blocks) * u128::from(observation.f_frsize);
    let free = u128::from(observation.f_bavail) * u128::from(observation.f_frsize);
    let floor = capacity.saturating_mul(15).div_ceil(100);
    let car_bytes: u128 = records
        .iter()
        .map(|record| u128::from(record.car.size))
        .sum();
    // Reserve all CAR payload bytes plus an equal filesystem-overhead budget.
    // Observe real free space again after each import as well.
    let required = car_bytes * 2 + records.len() as u128 * 1024 * 1024;
    anyhow::ensure!(
        capacity > 0 && free <= capacity && free >= floor + required,
        "Model directory import would cross the 15 percent free-space floor"
    );
    Ok(u64::try_from(floor).context("Model repository floor exceeds its integer bound")?)
}

async fn bounded_model_import(
    script: &Path,
    api: &elastos_server::local_http::LoopbackHttpBaseUrl,
    artifacts: &Path,
    record: &elastos_server::release_publication::ModelRetention,
    floor: u64,
) -> anyhow::Result<()> {
    use tokio::io::{AsyncRead, AsyncReadExt};
    async fn read_bounded(reader: impl AsyncRead + Unpin) -> anyhow::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        reader.take(64 * 1024 + 1).read_to_end(&mut bytes).await?;
        anyhow::ensure!(
            bytes.len() <= 64 * 1024,
            "Model import process output exceeded its bound"
        );
        Ok(bytes)
    }
    let mut child = tokio::process::Command::new("python3")
        .arg(script)
        .arg("import")
        .arg("--kubo-api")
        .arg(api.as_str())
        .arg("--cid")
        .arg(&record.package_cid)
        .arg("--car")
        .arg(artifacts.join(&record.car.name))
        .arg("--receipt")
        .arg(artifacts.join(&record.receipt.name))
        .arg("--free-space-floor-bytes")
        .arg(floor.to_string())
        .arg("--timeout-seconds")
        .arg("60")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("Cannot run the model package import helper")?;
    let stdout = child.stdout.take().context("Model import stdout missing")?;
    let stderr = child.stderr.take().context("Model import stderr missing")?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(300), async {
        let (stdout, _stderr, status) =
            tokio::try_join!(read_bounded(stdout), read_bounded(stderr), async {
                Ok::<_, anyhow::Error>(child.wait().await?)
            })?;
        anyhow::ensure!(
            status.success(),
            "Model CAR import failed before publication"
        );
        let value: serde_json::Value =
            serde_json::from_slice(&stdout).context("Model import receipt is invalid")?;
        let fields = value
            .as_object()
            .context("Model import receipt must be an object")?;
        anyhow::ensure!(
            fields.len() == 6
                && fields.keys().all(|key| matches!(
                    key.as_str(),
                    "schema" | "package_cid" | "pinned" | "blocks" | "block_bytes" | "kubo_version"
                )),
            "Model import receipt fields refused"
        );
        anyhow::ensure!(
            value["schema"] == "elastos.model.package-import/v1"
                && value["package_cid"] == record.package_cid
                && value["pinned"] == true
                && value["kubo_version"] == "0.40.1"
                && value["blocks"]
                    .as_u64()
                    .is_some_and(|count| count > 0 && count <= record.car.size)
                && value["block_bytes"]
                    .as_u64()
                    .is_some_and(|size| size > 0 && size <= record.car.size),
            "Model import receipt differs from the signed retention root"
        );
        Ok(())
    })
    .await;
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            let _ = child.kill().await;
            Err(error)
        }
        Err(_) => {
            let _ = child.kill().await;
            anyhow::bail!("Model CAR import exceeded five minutes before publication")
        }
    }
}

#[cfg(test)]
async fn import_publication_models(
    provider: &Path,
    workspace: &Path,
    data_dir: &Path,
    artifacts: &Path,
    records: &[elastos_server::release_publication::ModelRetention],
) -> anyhow::Result<()> {
    let mut session = PublicationProvider::start(provider, data_dir).await?;
    let result = import_publication_models_with_session(
        &mut session,
        workspace,
        data_dir,
        artifacts,
        records,
    )
    .await;
    let finished = session.finish().await;
    if finished.is_err() {
        let _ = session.child.kill().await;
    }
    result?;
    finished
}

async fn import_publication_models_with_session(
    session: &mut PublicationProvider,
    workspace: &Path,
    data_dir: &Path,
    artifacts: &Path,
    records: &[elastos_server::release_publication::ModelRetention],
) -> anyhow::Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let script = workspace.join("scripts/model-package-handoff.py");
    anyhow::ensure!(script.is_file(), "Model package import helper is required");
    async {
        session
            .request(serde_json::json!({"op":"runtime_prepare_backend"}))
            .await?;
        let repo = data_dir.join("ipfs-repo");
        let required = model_storage_bytes(records)?;
        let (floor, api) = session.check_capacity(data_dir, required, 0).await?;
        model_import_space_guard(&repo, records)?;
        for record in records {
            session
                .request(serde_json::json!({"op":"runtime_prepare_backend"}))
                .await?;
            // Use the held provider's admitted endpoint, rather than resolving
            // the mutable coordinate file again in the external helper.
            bounded_model_import(&script, &api, artifacts, record, floor).await?;
            let (_, after) = session.check_capacity(data_dir, 1, 0).await?;
            anyhow::ensure!(
                after.as_str() == api.as_str(),
                "Model provider API changed during CAR import"
            );
            model_import_space_guard(&repo, &[])?;
        }
        Ok(())
    }
    .await
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

fn load_publish_state(path: &Path) -> anyhow::Result<PublishState> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PublishState::default())
        }
        Err(error) => return Err(error).context("Cannot read the saved public publication pin"),
    };
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file(),
        "Publication state must be a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            metadata.uid() == unsafe { libc::geteuid() }
                && metadata.nlink() == 1
                && metadata.mode() & 0o022 == 0,
            "Publication state must be owner-controlled with one link"
        );
    }
    let mut data = Vec::new();
    file.take(64 * 1024 + 1).read_to_end(&mut data)?;
    anyhow::ensure!(data.len() <= 64 * 1024, "Publication state is too large");
    Ok(serde_json::from_slice(&data)?)
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

#[cfg(test)]
fn save_publish_state(path: &Path, state: &PublishState) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(state)?)?;
    Ok(())
}

fn validate_publishable_manifest(path: &Path, manifest: &CapsuleManifest) -> anyhow::Result<()> {
    manifest
        .validate()
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("Invalid capsule manifest {}", path.display()))?;

    if manifest
        .description
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
    {
        anyhow::bail!(
            "Capsule manifest {} must declare a description before publishing",
            path.display()
        );
    }
    let author = manifest
        .author
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Capsule manifest {} must declare an author before publishing",
                path.display()
            )
        })?;
    if matches!(author, "local-development" | "example-publisher") {
        anyhow::bail!(
            "Capsule manifest {} still uses placeholder author '{}'",
            path.display(),
            author
        );
    }

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
            validate_publishable_manifest(&capsule_json, &manifest)?;
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
        "home" => DEFAULT_PUBLISH_CAPSULES
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>(),
        "demo" => HOME_PUBLISH_CAPSULES
            .iter()
            .chain(DEMO_PUBLISH_CAPSULES.iter())
            .map(|name| name.to_string())
            .collect::<Vec<_>>(),
        "providers" => vec![
            "availability-provider".to_string(),
            "chain-provider".to_string(),
            "content-block-graph-provider".to_string(),
            "decrypt-provider".to_string(),
            "did-provider".to_string(),
            "drm-provider".to_string(),
            "exit-provider".to_string(),
            "ipfs-provider".to_string(),
            "key-provider".to_string(),
            "net-provider".to_string(),
            "object-provider".to_string(),
            "localhost-provider".to_string(),
            "rights-provider".to_string(),
            "tunnel-provider".to_string(),
            "wallet-provider".to_string(),
            "webspace-provider".to_string(),
        ],
        "full" => available
            .iter()
            .filter(|name| !RETIRED_PRODUCT_CAPSULES.contains(&name.as_str()))
            .cloned()
            .collect(),
        other => {
            anyhow::bail!(
                "Unknown publish profile '{}'. Available profiles: home, demo, providers, full",
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
    if RETIRED_PRODUCT_CAPSULES.contains(&name) {
        anyhow::bail!("Capsule '{}' is retired and cannot be published", name);
    }
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
    validate_platform_input_options(options)?;
    if !options.platform_inputs.is_empty() {
        let mut command = Command::new("python3");
        command
            .arg(workspace_root.join("scripts/release-platform-input.py"))
            .arg("validate-inputs")
            .arg("--version")
            .arg(&options.version)
            .current_dir(workspace_root);
        for input in &options.platform_inputs {
            command.arg("--input").arg(input);
        }
        if let Some(platform) = &options.preview_platform {
            command.arg("--preview-platform").arg(platform);
        }
        let output = command
            .output()
            .context("Failed to validate prepared release platform inputs")?;
        if !output.status.success() {
            anyhow::bail!(
                "Prepared release platform input validation failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        return Ok(());
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

fn validate_prepare_options(options: &PublishReleaseOptions) -> anyhow::Result<()> {
    if options.key.is_some() {
        anyhow::bail!("The separate custodian tool owns release keys; publish-release accepts public data only");
    }
    if options.allow_no_bootstrap {
        anyhow::bail!("The approved signing input owns the frozen public bootstrap; omit --allow-no-bootstrap");
    }
    if options.signed_publication.is_some() {
        if options.prepare_only.is_some()
            || !options.platform_inputs.is_empty()
            || options.preview_platform.is_some()
            || options.skip_build
            || options.skip_rootfs
            || options.cross.is_some()
            || !options.capsules.is_empty()
            || options.profile != "home"
            || options.public_url
            || options.public_with_sudo
        {
            anyhow::bail!("Signed publication consumes the custodian's complete frozen set; omit build, capsule and public-URL options");
        }
        validate_public_signer_did(
            options
                .publisher_did
                .as_deref()
                .context("--signed-publication requires --publisher-did")?,
        )?;
        return Ok(());
    }
    if options.allow_signer_rotation {
        anyhow::bail!("--allow-signer-rotation requires --signed-publication");
    }
    if options.prepare_only.is_some() {
        if options.platform_inputs.is_empty()
            || options.key.is_some()
            || options.public_url
            || options.public_with_sudo
        {
            anyhow::bail!("Unsigned preparation requires native inputs and public signer data; the custodian owns signing");
        }
        let did = options
            .publisher_did
            .as_deref()
            .context("--prepare-only requires --publisher-did")?;
        validate_public_signer_did(did)?;
    } else if options.publisher_did.is_some() {
        anyhow::bail!("--publisher-did requires --prepare-only");
    }
    Ok(())
}

fn validate_public_signer_did(did: &str) -> anyhow::Result<()> {
    let key = elastos_server::crypto::decode_did_key(did)?;
    anyhow::ensure!(
        elastos_server::crypto::encode_did_key(&key)? == did,
        "Canonical public Ed25519 signer DID required"
    );
    Ok(())
}

fn validate_platform_input_options(options: &PublishReleaseOptions) -> anyhow::Result<()> {
    if let Some(platform) = &options.preview_platform {
        if platform != "aarch64-darwin" {
            anyhow::bail!("--preview-platform supports aarch64-darwin only");
        }
        if options.channel != "canary" {
            anyhow::bail!("--preview-platform requires --channel canary");
        }
        if !options.dry_run && !options.preflight_only && options.prepare_only.is_none() {
            anyhow::bail!(
                "--preview-platform requires --prepare-only, --dry-run or --preflight-only"
            );
        }
        if options.platform_inputs.len() != 1 {
            anyhow::bail!(
                "--preview-platform requires exactly one --platform-input aarch64-darwin=DIR"
            );
        }
    }
    if options.platform_inputs.is_empty() {
        return Ok(());
    }
    if options.skip_build || options.skip_rootfs || options.cross.is_some() {
        anyhow::bail!("--platform-input conflicts with --skip-build, --skip-rootfs, and --cross");
    }
    if !options.capsules.is_empty() || options.profile != "home" {
        anyhow::bail!("--platform-input requires the home profile and the prepared component inventory; omit --capsules");
    }
    let expected = BTreeSet::from(["x86_64-linux", "aarch64-linux", "aarch64-darwin"]);
    let mut supplied = BTreeSet::new();
    for input in &options.platform_inputs {
        let (platform, _) = input
            .split_once('=')
            .filter(|(_, directory)| !directory.is_empty())
            .ok_or_else(|| anyhow::anyhow!("--platform-input requires PLATFORM=DIR: {}", input))?;
        if !expected.contains(platform) || !supplied.insert(platform) {
            anyhow::bail!("--platform-input requires each supported platform exactly once: x86_64-linux, aarch64-linux, aarch64-darwin");
        }
    }
    if let Some(platform) = &options.preview_platform {
        if !supplied.contains(platform.as_str()) {
            anyhow::bail!(
                "--preview-platform requires exactly one --platform-input aarch64-darwin=DIR"
            );
        }
    } else if supplied.len() < 2 {
        anyhow::bail!("--platform-input requires at least two platforms: x86_64-linux, aarch64-linux, aarch64-darwin");
    }
    Ok(())
}

fn resolve_platform_input_paths(inputs: &mut [String], caller_dir: &Path) -> anyhow::Result<()> {
    for input in inputs {
        let (platform, directory) = input
            .split_once('=')
            .context("--platform-input requires PLATFORM=DIR")?;
        // Joining keeps absolute paths and binds relative paths before subprocess cwd changes.
        let directory = caller_dir.join(directory);
        let directory = directory
            .to_str()
            .context("Prepared platform input paths must use UTF-8")?;
        *input = format!("{platform}={directory}");
    }
    Ok(())
}

fn append_publish_selection_args(
    command: &mut Command,
    options: &PublishReleaseOptions,
    selected_capsules: &[String],
) {
    if options.platform_inputs.is_empty() {
        command.arg("--capsules").arg(selected_capsules.join(","));
    } else {
        for input in &options.platform_inputs {
            command.arg("--platform-input").arg(input);
        }
        if let Some(platform) = &options.preview_platform {
            command.arg("--preview-platform").arg(platform);
        }
    }
}

fn print_publish_selection(options: &PublishReleaseOptions, selected_capsules: &[String]) {
    if options.platform_inputs.is_empty() {
        println!("  Capsules:  {}", selected_capsules.join(", "));
    } else {
        println!("  Mode:      import verified native Home platform inputs");
        if let Some(platform) = &options.preview_platform {
            println!("  Preview:   {platform} on canary (operator publication)");
        }
        for input in &options.platform_inputs {
            println!("  Input:     {}", input);
        }
        println!("  Inventory: prepared Home apps, providers, and metadata");
    }
}

struct CrossBuildDetails {
    binary_path: PathBuf,
    artifacts_dir: &'static str,
}

fn cross_build_details(arch: &str, ws_root: &Path) -> anyhow::Result<CrossBuildDetails> {
    let target_dir = cargo_target_dir(ws_root);
    match arch {
        "aarch64" => Ok(CrossBuildDetails {
            binary_path: target_dir.join("aarch64-unknown-linux-musl/release/elastos"),
            artifacts_dir: "artifacts-aarch64",
        }),
        other => anyhow::bail!("Unsupported cross architecture: {}", other),
    }
}

struct PublishPreflight {
    script_path: PathBuf,
    ipfs_provider_bin: Option<PathBuf>,
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

    if options.platform_inputs.is_empty() && !options.skip_build && which_in_path("cargo").is_none()
    {
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

    if options.platform_inputs.is_empty() && !options.skip_rootfs {
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
    if let Some(output) = &options.prepare_only {
        println!("  Unsigned input: {}", output.display());
        println!("  Signing: separately installed custodian tool");
    } else {
        println!("  Signing: separately installed custodian tool");
    }
    println!(
        "  IPFS bin:  {}",
        preflight
            .ipfs_provider_bin
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "missing".to_string())
    );
    print_publish_selection(options, &preflight.selected_capsules);
    if let Some(cross) = &options.cross {
        println!("  Cross:     {}", cross);
    }
    println!("  Tools:     {}", preflight.available_tools.join(", "));
    println!("  Result:    preflight passed");
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
        release_object_cid: head.payload.release_object_cid,
        head_cid: head_cid.to_string(),
        published_at: release.payload.released_at,
        signer_did: release.signer_did,
        selected_capsules: selected_capsules.to_vec(),
        platforms,
    })
}

fn components_artifact_path(artifacts_dir: &Path, platform: &str) -> PathBuf {
    let root = if artifacts_dir.join("artifacts").is_dir() {
        artifacts_dir.join("artifacts")
    } else {
        artifacts_dir.to_owned()
    };
    root.join(format!("components-{platform}.json"))
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
    if let Some(cid) = &current.release_object_cid {
        println!("  Release object: {}", cid);
    }
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
    let coords_path = super::runtime_control::runtime_coord_path(&data_dir);
    let coords = super::runtime_control::read_runtime_coords(&coords_path)
        .await
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No running runtime found. Start `elastos serve` first for gossip announcements."
            )
        })?;

    let tokens = super::runtime_control::attach_to_runtime(&coords).await?;
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
            "release_object_cid": entry.release_object_cid,
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
        options
            .publisher_did
            .as_deref()
            .unwrap_or("(operator supplies the public DID)")
    );
    if let Some(output) = &options.prepare_only {
        println!("  Unsigned input: {}", output.display());
        println!("  Signing: separately installed custodian tool");
    } else {
        println!("  Signing: separately installed custodian tool");
    }
    print_publish_selection(options, selected_capsules);
    if options.platform_inputs.is_empty() {
        println!("  Available: {}", available_capsules.join(", "));
    }
    println!(
        "  Build:     {}",
        if !options.platform_inputs.is_empty() {
            "use admitted native binaries"
        } else if options.skip_build {
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
        if !options.platform_inputs.is_empty() {
            "use admitted Home archives"
        } else if options.skip_rootfs {
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
    if options.platform_inputs.is_empty() {
        println!(
            "  Cross:     {}",
            options.cross.as_deref().unwrap_or("host platform only")
        );
    }
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
        append_publish_selection_args, build_release_ledger_entry, changed_capsules,
        discover_available_capsules, load_publish_state, publish_profile_capsules,
        release_discovery_topics, resolve_platform_input_paths, save_publish_state,
        select_capsules, source_discovery_uri, validate_platform_input_options,
        validate_prepare_options, validate_publish_inputs, validate_publishable_manifest,
        PublishReleaseOptions, PublishState, ReleaseLedgerPlatform, DEFAULT_PUBLISH_CAPSULES,
        DEMO_PUBLISH_CAPSULES, RETIRED_PRODUCT_CAPSULES,
    };
    use elastos_common::{
        CapsuleManifest, CapsuleType, MicroVmConfig, Permissions, RequirementKind, ResourceLimits,
    };
    use std::collections::BTreeMap;
    use std::path::Path;

    fn platform_input_options() -> PublishReleaseOptions {
        PublishReleaseOptions {
            version: "0.7.1".to_string(),
            channel: "stable".to_string(),
            profile: "home".to_string(),
            skip_build: false,
            skip_rootfs: false,
            cross: None,
            capsules: Vec::new(),
            platform_inputs: ["x86_64-linux", "aarch64-linux", "aarch64-darwin"]
                .iter()
                .map(|platform| format!("{platform}=/prepared/{platform}"))
                .collect(),
            preview_platform: None,
            prepare_only: None,
            signed_publication: None,
            allow_signer_rotation: false,
            publisher_did: None,
            key: None,
            dry_run: false,
            preflight_only: false,
            public_url: false,
            public_with_sudo: false,
            gateway_addr: "127.0.0.1:8090".to_string(),
            public_timeout: 60,
            ipfs_provider_bin: None,
            allow_no_bootstrap: false,
        }
    }

    #[test]
    fn test_platform_input_options_require_at_least_two_unique_supported_platforms() {
        let options = platform_input_options();
        validate_platform_input_options(&options).unwrap();
        let mut two = options.clone();
        two.platform_inputs.pop();
        validate_platform_input_options(&two).unwrap();
        let mut one = two.clone();
        one.platform_inputs.pop();
        assert!(validate_platform_input_options(&one).is_err());
        for invalid in [
            "x86_64-linux=/duplicate",
            "other-platform=/unsupported",
            "aarch64-darwin=",
            "aarch64-darwin",
        ] {
            let mut invalid_options = options.clone();
            invalid_options.platform_inputs[2] = invalid.to_string();
            assert!(
                validate_platform_input_options(&invalid_options).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn test_platform_input_options_reject_build_and_selection_overrides() {
        for flag in ["skip-build", "skip-rootfs", "cross", "capsules", "profile"] {
            let mut options = platform_input_options();
            match flag {
                "skip-build" => options.skip_build = true,
                "skip-rootfs" => options.skip_rootfs = true,
                "cross" => options.cross = Some("aarch64".to_string()),
                "capsules" => options.capsules = vec!["home".to_string()],
                "profile" => options.profile = "demo".to_string(),
                _ => unreachable!(),
            }
            assert!(validate_platform_input_options(&options).is_err(), "{flag}");
        }
    }

    fn preview_options() -> PublishReleaseOptions {
        let mut options = platform_input_options();
        options.channel = "canary".to_string();
        options.platform_inputs = vec!["aarch64-darwin=/prepared/mac".to_string()];
        options.preview_platform = Some("aarch64-darwin".to_string());
        options.dry_run = true;
        options
    }

    #[test]
    fn test_platform_input_unsigned_preparation_keeps_custodian_key_boundary() {
        let mut options = preview_options();
        options.dry_run = false;
        options.prepare_only = Some("/prepared/unsigned".into());
        options.publisher_did =
            Some("did:key:z6MktwupdmLXVVqTzCw4i46r4uGyosGXRnR3XjN4Zq7oMMsw".to_string());
        validate_prepare_options(&options).unwrap();
        validate_platform_input_options(&options).unwrap();
        for refusal in [
            "key",
            "publisher",
            "invalid-did",
            "noncanonical-did",
            "inputs",
            "public-url",
            "sudo",
        ] {
            let mut invalid = options.clone();
            match refusal {
                "key" => invalid.key = Some("/custodian/unopened.pem".into()),
                "publisher" => invalid.publisher_did = None,
                "invalid-did" => {
                    invalid.publisher_did = Some("did:web:example.invalid".to_string())
                }
                "noncanonical-did" => {
                    invalid.publisher_did =
                        Some(format!(" {}", options.publisher_did.as_deref().unwrap()))
                }
                "inputs" => invalid.platform_inputs.clear(),
                "public-url" => invalid.public_url = true,
                "sudo" => invalid.public_with_sudo = true,
                _ => unreachable!(),
            }
            assert!(validate_prepare_options(&invalid).is_err(), "{refusal}");
        }
        options.prepare_only = None;
        assert!(validate_prepare_options(&options).is_err());
    }

    #[test]
    fn test_platform_input_preview_accepts_only_canary_mac_inspection() {
        let options = preview_options();
        validate_platform_input_options(&options).unwrap();
        let mut preflight = options.clone();
        preflight.dry_run = false;
        preflight.preflight_only = true;
        validate_platform_input_options(&preflight).unwrap();
        for case in [
            "publication",
            "stable",
            "unsupported",
            "absent",
            "mixed",
            "wrong-input",
            "malformed",
            "skip-build",
            "skip-rootfs",
            "cross",
            "capsules",
            "profile",
        ] {
            let mut invalid = options.clone();
            match case {
                "publication" => invalid.dry_run = false,
                "stable" => invalid.channel = "stable".to_string(),
                "unsupported" => invalid.preview_platform = Some("aarch64-linux".to_string()),
                "absent" => invalid.platform_inputs.clear(),
                "mixed" => invalid
                    .platform_inputs
                    .push("x86_64-linux=/prepared/linux".to_string()),
                "wrong-input" => {
                    invalid.platform_inputs[0] = "aarch64-linux=/prepared/linux".to_string()
                }
                "malformed" => invalid.platform_inputs[0] = "aarch64-darwin=".to_string(),
                "skip-build" => invalid.skip_build = true,
                "skip-rootfs" => invalid.skip_rootfs = true,
                "cross" => invalid.cross = Some("aarch64".to_string()),
                "capsules" => invalid.capsules = vec!["home".to_string()],
                "profile" => invalid.profile = "demo".to_string(),
                _ => unreachable!(),
            }
            assert!(validate_platform_input_options(&invalid).is_err(), "{case}");
        }
    }

    #[test]
    fn test_platform_input_preview_arguments_reach_publisher_and_input_admission() {
        let options = preview_options();
        let mut command = std::process::Command::new("bash");
        append_publish_selection_args(&mut command, &options, &[]);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "--platform-input",
                "aarch64-darwin=/prepared/mac",
                "--preview-platform",
                "aarch64-darwin"
            ]
        );

        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("scripts")).unwrap();
        std::fs::write(
            temp.path().join("scripts/release-platform-input.py"),
            "import sys\nassert sys.argv[1:] == ['validate-inputs', '--version', '0.7.1', '--input', 'aarch64-darwin=/prepared/mac', '--preview-platform', 'aarch64-darwin']\n",
        ).unwrap();
        validate_publish_inputs(&options, temp.path(), &[]).unwrap();
    }

    #[test]
    fn test_platform_input_arguments_forward_without_capsule_selection() {
        let mut options = platform_input_options();
        let mut command = std::process::Command::new("bash");
        append_publish_selection_args(&mut command, &options, &["home".to_string()]);
        let args = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            [
                "--platform-input",
                "x86_64-linux=/prepared/x86_64-linux",
                "--platform-input",
                "aarch64-linux=/prepared/aarch64-linux",
                "--platform-input",
                "aarch64-darwin=/prepared/aarch64-darwin",
            ]
        );

        options.platform_inputs.clear();
        let mut command = std::process::Command::new("bash");
        append_publish_selection_args(&mut command, &options, &["home".to_string()]);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["--capsules", "home"]
        );
    }

    #[test]
    fn test_platform_input_relative_paths_forward_from_the_caller_directory() {
        let mut options = platform_input_options();
        options.platform_inputs = vec![
            "x86_64-linux=prepared inputs/linux=amd64".to_string(),
            "aarch64-linux=/prepared inputs/linux=arm64".to_string(),
            "aarch64-darwin=prepared/mac".to_string(),
        ];
        resolve_platform_input_paths(&mut options.platform_inputs, Path::new("/caller/session"))
            .unwrap();
        let mut command = std::process::Command::new("bash");
        command.current_dir("/reviewed/workspace");
        append_publish_selection_args(&mut command, &options, &[]);
        let args = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            [
                "--platform-input",
                "x86_64-linux=/caller/session/prepared inputs/linux=amd64",
                "--platform-input",
                "aarch64-linux=/prepared inputs/linux=arm64",
                "--platform-input",
                "aarch64-darwin=/caller/session/prepared/mac",
            ]
        );
    }

    #[tokio::test]
    async fn key_option_is_refused_before_platform_reads_or_key_creation() {
        let temp = tempfile::tempdir().unwrap();
        let key = temp.path().join("publisher/key");
        for dry_run in [false, true] {
            let mut options = platform_input_options();
            options.dry_run = dry_run;
            options.key = Some(key.clone());
            options.platform_inputs = ["x86_64-linux", "aarch64-linux", "aarch64-darwin"]
                .iter()
                .map(|platform| format!("{platform}={}", temp.path().join(platform).display()))
                .collect();
            let error = super::run_publish_release(options).await.unwrap_err();
            assert!(error.to_string().contains("custodian"), "{error}");
            assert!(!key.parent().unwrap().exists());
        }
    }

    fn test_manifest(name: &str, capsule_requires: &[&str]) -> CapsuleManifest {
        CapsuleManifest {
            schema: elastos_common::SCHEMA_V1.to_string(),
            version: "0.1.0".to_string(),
            name: name.to_string(),
            description: None,
            author: None,
            role: elastos_common::CapsuleRole::App,
            capsule_type: CapsuleType::MicroVM,
            runtime_abi: None,
            bus_contract: None,
            wit_world_sha256: None,
            execution: None,
            projections: Vec::new(),
            entrypoint: "rootfs.ext4".to_string(),
            requires: capsule_requires
                .iter()
                .map(|requirement| elastos_common::CapsuleRequirement {
                    name: (*requirement).to_string(),
                    kind: RequirementKind::Capsule,
                })
                .collect(),
            provides: None,
            authority: None,
            capabilities: Vec::new(),
            interfaces: Vec::new(),
            resources: ResourceLimits::default(),
            permissions: Permissions::default(),
            microvm: Some(MicroVmConfig::default()),
            providers: None,
            icon: None,
            viewer: None,
            window_policy: None,
            model_content: None,
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
    fn test_select_capsules_defaults_to_home_publish_set() {
        let entries = DEFAULT_PUBLISH_CAPSULES
            .iter()
            .map(|name| (*name, &[][..]))
            .collect::<Vec<(&str, &[&str])>>();
        let manifests = test_manifests(&entries);
        let selected = select_capsules("home", &[], &manifests).unwrap();
        let mut expected = DEFAULT_PUBLISH_CAPSULES
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(selected, expected);
        assert!(selected.contains(&"archive-manager".to_string()));
        assert!(selected.contains(&"services".to_string()));
        assert!(selected.contains(&"content-block-graph-provider".to_string()));
        assert!(selected.contains(&"chain-provider".to_string()));
        assert!(selected.contains(&"wallet-provider".to_string()));
        assert!(selected.contains(&"ipfs-provider".to_string()));
        assert!(!selected.contains(&"custody-provider".to_string()));
        assert!(!selected.contains(&"chat-room".to_string()));
        assert!(!selected.contains(&"gba-emulator".to_string()));
    }

    #[test]
    fn test_publish_profile_demo_extends_home_with_demo_capsules() {
        let mut entries = DEFAULT_PUBLISH_CAPSULES
            .iter()
            .chain(DEMO_PUBLISH_CAPSULES.iter())
            .map(|name| (*name, &[][..]))
            .collect::<Vec<(&str, &[&str])>>();
        entries.sort_by_key(|(name, _)| *name);
        entries.dedup_by_key(|(name, _)| *name);
        let manifests = test_manifests(&entries);
        let selected = select_capsules("demo", &[], &manifests).unwrap();

        assert!(selected.contains(&"services".to_string()));
        assert!(selected.contains(&"content-block-graph-provider".to_string()));
        assert!(selected.contains(&"chat-room".to_string()));
        assert!(selected.contains(&"gba-emulator".to_string()));
        assert!(selected.contains(&"ipfs-provider".to_string()));
    }

    #[test]
    fn test_select_capsules_rejects_unknown_name() {
        let manifests = test_manifests(&[("sample-app", &[])]);
        let err = select_capsules("demo", &["missing".to_string()], &manifests).unwrap_err();
        assert!(err.to_string().contains("Unknown capsule"));
    }

    #[test]
    fn test_select_capsules_expands_transitive_capsule_dependencies() {
        let manifests = test_manifests(&[
            ("sample-app", &["did-provider"]),
            ("did-provider", &[]),
            ("shell", &[]),
        ]);
        let selected = select_capsules(
            "demo",
            &["shell".to_string(), "sample-app".to_string()],
            &manifests,
        )
        .unwrap();
        assert_eq!(
            selected,
            vec![
                "did-provider".to_string(),
                "sample-app".to_string(),
                "shell".to_string(),
            ]
        );
    }

    #[test]
    fn test_select_capsules_rejects_missing_transitive_dependency() {
        let manifests = test_manifests(&[("sample-app", &["ipfs-provider"])]);
        let err = select_capsules("demo", &["sample-app".to_string()], &manifests).unwrap_err();
        assert!(err.to_string().contains("requires capsule 'ipfs-provider'"));
    }

    #[test]
    fn test_select_capsules_rejects_retired_product_capsules() {
        let manifests = test_manifests(&[("chat", &[]), ("agent", &[])]);
        for retired in RETIRED_PRODUCT_CAPSULES {
            let err = select_capsules("demo", &[retired.to_string()], &manifests).unwrap_err();
            assert!(err
                .to_string()
                .contains("is retired and cannot be published"));
        }
    }

    #[test]
    fn test_publish_profile_full_uses_all_available_capsules() {
        let available = vec![
            "chat".to_string(),
            "agent".to_string(),
            "shell".to_string(),
            "did-provider".to_string(),
        ];
        assert_eq!(
            publish_profile_capsules("full", &available).unwrap(),
            vec!["did-provider".to_string(), "shell".to_string()]
        );
    }

    #[test]
    fn test_publish_profile_providers_includes_first_party_provider_capsules() {
        let selected = publish_profile_capsules("providers", &[]).unwrap();

        assert!(selected.contains(&"chain-provider".to_string()));
        assert!(selected.contains(&"content-block-graph-provider".to_string()));
        assert!(selected.contains(&"net-provider".to_string()));
        assert!(selected.contains(&"exit-provider".to_string()));
        assert!(selected.contains(&"wallet-provider".to_string()));
        assert!(selected.contains(&"object-provider".to_string()));
        assert!(selected.contains(&"webspace-provider".to_string()));
        assert!(selected.contains(&"drm-provider".to_string()));
        assert!(selected.contains(&"rights-provider".to_string()));
        assert!(selected.contains(&"key-provider".to_string()));
        assert!(selected.contains(&"decrypt-provider".to_string()));
        assert!(selected.contains(&"availability-provider".to_string()));
    }

    #[cfg(unix)]
    fn import_provider_fixture(
        root: &Path,
        receipt: serde_json::Value,
        status: u8,
    ) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let provider = root.join("fixture-provider");
        let body = format!(
            "#!/bin/sh\nIFS= read -r init || exit 94\nIFS= read -r operation || exit 95\nprintf '%s\\n' \"$init\" \"$operation\" > \"$0.requests\"\ncat <<'PUBLIC_FIXTURE_RECEIPT'\n{receipt}\nPUBLIC_FIXTURE_RECEIPT\nexit {status}\n"
        );
        std::fs::write(&provider, body).unwrap();
        std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o700)).unwrap();
        provider
    }

    #[cfg(unix)]
    fn import_fixture_requests(provider: &Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(provider.with_extension("requests"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[cfg(unix)]
    fn model_import_fixture(
        root: &Path,
        mode: &str,
    ) -> (
        PathBuf,
        PathBuf,
        PathBuf,
        Vec<elastos_server::release_publication::ModelRetention>,
    ) {
        use std::os::unix::fs::PermissionsExt;
        let data = root.join("data");
        let artifacts = root.join("artifacts");
        std::fs::create_dir_all(data.join("ipfs-repo")).unwrap();
        std::fs::create_dir(&artifacts).unwrap();
        std::fs::create_dir(root.join("scripts")).unwrap();
        let provider = root.join("model-provider-fixture");
        let provider_program = r#"#!/usr/bin/env python3
import base64, hashlib, http.server, json, os, pathlib, sys, threading
cid = 'b' + base64.b32encode(b'\x01\x55\x12\x20' + hashlib.sha256(b'public model provider fixture').digest()).decode().lower().rstrip('=')
with open(__file__ + '.starts', 'a') as sink: sink.write(str(os.getpid()) + '\n')
repo = pathlib.Path(__file__).parent / 'data' / 'ipfs-repo'
class API(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_POST(self):
        path = str(repo) if __MODE__ != 'wrong-repo' else str(repo.parent / 'other-repo')
        value = {'RepoPath':path, 'RepoSize':0} if self.path == '/api/v0/repo/stat' else {'Key':cid}
        data = json.dumps(value).encode()
        self.send_response(200)
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)
server = http.server.HTTPServer(('127.0.0.1', 0), API)
endpoint = 'http://127.0.0.1:' + str(server.server_port)
pathlib.Path(__file__ + '.api').write_text(endpoint + '/')
worker = threading.Thread(target=server.serve_forever, daemon=True)
worker.start()
prepares = 0
try:
    for line in sys.stdin:
        with open(__file__ + '.requests', 'a') as sink: sink.write(line)
        request = json.loads(line)
        data = {}
        if request['op'] == 'runtime_prepare_backend':
            prepares += 1
            if __MODE__ == 'replaced-coords' and prepares > 1:
                (repo.parent / 'ipfs-coords.json').write_text(json.dumps({'api_port':1}))
        if request['op'] == 'add_path': data = {'cid':cid}
        if request['op'] == 'status':
            api = endpoint
            if __MODE__ == 'changed-api' and (repo.parent.parent / 'scripts/model-package-handoff.requests').exists():
                api = endpoint.replace('127.0.0.1', 'localhost')
            data = {'api_endpoint':api}
        if request['op'] == 'runtime_check_capacity':
            stats = os.statvfs(repo)
            data = {'volume_id':os.stat(repo).st_dev, 'capacity_bytes':stats.f_blocks * stats.f_frsize,
                'available_bytes':stats.f_bavail * stats.f_frsize, 'required_bytes':request['required_bytes']}
        print(json.dumps({'status':'ok','data':data}), flush=True)
        if request['op'] == 'shutdown': break
finally:
    server.shutdown()
    worker.join()
    server.server_close()
"#.replace("__MODE__", &serde_json::to_string(mode).unwrap());
        std::fs::write(&provider, provider_program).unwrap();
        std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o700)).unwrap();
        let script = root.join("scripts/model-package-handoff.py");
        let program = format!(
            r#"import json, pathlib, sys, urllib.request
args = sys.argv[1:]
with pathlib.Path(__file__).with_suffix('.requests').open('a') as sink:
    sink.write(json.dumps(args) + '\n')
if {mode:?} == 'replaced-coords':
    root = pathlib.Path(__file__).parent.parent
    assert json.loads((root / 'data/ipfs-coords.json').read_text())['api_port'] == 1
    assert '--data-dir' not in args
    api = args[args.index('--kubo-api') + 1]
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({{}}))
    with opener.open(urllib.request.Request(api.rstrip('/') + '/api/v0/repo/stat', data=b''), timeout=2) as response:
        assert json.load(response)['RepoPath'] == str(root / 'data/ipfs-repo')
if {mode:?} == 'failure':
    sys.exit(7)
if {mode:?} == 'oversized':
    print('x' * 65537)
    sys.exit(0)
cid = args[args.index('--cid') + 1]
print(json.dumps({{'schema':'elastos.model.package-import/v1','package_cid':cid,
    'pinned':{pin},'blocks':1,'block_bytes':5,'kubo_version':'0.40.1'}}))
"#,
            pin = if mode == "unpinned" { "False" } else { "True" }
        );
        std::fs::write(script, program).unwrap();
        let records = (0..4)
            .map(|index| {
                let bytes = format!("public model fixture {index}");
                let cid = import_fixture_raw_cid(bytes.as_bytes());
                let raw = cid::Cid::try_from(cid.as_str()).unwrap();
                let package_cid = cid::Cid::new_v1(0x70, *raw.hash()).to_string();
                let descriptor =
                    |name: String| elastos_server::release_publication::ArtifactDescriptor {
                        name,
                        cid: cid.clone(),
                        sha256: "a".repeat(64),
                        size: 32,
                    };
                elastos_server::release_publication::ModelRetention {
                    component: format!("model-fixture-{index}"),
                    package_cid,
                    car: descriptor(format!("model-fixture-{index}.car")),
                    receipt: descriptor(format!("model-fixture-{index}.car.receipt.json")),
                }
            })
            .collect();
        (provider, data, artifacts, records)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn publication_files_raw_blocks_and_model_imports_share_one_owned_provider() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (provider, data, artifacts, records) = model_import_fixture(&root, "success");
        let file = artifacts.join("public-file.tar");
        std::fs::write(&file, b"public fixture").unwrap();
        let mut session = super::PublicationProvider::start(&provider, &data)
            .await
            .unwrap();
        let expected = import_fixture_raw_cid(b"public model provider fixture");
        assert_eq!(session.import_file(&file).await.unwrap(), expected);
        assert_eq!(
            session
                .import_raw_block("model-catalog.json", b"public fixture".to_vec())
                .await
                .unwrap(),
            expected
        );
        super::import_publication_models_with_session(
            &mut session,
            &root,
            &data,
            &artifacts,
            &records,
        )
        .await
        .unwrap();
        session.finish().await.unwrap();
        let starts = std::fs::read_to_string(format!("{}.starts", provider.display())).unwrap();
        assert_eq!(starts.lines().count(), 1);
        let requests = import_fixture_requests(&provider);
        assert_eq!(
            requests
                .iter()
                .filter(|request| request["op"] == "init")
                .count(),
            1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| request["op"] == "add_path")
                .count(),
            1
        );
        assert_eq!(requests.last().unwrap()["op"], "shutdown");
        assert_eq!(
            std::fs::read_to_string(root.join("scripts/model-package-handoff.requests"))
                .unwrap()
                .lines()
                .count(),
            4
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn model_retention_batch_keeps_provider_ready_and_passes_exact_import_inputs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (provider, data, artifacts, records) = model_import_fixture(&root, "success");
        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            super::import_publication_models(&provider, &root, &data, &artifacts, &records),
        )
        .await
        .unwrap()
        .unwrap();
        let requests = import_fixture_requests(&provider);
        assert_eq!(
            requests.first().unwrap(),
            &serde_json::json!({"op":"init","config":{}})
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| request["op"] == "runtime_prepare_backend")
                .count(),
            5
        );
        assert_eq!(
            requests.last().unwrap(),
            &serde_json::json!({"op":"shutdown"})
        );
        let arguments: Vec<Vec<String>> =
            std::fs::read_to_string(root.join("scripts/model-package-handoff.requests"))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
        assert_eq!(arguments.len(), 4);
        let api = std::fs::read_to_string(format!("{}.api", provider.display())).unwrap();
        for (args, record) in arguments.iter().zip(&records) {
            assert_eq!(args[0], "import");
            assert!(!args.iter().any(|arg| arg == "--data-dir"));
            for (flag, expected) in [
                ("--kubo-api", api.clone()),
                ("--cid", record.package_cid.clone()),
                (
                    "--car",
                    artifacts
                        .join(&record.car.name)
                        .to_string_lossy()
                        .into_owned(),
                ),
                (
                    "--receipt",
                    artifacts
                        .join(&record.receipt.name)
                        .to_string_lossy()
                        .into_owned(),
                ),
                ("--timeout-seconds", "60".to_owned()),
            ] {
                assert_eq!(
                    &args[args.iter().position(|arg| arg == flag).unwrap() + 1],
                    &expected
                );
            }
            assert!(
                args[args
                    .iter()
                    .position(|arg| arg == "--free-space-floor-bytes")
                    .unwrap()
                    + 1]
                .parse::<u64>()
                .unwrap()
                    > 0
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn model_import_keeps_admitted_provider_api_when_coordinates_are_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (provider, data, artifacts, records) = model_import_fixture(&root, "replaced-coords");
        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            super::import_publication_models(&provider, &root, &data, &artifacts, &records),
        )
        .await
        .unwrap()
        .unwrap();
        let imports =
            std::fs::read_to_string(root.join("scripts/model-package-handoff.requests")).unwrap();
        assert_eq!(imports.lines().count(), 4);
        let coordinates: serde_json::Value =
            serde_json::from_slice(&std::fs::read(data.join("ipfs-coords.json")).unwrap()).unwrap();
        assert_eq!(coordinates["api_port"], 1);
        assert_eq!(
            import_fixture_requests(&provider).last().unwrap()["op"],
            "shutdown"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn model_import_failure_refuses_the_batch_and_preserves_previous_public_files() {
        for mode in ["failure", "unpinned", "oversized", "changed-api"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let (provider, data, artifacts, records) = model_import_fixture(&root, mode);
            let previous = data.join("previous-public-head.json");
            std::fs::write(&previous, b"accepted previous public generation").unwrap();
            let error = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                super::import_publication_models(&provider, &root, &data, &artifacts, &records),
            )
            .await
            .unwrap()
            .unwrap_err();
            assert!(error.to_string().contains("Model"), "{error}");
            assert_eq!(
                std::fs::read(&previous).unwrap(),
                b"accepted previous public generation"
            );
            let requests = import_fixture_requests(&provider);
            assert_eq!(
                requests.last().unwrap(),
                &serde_json::json!({"op":"shutdown"})
            );
            let imports =
                std::fs::read_to_string(root.join("scripts/model-package-handoff.requests"))
                    .unwrap();
            assert_eq!(
                imports.lines().count(),
                1,
                "failed import must stop the remaining batch"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn model_import_refuses_an_api_repository_outside_selected_runtime_before_writes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (provider, data, artifacts, records) = model_import_fixture(&root, "wrong-repo");
        let error = super::import_publication_models(&provider, &root, &data, &artifacts, &records)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("repository differs"), "{error}");
        assert!(!root.join("scripts/model-package-handoff.requests").exists());
        assert_eq!(
            import_fixture_requests(&provider).last().unwrap()["op"],
            "shutdown"
        );
    }

    #[test]
    fn publication_budget_reserves_copies_file_blocks_and_directory_blocks_before_writes() {
        let budget = super::storage_budget(100, 2, 60).unwrap();
        assert_eq!(budget.local_bytes, 200 + 2 * 1024 * 1024);
        assert_eq!(budget.repo_bytes, budget.local_bytes + 60);
        assert!(super::storage_budget(u64::MAX, 1, 1).is_err());
        let repo = super::StorageObservation {
            volume: 7,
            capacity: 1000,
            free: 350,
        };
        let value = serde_json::json!({"volume_id":7,"capacity_bytes":1000,"available_bytes":350,"required_bytes":150});
        // Each 150-byte write would fit on its own, but their combined peak
        // would leave only 50 bytes on the same volume, below the 15% floor.
        assert!(super::validate_publication_capacity(&value, 150, repo, repo, 150, 150).is_err());
        let other = super::StorageObservation {
            volume: 8,
            capacity: 1000,
            free: 350,
        };
        assert!(super::validate_publication_capacity(&value, 150, repo, other, 150, 150).is_ok());
        let wrong = serde_json::json!({"volume_id":8,"capacity_bytes":1000,"available_bytes":350,"required_bytes":150});
        assert!(super::validate_publication_capacity(&wrong, 150, repo, other, 150, 150).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn model_storage_preflight_reserves_all_records_before_import() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (_provider, data, _artifacts, mut records) = model_import_fixture(&root, "success");
        for record in &mut records {
            record.car.size = u64::MAX;
        }
        assert!(super::model_import_space_guard(&data.join("ipfs-repo"), &records).is_err());
    }

    #[cfg(unix)]
    fn import_fixture_raw_cid(bytes: &[u8]) -> String {
        use sha2::Digest;
        let hash =
            cid::multihash::Multihash::<64>::wrap(0x12, &sha2::Sha256::digest(bytes)).unwrap();
        cid::Cid::new_v1(0x55, hash).to_string()
    }

    #[cfg(unix)]
    async fn raw_import_api_fixture(
        receipt: serde_json::Value,
        redirect: bool,
    ) -> (String, tokio::task::JoinHandle<(Vec<u8>, bool)>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let location = format!("Location: {endpoint}/unexpected-follow\r\n");
        let task = tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let header_end = loop {
                    let mut chunk = [0; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0, "import closed before its headers arrived");
                    request.extend_from_slice(&chunk[..count]);
                    assert!(request.len() < 32 * 1024, "fixture request exceeds its limit");
                    if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        break end + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let length: usize = headers.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().unwrap())
                }).expect("multipart import must state its length");
                assert!(header_end + length < 32 * 1024);
                while request.len() < header_end + length {
                    let mut chunk = [0; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0, "import closed before its body arrived");
                    request.extend_from_slice(&chunk[..count]);
                }
                let body = serde_json::to_vec(&receipt).unwrap();
                let status = if redirect { "302 Found" } else { "200 OK" };
                let redirect_header = if redirect { location.as_str() } else { "" };
                let response = format!("HTTP/1.1 {status}\r\n{redirect_header}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                socket.write_all(response.as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
                socket.shutdown().await.unwrap();
                drop(socket);
                let followed = redirect && tokio::time::timeout(
                    std::time::Duration::from_millis(500), listener.accept()
                ).await.is_ok();
                (request, followed)
            }).await.expect("mock import connection exceeded five seconds")
        });
        (endpoint, task)
    }

    #[cfg(unix)]
    async fn finish_raw_import_api(
        mut task: tokio::task::JoinHandle<(Vec<u8>, bool)>,
    ) -> (Vec<u8>, bool) {
        match tokio::time::timeout(std::time::Duration::from_secs(5), &mut task).await {
            Ok(result) => result.expect("mock import server failed"),
            Err(_) => {
                task.abort();
                let _ = task.await;
                panic!("mock import server join exceeded five seconds");
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn publication_file_import_accepts_the_provider_cid_and_pinned_request() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("public artifact with spaces");
        std::fs::write(&file, b"public fixture").unwrap();
        let cid = import_fixture_raw_cid(b"public fixture");
        let provider = import_provider_fixture(
            temp.path(),
            serde_json::json!({"status":"ok","data":{"cid":cid}}),
            0,
        );
        let imported = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            super::import_publication_file(&provider, &file),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(imported, cid);
        super::require_import_cid(&imported, &cid).unwrap();
        assert_eq!(
            import_fixture_requests(&provider),
            vec![
                serde_json::json!({"op":"init","config":{}}),
                serde_json::json!({"op":"add_path","path":file,"pin":true}),
            ]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn publication_file_import_refuses_provider_failure_and_invalid_cid_receipts() {
        for (receipt, status, expected) in [
            (
                serde_json::json!({"status":"ok","data":{}}),
                7,
                "provider import failed",
            ),
            (
                serde_json::json!({"status":"error","message":"fixture refusal"}),
                0,
                "provider refused import",
            ),
            (
                serde_json::json!({"status":"ok","data":{}}),
                0,
                "receipt lacks a CID",
            ),
            (
                serde_json::json!({"status":"ok","data":{"cid":42}}),
                0,
                "receipt lacks a CID",
            ),
            (
                serde_json::json!({"status":"ok","data":{"cid":"invalid-cid"}}),
                0,
                "receipt has an invalid CID",
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let file = temp.path().join("public artifact");
            std::fs::write(&file, b"public fixture").unwrap();
            let provider = import_provider_fixture(temp.path(), receipt, status);
            let error = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                super::import_publication_file(&provider, &file),
            )
            .await
            .unwrap()
            .unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn raw_publication_import_refuses_provider_failure_and_remote_endpoint() {
        for (receipt, expected) in [
            (
                serde_json::json!({"status":"error"}),
                "provider refused import",
            ),
            (
                serde_json::json!({"status":"ok","data":{}}),
                "API endpoint missing",
            ),
            (
                serde_json::json!({"status":"ok","data":{"api_endpoint":"http://192.0.2.1:5001"}}),
                "non-loopback",
            ),
            (
                serde_json::json!({"status":"ok","data":{"api_endpoint":"https://example.invalid"}}),
                "non-loopback",
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let provider = import_provider_fixture(temp.path(), receipt, 0);
            let error = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                super::import_publication_raw_block(
                    &provider,
                    "model-catalog.json",
                    b"public catalog".to_vec(),
                ),
            )
            .await
            .unwrap()
            .unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
            assert_eq!(
                import_fixture_requests(&provider),
                vec![
                    serde_json::json!({"op":"init","config":{}}),
                    serde_json::json!({"op":"status"})
                ]
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn raw_publication_catalog_import_posts_exact_bytes_and_raw_cid_parameters() {
        let catalog = br#"{"payload":{"schema":"elastos.model.catalog/v1","entries":[]}}"#;
        let expected = import_fixture_raw_cid(catalog);
        let (endpoint, server) =
            raw_import_api_fixture(serde_json::json!({"Key":expected}), false).await;
        let temp = tempfile::tempdir().unwrap();
        let provider = import_provider_fixture(
            temp.path(),
            serde_json::json!({"status":"ok","data":{"api_endpoint":endpoint}}),
            0,
        );
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            super::import_publication_raw_block(&provider, "model-catalog.json", catalog.to_vec()),
        )
        .await
        .unwrap();
        let (request, followed) = finish_raw_import_api(server).await;
        let actual = result.unwrap();
        super::require_import_cid(&actual, &expected).unwrap();
        assert!(!followed);
        let text = std::str::from_utf8(&request).unwrap();
        let mut first_line = text.lines().next().unwrap().split_whitespace();
        assert_eq!(first_line.next(), Some("POST"));
        let target =
            url::Url::parse(&format!("http://fixture{}", first_line.next().unwrap())).unwrap();
        assert_eq!(target.path(), "/api/v0/block/put");
        let parameters: BTreeMap<_, _> = target
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        assert_eq!(
            parameters,
            BTreeMap::from([
                ("cid-codec".to_owned(), "raw".to_owned()),
                ("mhtype".to_owned(), "sha2-256".to_owned()),
                ("pin".to_owned(), "true".to_owned()),
            ])
        );
        assert!(text
            .to_ascii_lowercase()
            .contains("multipart/form-data; boundary="));
        assert!(text
            .to_ascii_lowercase()
            .contains("name=\"file\"; filename=\"model-catalog.json\""));
        let (headers, _) = text.split_once("\r\n\r\n").unwrap();
        let content_type = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-type").then_some(value)
            })
            .unwrap();
        let boundary = content_type
            .split_once("boundary=")
            .unwrap()
            .1
            .trim()
            .trim_matches('"');
        let multipart = &request[headers.len() + 4..];
        let file_start = multipart
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .unwrap()
            + 4;
        let trailer = format!("\r\n--{boundary}--\r\n");
        let uploaded = multipart[file_start..]
            .strip_suffix(trailer.as_bytes())
            .unwrap();
        assert_eq!(uploaded, &catalog[..]);
        assert_eq!(
            import_fixture_requests(&provider)[1],
            serde_json::json!({"op":"status"})
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn raw_publication_import_refuses_missing_and_changed_cid_receipts() {
        let bytes = b"public catalog";
        let expected = import_fixture_raw_cid(bytes);
        for receipt in [
            serde_json::json!({}),
            serde_json::json!({"Key":"invalid-cid"}),
            serde_json::json!({"Key":import_fixture_raw_cid(b"other bytes")}),
        ] {
            let (endpoint, server) = raw_import_api_fixture(receipt.clone(), false).await;
            let temp = tempfile::tempdir().unwrap();
            let provider = import_provider_fixture(
                temp.path(),
                serde_json::json!({"status":"ok","data":{"api_endpoint":endpoint}}),
                0,
            );
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                super::import_publication_raw_block(
                    &provider,
                    "model-catalog.json",
                    bytes.to_vec(),
                ),
            )
            .await
            .unwrap();
            finish_raw_import_api(server).await;
            if receipt.get("Key").is_none() {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("receipt lacks its CID"));
            } else {
                assert!(super::require_import_cid(&result.unwrap(), &expected).is_err());
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn raw_publication_import_keeps_redirects_on_the_first_loopback_endpoint() {
        let (endpoint, server) =
            raw_import_api_fixture(serde_json::json!({"redirect":"required"}), true).await;
        let temp = tempfile::tempdir().unwrap();
        let provider = import_provider_fixture(
            temp.path(),
            serde_json::json!({"status":"ok","data":{"api_endpoint":endpoint}}),
            0,
        );
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            super::import_publication_raw_block(
                &provider,
                "model-catalog.json",
                b"public catalog".to_vec(),
            ),
        )
        .await
        .unwrap();
        let (_, followed) = finish_raw_import_api(server).await;
        assert!(result.is_err());
        assert!(!followed, "raw import followed a loopback redirect");
    }

    #[test]
    fn saved_public_pin_changes_require_approval_and_exact_confirmation() {
        let old = "did:key:z6MktwupdmLXVVqTzCw4i46r4uGyosGXRnR3XjN4Zq7oMMsw";
        let new = "did:key:z6MkgwHd2BCWe1jHMXPiR6H1q1RFPcv1YzhMbK5G1kBarbfe";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("publish-state.json");
        let state = PublishState {
            publisher_did: Some(old.to_owned()),
            ..Default::default()
        };
        save_publish_state(&path, &state).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(super::publication_pin_change(&state, new, false).is_err());
        assert!(super::publication_pin_change(&state, "did:key:invalid", true).is_err());
        assert!(!super::publication_pin_change(&state, old, false).unwrap());
        assert!(super::publication_pin_change(&state, new, true).unwrap());
        for answer in [
            String::new(),
            "\n".to_owned(),
            format!("{old}\n"),
            format!(" {new}\n"),
            "x".repeat(1024),
        ] {
            assert!(
                super::confirm_publication_pin(new, &mut std::io::Cursor::new(answer)).is_err()
            );
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        super::confirm_publication_pin(new, &mut std::io::Cursor::new(format!("{new}\n"))).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let legacy = PublishState {
            last_release_cid: Some("existing-release".to_owned()),
            ..Default::default()
        };
        assert!(super::publication_pin_change(&legacy, new, false).is_err());
        assert!(super::publication_pin_change(&legacy, new, true).unwrap());
        assert!(!super::publication_pin_change(&PublishState::default(), new, false).unwrap());
    }

    #[test]
    fn publication_receipt_refuses_changed_cids_and_broken_previous_links() {
        let state = PublishState {
            last_release_cid: Some("previous-release".to_owned()),
            last_head_cid: Some("previous-head".to_owned()),
            ..Default::default()
        };
        let head =
            serde_json::to_vec(&serde_json::json!({"payload":{"prev_head_cid":"previous-head"}}))
                .unwrap();
        let release = serde_json::to_vec(
            &serde_json::json!({"payload":{"prev_release_cid":"previous-release"}}),
        )
        .unwrap();
        super::validate_publication_chain(&state, &head, &release, "next-release").unwrap();
        assert!(super::validate_publication_chain(
            &state,
            br#"{"payload":{"prev_head_cid":null}}"#,
            &release,
            "next-release"
        )
        .is_err());
        assert!(super::validate_publication_chain(
            &state,
            &head,
            br#"{"payload":{"prev_release_cid":null}}"#,
            "next-release"
        )
        .is_err());
        assert!(super::require_import_cid("changed-cid", "signed-cid").is_err());
        super::require_import_cid("signed-cid", "signed-cid").unwrap();
        assert!(
            super::validate_publication_chain(&state, &head, &release, "previous-release").is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn publisher_pin_receipt_refuses_links_and_oversized_files() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real-state.json");
        save_publish_state(&real, &PublishState::default()).unwrap();
        let link = dir.path().join("publish-state.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(load_publish_state(&link).is_err());
        std::fs::remove_file(link).unwrap();
        std::fs::write(&real, vec![b' '; 65537]).unwrap();
        assert!(load_publish_state(&real).is_err());
        assert!(load_publish_state(dir.path()).is_err());
    }

    #[test]
    fn test_publish_state_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("publish-state.json");
        let state = PublishState {
            publisher_did: Some("did:key:fixture".to_string()),
            last_release_cid: Some("release-cid".to_string()),
            last_head_cid: Some("head-cid".to_string()),
            last_version: Some("0.11.0".to_string()),
            last_published_at: Some(42),
        };
        save_publish_state(&path, &state).unwrap();
        assert_eq!(load_publish_state(&path).unwrap(), state);
    }

    #[test]
    fn test_release_discovery_topics_include_scoped_and_global_topics() {
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
        assert!(capsules.iter().any(|name| name == "chat-room"));
        assert!(capsules.iter().any(|name| name == "home-cli"));
        assert!(capsules.iter().any(|name| name == "ipfs-provider"));
    }

    #[test]
    fn test_publish_rejects_invalid_and_placeholder_manifests() {
        let path = Path::new("capsule.json");
        let mut manifest = test_manifest("sample", &[]);
        manifest.description = Some("Useful sample".to_string());
        manifest.author = Some("publisher".to_string());
        validate_publishable_manifest(path, &manifest).unwrap();

        manifest.schema = "wrong".to_string();
        assert!(validate_publishable_manifest(path, &manifest)
            .unwrap_err()
            .to_string()
            .contains("Invalid capsule manifest"));

        manifest.schema = elastos_common::SCHEMA_V1.to_string();
        manifest.author = Some("local-development".to_string());
        assert!(validate_publishable_manifest(path, &manifest)
            .unwrap_err()
            .to_string()
            .contains("placeholder author"));

        manifest.author = None;
        assert!(validate_publishable_manifest(path, &manifest)
            .unwrap_err()
            .to_string()
            .contains("must declare an author"));

        manifest.author = Some("publisher".to_string());
        manifest.description = None;
        assert!(validate_publishable_manifest(path, &manifest)
            .unwrap_err()
            .to_string()
            .contains("must declare a description"));
    }

    #[test]
    fn test_changed_capsules_reports_new_updated_and_removed_entries() {
        let previous = ReleaseLedgerPlatform {
            binary_cid: "old-binary".to_string(),
            components_cid: "old-components".to_string(),
            capsules: BTreeMap::from([
                ("chat".to_string(), "cid-chat-1".to_string()),
                ("removed-provider".to_string(), "cid-removed-1".to_string()),
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
                "removed-provider".to_string()
            ]
        );
    }

    #[test]
    fn test_build_release_ledger_entry_keeps_three_platforms_distinct() {
        let temp = tempfile::tempdir().unwrap();
        let artifacts_dir = temp.path();
        let platforms = ["x86_64-linux", "aarch64-linux", "aarch64-darwin"];
        let descriptors = platforms
            .iter()
            .map(|platform| {
                (
                    *platform,
                    serde_json::json!({
                        "binary": { "cid": format!("binary-{platform}") },
                        "components": { "cid": format!("components-{platform}") }
                    }),
                )
            })
            .collect::<BTreeMap<_, _>>();
        std::fs::write(
            artifacts_dir.join("release.json"),
            serde_json::json!({
                "payload": {
                    "channel": "stable",
                    "version": "0.11.0",
                    "released_at": 42,
                    "platforms": descriptors
                },
                "signer_did": "did:key:z6Mktest"
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            artifacts_dir.join("release-head.json"),
            serde_json::json!({ "payload": { "latest_release_cid": "release-cid" } }).to_string(),
        )
        .unwrap();
        for platform in platforms {
            std::fs::write(
                artifacts_dir.join(format!("components-{platform}.json")),
                serde_json::json!({
                    "external": {},
                    "profiles": {},
                    "capsules": {
                        "home": { "cid": format!("home-{platform}"), "sha256": "a", "size": 1, "platforms": [platform] }
                    }
                })
                .to_string(),
            )
            .unwrap();
        }
        // Old architecture-only files can remain from a previous publication.
        // They must never substitute for either ARM platform's actual manifest.
        std::fs::copy(
            artifacts_dir.join("components-x86_64-linux.json"),
            artifacts_dir.join("components-x86_64.json"),
        )
        .unwrap();
        std::fs::copy(
            artifacts_dir.join("components-aarch64-linux.json"),
            artifacts_dir.join("components-aarch64.json"),
        )
        .unwrap();

        let read = || {
            build_release_ledger_entry(
                artifacts_dir,
                "release-cid",
                "head-cid",
                &["home".to_string()],
            )
        };
        let entry = read().unwrap();
        assert_eq!(entry.version, "0.11.0");
        assert_eq!(entry.channel, "stable");
        assert_eq!(entry.release_cid, "release-cid");
        assert_eq!(entry.head_cid, "head-cid");
        assert_eq!(entry.signer_did, "did:key:z6Mktest");
        assert_eq!(entry.platforms.len(), 3);
        for platform in platforms {
            let record = &entry.platforms[platform];
            assert_eq!(record.binary_cid, format!("binary-{platform}"));
            assert_eq!(record.components_cid, format!("components-{platform}"));
            assert_eq!(record.capsules["home"], format!("home-{platform}"));
        }
        std::fs::remove_file(artifacts_dir.join("components-aarch64-darwin.json")).unwrap();
        assert!(read()
            .unwrap_err()
            .to_string()
            .contains("components-aarch64-darwin.json"));
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
            platform_inputs: Vec::new(),
            preview_platform: None,
            prepare_only: None,
            signed_publication: None,
            allow_signer_rotation: false,
            publisher_did: None,
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
            platform_inputs: Vec::new(),
            preview_platform: None,
            prepare_only: None,
            signed_publication: None,
            allow_signer_rotation: false,
            publisher_did: None,
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

        let selected = super::REQUIRED_SUPPORTED_PUBLISH_CAPSULES
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<_>>();
        let target_dir = super::cargo_target_dir(temp.path());
        let gnu = target_dir.join("aarch64-unknown-linux-gnu/release/elastos");
        std::fs::create_dir_all(gnu.parent().unwrap()).unwrap();
        std::fs::write(&gnu, b"stale gnu build").unwrap();
        let err = validate_publish_inputs(&options, temp.path(), &selected).unwrap_err();
        assert!(err.to_string().contains("aarch64-unknown-linux-musl"));

        let musl = target_dir.join("aarch64-unknown-linux-musl/release/elastos");
        std::fs::create_dir_all(musl.parent().unwrap()).unwrap();
        std::fs::write(&musl, b"publisher musl build").unwrap();
        validate_publish_inputs(&options, temp.path(), &selected).unwrap();
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
            platform_inputs: Vec::new(),
            preview_platform: None,
            prepare_only: None,
            signed_publication: None,
            allow_signer_rotation: false,
            publisher_did: None,
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
            platform_inputs: Vec::new(),
            preview_platform: None,
            prepare_only: None,
            signed_publication: None,
            allow_signer_rotation: false,
            publisher_did: None,
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
}
