//! Update/upgrade logic: Carrier-first release fetch, explicit transport
//! overrides, platform detection, cache management, and the main update flow.
//!
//! CLI and operator entry points share signed head discovery and admission.

use anyhow::Context as _;
use std::cmp::Ordering;
use std::future::Future;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::pin::Pin;

#[cfg(test)]
use elastos_common::localhost::{
    installation_release_head_path, installation_release_manifest_path,
};

use crate::crypto::{verify_release_envelope, verify_release_envelope_against_dids};
use crate::install_transaction::{InstallTransaction, ReleaseFile, RestartPhase};
use crate::install_transaction::{RestartPlan, RestartRecord};
use crate::sources::{
    default_data_dir, default_install_path, load_trusted_sources, normalize_gateways, TrustedSource,
};

/// Async callback for fetching a CID or `release-path:<path>` from the trusted source.
/// The caller decides whether any explicit transport override is allowed.
pub type FetchFn = Box<
    dyn Fn(String, Vec<String>) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<u8>>> + Send>>
        + Send
        + Sync,
>;

/// Async callback for attempting P2P release discovery.
/// An unavailable connection returns None; discovery errors remain terminal.
pub type TryP2pFn = Box<
    dyn Fn(
            TrustedSource,
            String,
        ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<DiscoveredHead>>> + Send>>
        + Send
        + Sync,
>;

/// Verified discovery bytes, with a head CID only when the publisher supplied one.
#[derive(Clone, Debug)]
pub struct DiscoveredHead {
    pub head_cid: Option<String>,
    pub bytes: Vec<u8>,
}

pub async fn discover_carrier_release_head(
    client: &crate::carrier::CarrierClient,
    source: &TrustedSource,
) -> anyhow::Result<DiscoveredHead> {
    let announcement = client
        .release_head()
        .await
        .context(UpdateSourceUnavailable)?
        .ok_or_else(|| anyhow::anyhow!("Trusted source returned no release head"))?;
    resolve_discovered_head(&announcement, source, |cid| async move {
        match cid {
            Some(cid) => client
                .fetch_content(&cid, None)
                .await
                .context(UpdateSourceUnavailable),
            None => client
                .fetch_file("release-head.json")
                .await
                .context(UpdateSourceUnavailable),
        }
    })
    .await
}

async fn resolve_discovered_head<F, Fut>(
    announcement: &serde_json::Value,
    source: &TrustedSource,
    fetch: F,
) -> anyhow::Result<DiscoveredHead>
where
    F: FnOnce(Option<String>) -> Fut,
    Fut: Future<Output = anyhow::Result<Vec<u8>>>,
{
    let field = |name: &str| -> anyhow::Result<Option<&str>> {
        match announcement.get(name) {
            None => Ok(None),
            Some(serde_json::Value::String(value)) => {
                Ok((!value.is_empty()).then_some(value.as_str()))
            }
            Some(_) => anyhow::bail!("Invalid release announcement {name}"),
        }
    };
    let head_cid = field("head_cid")?;
    let release_cid = field("release_cid")?;
    // Older publishers put the release CID in head_cid. Missing receipt fields
    // also require the signed filename discovery path; explicit CIDs bypass it.
    let distinct_head = head_cid.is_some() && release_cid.is_some() && head_cid != release_cid;
    let bytes = fetch(if distinct_head {
        head_cid.map(str::to_owned)
    } else {
        None
    })
    .await?;
    let publisher = source
        .publisher_dids
        .first()
        .ok_or_else(|| anyhow::anyhow!("Trusted source has no publisher DID"))?;
    let head = verify_release_envelope(&bytes, "elastos.release.head.v1", publisher)
        .map_err(new_publisher_key_hint)?;
    anyhow::ensure!(
        head["payload"]["schema"] == "elastos.release.head/v1",
        "Unsupported release head schema"
    );
    verify_source_channel(source, head["payload"]["channel"].as_str().unwrap_or(""))?;
    let signed_release = head["payload"]["latest_release_cid"]
        .as_str()
        .filter(|cid| !cid.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Release head has no latest_release_cid"))?;
    if let Some(release) = release_cid {
        anyhow::ensure!(
            release == signed_release,
            "Release announcement differs from signed head"
        );
    }
    let head_cid = match head_cid {
        Some(cid) if !distinct_head && cid == signed_release => None,
        Some(cid) => {
            verify_release_metadata_cid(cid, &bytes)?;
            Some(cid.to_owned())
        }
        None => None,
    };
    Ok(DiscoveredHead { head_cid, bytes })
}

#[derive(Debug)]
pub struct InvalidInstalledVersion;

impl std::fmt::Display for InvalidInstalledVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Installed release version is invalid. Run `elastos update --force` locally to install a verified signed release and repair it.")
    }
}
impl std::error::Error for InvalidInstalledVersion {}

pub(crate) fn installed_release_version(
    installed: &str,
) -> anyhow::Result<Option<semver::Version>> {
    if installed.is_empty() {
        return Ok(None);
    }
    semver::Version::parse(installed)
        .map(Some)
        .map_err(|_| InvalidInstalledVersion.into())
}

/// An empty installed version is the legacy first-install state. Every
/// nonempty version must have exact SemVer syntax before it is compared.
pub fn compare_release_versions(installed: &str, offered: &str) -> anyhow::Result<Ordering> {
    let offered = semver::Version::parse(offered)
        .map_err(|err| anyhow::anyhow!("Invalid signed release version '{offered}': {err}"))?;
    let Some(installed) = installed_release_version(installed)? else {
        return Ok(Ordering::Greater);
    };
    Ok(offered.cmp_precedence(&installed))
}

pub fn verify_source_channel(source: &TrustedSource, signed_channel: &str) -> anyhow::Result<()> {
    let subscribed = if source.channel.trim().is_empty() {
        "stable"
    } else {
        source.channel.trim()
    };
    anyhow::ensure!(
        subscribed == signed_channel,
        "Signed release channel '{signed_channel}' differs from trusted source '{}' subscription '{subscribed}'",
        source.name
    );
    Ok(())
}

/// Check metadata fetched by CID against the returned bytes. Published release
/// envelopes fit in one IPFS block; Kubo may encode that block as raw or as a
/// UnixFS File in dag-pb (including the CIDv0 form used by older publishers).
pub fn verify_release_metadata_cid(cid_text: &str, bytes: &[u8]) -> anyhow::Result<()> {
    use sha2::Digest;

    let cid = cid::Cid::try_from(cid_text)
        .map_err(|err| anyhow::anyhow!("Invalid release metadata CID {cid_text}: {err}"))?;
    anyhow::ensure!(
        cid.hash().code() == 0x12 && cid.hash().digest().len() == 32,
        "Unsupported release metadata CID hash: {cid_text}"
    );
    let encoded = match cid.codec() {
        0x55 => bytes.to_vec(), // raw
        0x70 => {
            anyhow::ensure!(
                !bytes.is_empty() && bytes.len() <= 256 * 1024,
                "Release metadata is outside the supported single-block UnixFS size"
            );
            let mut unixfs = vec![0x08, 0x02, 0x12]; // Type=File, Data
            append_varint(&mut unixfs, bytes.len());
            unixfs.extend_from_slice(bytes);
            unixfs.push(0x18); // filesize
            append_varint(&mut unixfs, bytes.len());
            let mut node = vec![0x0a]; // PBNode.Data
            append_varint(&mut node, unixfs.len());
            node.extend_from_slice(&unixfs);
            node
        }
        codec => anyhow::bail!("Unsupported release metadata CID codec {codec}: {cid_text}"),
    };
    let digest = sha2::Sha256::digest(&encoded);
    anyhow::ensure!(
        digest.as_slice() == cid.hash().digest(),
        "Release metadata bytes do not match requested CID {cid_text}"
    );
    Ok(())
}

fn append_varint(target: &mut Vec<u8>, mut value: usize) {
    while value >= 0x80 {
        target.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    target.push(value as u8);
}

pub fn ordered_update_gateways(source_gateways: &[String]) -> Vec<String> {
    let mut gateways = Vec::new();
    for gateway in source_gateways {
        let gateway = gateway.trim_end_matches('/').to_string();
        if !gateway.is_empty() && !gateways.iter().any(|g| g == &gateway) {
            gateways.push(gateway);
        }
    }
    gateways
}

/// Check if a gateway URL is a public IPFS content gateway (serves /ipfs/<cid> only).
/// These don't serve `/release-head.json`.
pub fn is_ipfs_content_gateway(url: &str) -> bool {
    let url_lower = url.to_lowercase();
    [
        "ipfs.io",
        "dweb.link",
        "w3s.link",
        "cloudflare-ipfs.com",
        "gateway.pinata.cloud",
        "nftstorage.link",
    ]
    .iter()
    .any(|host| url_lower.contains(host))
}

/// Try to fetch `release-head.json` directly from an explicitly granted
/// transport override URL. Skips IPFS content gateways (they only serve
/// `/ipfs/<cid>` paths). Returns `(release_cid, head_bytes, working_url)`.
pub async fn try_gateway_head_discovery(
    gateway_urls: &[String],
    publisher_did: &str,
) -> Option<(String, Vec<u8>, String)> {
    let publisher_gateways: Vec<&String> = gateway_urls
        .iter()
        .filter(|gw| !is_ipfs_content_gateway(gw))
        .collect();

    if publisher_gateways.is_empty() {
        return None;
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .ok()?;

    for gw in publisher_gateways {
        let url = format!("{}/release-head.json", gw.trim_end_matches('/'));
        tracing::debug!("update: trying explicit transport override: {}", gw);
        let resp = match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                eprintln!("  Explicit transport override {}: HTTP {}", gw, r.status());
                continue;
            }
            Err(e) => {
                eprintln!("  Explicit transport override {}: {}", gw, e);
                continue;
            }
        };
        let bytes = match resp.bytes().await {
            Ok(b) => b.to_vec(),
            Err(e) => {
                eprintln!(
                    "  Explicit transport override {}: failed to read body: {}",
                    gw, e
                );
                continue;
            }
        };
        match crate::crypto::verify_release_envelope(
            &bytes,
            "elastos.release.head.v1",
            publisher_did,
        ) {
            Ok(head) => {
                let version = head["payload"]["version"].as_str().unwrap_or("unknown");
                let release_cid = head["payload"]["latest_release_cid"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                println!("  Found via explicit transport override: v{}", version);
                if release_cid.is_empty() {
                    eprintln!("  Gateway HEAD has no latest_release_cid, skipping");
                    continue;
                }
                return Some((release_cid, bytes, gw.clone()));
            }
            Err(e) => {
                eprintln!(
                    "  Explicit transport override {}: verification failed: {}",
                    gw, e
                );
            }
        }
    }
    None
}

/// Fetch a raw CID payload through the ordered gateway list using `/ipfs/<cid>`.
pub async fn fetch_cid_via_gateways(cid: &str, gateway_urls: &[String]) -> anyhow::Result<Vec<u8>> {
    if gateway_urls.is_empty() {
        anyhow::bail!("no gateway URLs configured for CID fetch");
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let mut failures = Vec::new();
    for gw in gateway_urls {
        let gateway = gw.trim_end_matches('/');
        if gateway.is_empty() {
            continue;
        }
        let url = format!("{}/ipfs/{}", gateway, cid);
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let bytes = resp.bytes().await.map_err(|e| {
                    anyhow::anyhow!("{}: failed to read response body: {}", gateway, e)
                })?;
                return Ok(bytes.to_vec());
            }
            Ok(resp) => failures.push(format!("{} -> HTTP {}", gateway, resp.status())),
            Err(err) => failures.push(format!("{} -> {}", gateway, err)),
        }
    }

    anyhow::bail!(
        "failed to fetch CID {} from configured gateways: {}",
        cid,
        failures.join("; ")
    )
}

/// Fetch the signed `release.json` envelope directly from a publisher-style
/// gateway edge. This is the correct follow-on after successful
/// `release-head.json` discovery via that same gateway.
pub async fn fetch_release_manifest_via_gateway(gateway_url: &str) -> anyhow::Result<Vec<u8>> {
    let gateway = gateway_url.trim_end_matches('/');
    if gateway.is_empty() {
        anyhow::bail!("gateway URL is empty");
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let url = format!("{}/release.json", gateway);
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("{} -> {}", gateway, e))?;

    if !resp.status().is_success() {
        anyhow::bail!("{} -> HTTP {}", gateway, resp.status());
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| anyhow::anyhow!("{}: failed to read response body: {}", gateway, e))?;

    Ok(bytes.to_vec())
}

pub fn format_bytes(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

/// Detect the current platform for release binary selection.
pub fn detect_release_platform() -> &'static str {
    release_platform_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn release_platform_for(os: &str, arch: &str) -> &'static str {
    match (os, arch) {
        ("linux", "x86_64") => "x86_64-linux",
        ("linux", "aarch64") => "aarch64-linux",
        ("macos", "x86_64") => "x86_64-darwin",
        ("macos", "aarch64") => "aarch64-darwin",
        _ => "unknown",
    }
}

pub fn changed_capsule_names(
    old_components: Option<&[u8]>,
    new_components: &[u8],
) -> anyhow::Result<Vec<String>> {
    let new_manifest: crate::setup::ComponentsManifest = serde_json::from_slice(new_components)?;
    let Some(old_bytes) = old_components else {
        return Ok(Vec::new());
    };
    let old_manifest: crate::setup::ComponentsManifest = serde_json::from_slice(old_bytes)?;

    let mut changed = Vec::new();
    for (name, new_entry) in &new_manifest.capsules {
        match old_manifest.capsules.get(name) {
            Some(old_entry) if old_entry.cid == new_entry.cid => {}
            _ => changed.push(name.clone()),
        }
    }
    for name in old_manifest.capsules.keys() {
        if !new_manifest.capsules.contains_key(name) {
            changed.push(name.clone());
        }
    }
    changed.sort();
    changed.dedup();
    Ok(changed)
}

pub fn evict_changed_capsule_cache(data_dir: &std::path::Path, changed_capsules: &[String]) -> u32 {
    if changed_capsules.is_empty() {
        return 0;
    }

    let capsules_dir = data_dir.join("capsules");
    let mut cleared = 0u32;
    for name in changed_capsules {
        let capsule_dir = capsules_dir.join(name);
        if capsule_dir.is_dir() && std::fs::remove_dir_all(&capsule_dir).is_ok() {
            cleared += 1;
        }
    }
    cleared
}

/// The owned probe lives beside the installed binary and closes before migration.
async fn verify_candidate_before_migration(
    transaction: &InstallTransaction,
    bytes: &[u8],
    version: &str,
) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let parent = transaction
        .binary_path()
        .parent()
        .context("installed Runtime parent missing")?;
    crate::install_transaction::require_controller_disk_reserve(
        parent,
        bytes.len() as u64 + 64 * 1024,
    )?;
    let mut probe = tempfile::Builder::new()
        .prefix(".elastos.candidate-check-")
        .tempfile_in(parent)?;
    probe
        .as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o700))?;
    probe.write_all(bytes)?;
    probe.as_file().sync_all()?;
    // Linux requires the writer descriptor to close before executing these bytes.
    let path = probe.into_temp_path();
    verify_installed_binary_version(&path, version).await
}

async fn verify_installed_binary_version(
    bin_path: &Path,
    expected_version: &str,
) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;
    use tokio::io::AsyncReadExt;

    let mut command = tokio::process::Command::new(bin_path);
    command
        .arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    command.as_std_mut().process_group(0);
    let mut child = command.spawn().map_err(|error| {
        anyhow::anyhow!(
            "failed to run installed binary {}: {error}",
            bin_path.display()
        )
    })?;
    let pid = child.id().expect("spawned version probe has a PID");
    let mut stdout = child.stdout.take().unwrap().take(4097);
    let mut stderr = child.stderr.take().unwrap().take(4097);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        // Keep the leader unreaped until pipe admission has finished. Its PID
        // then anchors the owned process group throughout any timeout cleanup.
        tokio::try_join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err))?;
        if out.len() > 4096 || err.len() > 4096 {
            return Err(std::io::Error::other(
                "installed binary version check exceeded its output limit",
            ));
        }
        child.wait().await
    })
    .await;
    let status = match result {
        Ok(Ok(status)) => status,
        failure => {
            // The probe owns this process group, including children that retain its pipes.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            let _ = child.kill().await;
            let _ = child.wait().await;
            match failure {
                Err(_) => anyhow::bail!("installed binary version check timed out after 5 seconds"),
                Ok(Err(error)) => return Err(error.into()),
                _ => unreachable!(),
            }
        }
    };
    verify_installed_binary_version_output(bin_path, expected_version, status.success(), &out, &err)
}

fn verify_installed_binary_version_output(
    bin_path: &Path,
    expected_version: &str,
    success: bool,
    stdout: &[u8],
    stderr: &[u8],
) -> anyhow::Result<()> {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let combined = format!("{}{}", stdout, stderr).trim().to_string();

    if !success {
        anyhow::bail!(
            "Installed binary version check failed at {}\n  Output: {}",
            bin_path.display(),
            if combined.is_empty() {
                "<no output>"
            } else {
                &combined
            }
        );
    }

    if stdout.trim() != format!("elastos {expected_version}") || !stderr.is_empty() {
        anyhow::bail!(
            "Installed binary version mismatch at {}\n  Expected: {}\n  Got:      {}",
            bin_path.display(),
            expected_version,
            if combined.is_empty() {
                "<no output>"
            } else {
                &combined
            }
        );
    }

    Ok(())
}

/// Restore an interrupted offline installation before opening release transport.
/// Check-only callers retain their read-only behavior by omitting this hook.
pub fn recover_pending_installation(data_dir: &Path) -> anyhow::Result<()> {
    let sources = load_trusted_sources(data_dir)?;
    let Some(source) = sources.default_source() else {
        return Ok(());
    };
    let binary = if source.install_path.is_empty() {
        default_install_path()
    } else {
        PathBuf::from(&source.install_path)
    };
    if InstallTransaction::has_pending_recovery(&binary) {
        let transaction = InstallTransaction::acquire(data_dir, &binary)?;
        let _offline = crate::host_lock::acquire_host_process_lock(
            transaction.data_dir(),
            "update-recovery",
            "offline",
        )?;
        transaction.recover()?;
    }
    Ok(())
}

/// A new publisher key is trusted only through an explicit install.sh run.
fn new_publisher_key_hint(error: anyhow::Error) -> anyhow::Error {
    if error.to_string().starts_with("Signer DID mismatch") {
        anyhow::anyhow!("{error}\n  This release is signed by a new publisher key. Run the publisher's install.sh to trust it; this installation was not changed.")
    } else {
        error
    }
}

/// install.sh hands its verified candidate to the shared installation writer.
/// `check` restores an interrupted install and admits the release before the
/// installer stops this Home. The installer leaves components.json to setup.
pub fn install_release(
    data_dir: &Path,
    binary: &Path,
    candidate: [&Path; 4],
    check: bool,
) -> anyhow::Result<()> {
    let [executable, sources, head, release] = candidate.map(std::fs::read);
    let (executable, sources, head, release) = (executable?, sources?, head?, release?);
    let config: crate::sources::TrustedSourcesConfig = serde_json::from_slice(&sources)?;
    let source = config
        .default_source()
        .context("installer trusted source is missing")?;
    crate::installed_release::admit_candidate(binary, source, &executable, &head, &release)?;
    let mut transaction = InstallTransaction::acquire(data_dir, binary)?;
    if transaction.restart_record_if_any()?.is_some()
        || crate::update_controller::has_queued_update(transaction.data_dir())?
    {
        anyhow::bail!("A Home update is still in progress. Open Home to let it finish, then run the installer again; this installation was not changed.");
    }
    if InstallTransaction::has_pending_recovery(transaction.binary_path()) {
        // Restore an interrupted install or update before this release is admitted.
        let offline = crate::host_lock::acquire_host_process_lock(
            transaction.data_dir(),
            "update",
            "offline",
        )?;
        transaction.recover()?;
        drop((offline, transaction));
        // Recovery keeps its journal's layout; the next writer selects the current one.
        transaction = InstallTransaction::acquire(data_dir, binary)?;
    }
    let current = load_trusted_sources(transaction.data_dir())
        .ok()
        .and_then(|config| config.default_source().cloned())
        .filter(|current| !current.installed_version.is_empty());
    if let Some(current) = current {
        let installed = Path::new(&current.install_path);
        if let (Some(Ok(parent)), Some(name)) = (
            installed.parent().map(std::fs::canonicalize),
            installed.file_name(),
        ) {
            anyhow::ensure!(
                parent.join(name) == transaction.binary_path(),
                "This Home's Runtime is installed at {}. Run the installer with --install-dir {}; this installation was not changed.",
                installed.display(),
                parent.display()
            );
        }
        anyhow::ensure!(
            compare_release_versions(&current.installed_version, &source.installed_version)?
                != Ordering::Less,
            "Release {} is older than the installed release {}; this installation was not changed.",
            source.installed_version,
            current.installed_version
        );
        verify_source_channel(&current, &source.channel)?;
    } else {
        // Without a release record an existing Runtime could be newer than this one.
        anyhow::ensure!(
            std::fs::symlink_metadata(transaction.binary_path()).is_err(),
            "{} is installed, but this Home's release record (sources.json) is missing or unreadable, so the installer cannot tell whether this release is older. Move that Runtime aside, then run the installer again; this installation was not changed.",
            binary.display()
        );
    }
    if check {
        return Ok(());
    }
    let _offline =
        crate::host_lock::acquire_host_process_lock(transaction.data_dir(), "update", "offline")?;
    transaction.prepare(&[
        (ReleaseFile::RuntimeBinary, executable.as_slice()),
        (ReleaseFile::Sources, sources.as_slice()),
        (ReleaseFile::ReleaseHead, head.as_slice()),
        (ReleaseFile::ReleaseManifest, release.as_slice()),
    ])?;
    transaction.commit_checked(|| Ok(()))
}

/// Main update flow. Discovers the latest release, verifies signatures, and installs.
#[allow(clippy::too_many_arguments)]
pub async fn run_update(
    fetch_fn: &FetchFn,
    try_p2p_fn: Option<&TryP2pFn>,
    check_only: bool,
    head_cid_override: Option<String>,
    no_p2p: bool,
    cli_gateways: Vec<String>,
    version: &str,
    auto_confirm: bool,
    force: bool,
) -> anyhow::Result<()> {
    let data_dir = default_data_dir();
    run_update_for_data_dir(
        &data_dir,
        fetch_fn,
        try_p2p_fn,
        check_only,
        head_cid_override,
        no_p2p,
        cli_gateways,
        version,
        auto_confirm,
        force,
    )
    .await
}

/// Main update flow using an explicit runtime data dir.
#[allow(clippy::too_many_arguments)]
pub async fn run_update_for_data_dir(
    data_dir: &Path,
    fetch_fn: &FetchFn,
    try_p2p_fn: Option<&TryP2pFn>,
    check_only: bool,
    head_cid_override: Option<String>,
    no_p2p: bool,
    cli_gateways: Vec<String>,
    version: &str,
    auto_confirm: bool,
    force: bool,
) -> anyhow::Result<()> {
    run_update_for_data_dir_in_context(
        data_dir,
        fetch_fn,
        try_p2p_fn,
        check_only,
        head_cid_override,
        no_p2p,
        cli_gateways,
        version,
        auto_confirm,
        force,
        crate::setup::FirstPartyCarrierContext::Setup,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_update_for_data_dir_in_context(
    data_dir: &Path,
    fetch_fn: &FetchFn,
    try_p2p_fn: Option<&TryP2pFn>,
    check_only: bool,
    head_cid_override: Option<String>,
    no_p2p: bool,
    cli_gateways: Vec<String>,
    version: &str,
    auto_confirm: bool,
    force: bool,
    carrier_context: crate::setup::FirstPartyCarrierContext,
) -> anyhow::Result<()> {
    run_update_for_data_dir_with_mode(
        data_dir,
        fetch_fn,
        try_p2p_fn,
        check_only,
        head_cid_override,
        no_p2p,
        cli_gateways,
        version,
        auto_confirm,
        force,
        carrier_context,
        ApplyMode::Normal,
    )
    .await
}

/// Internal foundation for a Home restart owner that has stopped Runtime.
/// This uses the normal signed discovery and admission flow, then stages an
/// offline transaction with unchanged support assets. The caller owns restart
/// and installed Home acceptance.
#[allow(clippy::too_many_arguments)]
pub async fn run_frozen_update_for_data_dir(
    data_dir: &Path,
    fetch_fn: &FetchFn,
    try_p2p_fn: Option<&TryP2pFn>,
    check_only: bool,
    head_cid_override: Option<String>,
    no_p2p: bool,
    cli_gateways: Vec<String>,
    version: &str,
    auto_confirm: bool,
    force: bool,
) -> anyhow::Result<()> {
    run_update_for_data_dir_with_mode(
        data_dir,
        fetch_fn,
        try_p2p_fn,
        check_only,
        head_cid_override,
        no_p2p,
        cli_gateways,
        version,
        auto_confirm,
        force,
        crate::setup::FirstPartyCarrierContext::Setup,
        ApplyMode::FrozenOffline,
    )
    .await
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ApplyMode {
    Normal,
    FrozenOffline,
}

#[derive(Debug)]
pub(crate) struct UpdateSourceUnavailable;
impl std::fmt::Display for UpdateSourceUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("update source unavailable")
    }
}
impl std::error::Error for UpdateSourceUnavailable {}

pub(crate) fn download_message(size: Option<u64>) -> String {
    let size = size
        .map(|bytes| format!(" ({})", format_bytes(bytes as usize)))
        .unwrap_or_default();
    format!("Downloading the update{size}. Home restarts when the update is ready.")
}

pub(crate) trait RestartOwner: Send {
    fn progress(&self, phase: &str, message: &str) -> anyhow::Result<()>;

    fn plan(
        &self,
        support_sha256: String,
        previous_version: &str,
        candidate_version: &str,
    ) -> anyhow::Result<RestartPlan>;
    fn start<'a>(
        &'a mut self,
        transaction: &'a InstallTransaction,
        record: RestartRecord,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;
    fn stop<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;
}

pub(crate) async fn run_restarting_update(
    data_dir: &Path,
    fetch_fn: &FetchFn,
    head_cid: String,
    owner: &mut dyn RestartOwner,
) -> anyhow::Result<()> {
    run_update_with_restart(
        data_dir,
        fetch_fn,
        None,
        false,
        Some(head_cid),
        false,
        Vec::new(),
        env!("ELASTOS_VERSION"),
        true,
        false,
        crate::setup::FirstPartyCarrierContext::Setup,
        ApplyMode::FrozenOffline,
        Some(owner),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_update_for_data_dir_with_mode(
    data_dir: &Path,
    fetch_fn: &FetchFn,
    try_p2p_fn: Option<&TryP2pFn>,
    check_only: bool,
    head_cid_override: Option<String>,
    no_p2p: bool,
    cli_gateways: Vec<String>,
    version: &str,
    auto_confirm: bool,
    force: bool,
    carrier_context: crate::setup::FirstPartyCarrierContext,
    apply_mode: ApplyMode,
) -> anyhow::Result<()> {
    run_update_with_restart(
        data_dir,
        fetch_fn,
        try_p2p_fn,
        check_only,
        head_cid_override,
        no_p2p,
        cli_gateways,
        version,
        auto_confirm,
        force,
        carrier_context,
        apply_mode,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_update_with_restart(
    data_dir: &Path,
    fetch_fn: &FetchFn,
    try_p2p_fn: Option<&TryP2pFn>,
    check_only: bool,
    head_cid_override: Option<String>,
    no_p2p: bool,
    cli_gateways: Vec<String>,
    version: &str,
    auto_confirm: bool,
    force: bool,
    carrier_context: crate::setup::FirstPartyCarrierContext,
    apply_mode: ApplyMode,
    restart_owner: Option<&mut dyn RestartOwner>,
) -> anyhow::Result<()> {
    if !check_only {
        recover_pending_installation(data_dir)?;
    }
    let sources = load_trusted_sources(data_dir)?;
    let source = sources.default_source().cloned().ok_or_else(|| {
        anyhow::anyhow!("No trusted source configured. Run `elastos source add ...` first.")
    })?;
    let primary_publisher =
        source.publisher_dids.first().cloned().ok_or_else(|| {
            anyhow::anyhow!("Trusted source '{}' has no publisher DID", source.name)
        })?;
    // Explicit transport override only. Default update path is Carrier-first.
    let ordered_gateways = ordered_update_gateways(&cli_gateways);

    let current_version = source.installed_version.clone();
    if !force {
        installed_release_version(&current_version)?;
    }

    println!("ElastOS Update v{}", version);
    let installed_display = if current_version.is_empty() {
        "unknown"
    } else {
        &current_version
    };
    println!("  Installed release: {}", installed_display);
    if !current_version.is_empty() && current_version != version {
        println!(
            "  Running binary:    {} (differs from installed release)",
            version
        );
    }
    println!();

    // 2. Resolve release head
    let mut resolved_head_cid: Option<String> = None;
    let mut discovered_head_bytes: Option<Vec<u8>> = None;
    let mut working_gateway: Option<String> = None;
    let discovery_method = if let Some(ref cid) = head_cid_override {
        if force {
            println!("  Rollback to HEAD CID: {}", cid);
        } else {
            println!("  Using provided HEAD CID: {}", cid);
        }
        resolved_head_cid = Some(cid.clone());
        if force {
            "rollback (--rollback-to)"
        } else {
            "manual (--head-cid)"
        }
    } else {
        // Step A: Try P2P discovery (unless --no-p2p)
        let mut resolved: Option<&str> = None;
        if !no_p2p {
            if let Some(p2p_fn) = try_p2p_fn {
                println!("  Checking for updates...");
                if !source.connect_ticket.is_empty() {
                    println!("  Bootstrap: direct publisher ticket configured");
                }
                resolved = match tokio::time::timeout(
                    std::time::Duration::from_secs(20),
                    p2p_fn(source.clone(), primary_publisher.clone()),
                )
                .await
                {
                    Ok(Ok(Some(head))) => {
                        resolved_head_cid = head.head_cid;
                        discovered_head_bytes = Some(head.bytes);
                        Some("Carrier")
                    }
                    Ok(Ok(None)) => None,
                    Ok(Err(error)) => return Err(error),
                    Err(_) => None,
                };
            }
        }

        // Step B: try an explicitly granted transport override.
        if resolved.is_none() && !ordered_gateways.is_empty() {
            if no_p2p {
                println!("  Checking explicit transport override...");
            } else {
                eprintln!("  Carrier unavailable, trying explicit transport override...");
            }
            if let Some((_release_cid, head_bytes, gw)) =
                try_gateway_head_discovery(&ordered_gateways, &primary_publisher).await
            {
                discovered_head_bytes = Some(head_bytes);
                working_gateway = Some(gw);
                resolved = Some("gateway");
            }
        }

        resolved.ok_or_else(|| {
            let mut msg = String::from("Could not discover updates.");
            if no_p2p {
                msg.push_str("\n  Carrier was skipped (--no-p2p).");
            } else {
                msg.push_str("\n  Carrier discovery did not find a live trusted source.");
            }
            if ordered_gateways.is_empty() {
                msg.push_str(
                    "\n  No explicit transport override configured. Use --gateway <url> only when you explicitly approve web transport.",
                );
            } else {
                msg.push_str("\n  Explicit transport override was also unreachable.");
            }
            if source.head_cid.is_empty() {
                msg.push_str("\n  No cached head CID in sources.json.");
            }
            msg.push_str("\n  Manual override: elastos update --head-cid <cid>");
            anyhow::anyhow!("{}", msg)
        })?
    };

    // 3. Fetch or reuse release-head.json
    let head_bytes = if let Some(bytes) = discovered_head_bytes {
        if let Some(ref gw) = working_gateway {
            println!(
                "  Using release head from explicit transport override: {}",
                gw
            );
        }
        bytes
    } else {
        let head_cid = resolved_head_cid
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No release head CID resolved"))?;
        println!(
            "  Fetching release head: {}...",
            &head_cid[..12.min(head_cid.len())]
        );
        fetch_fn(head_cid, ordered_gateways.clone()).await?
    };
    if let Some(cid) = resolved_head_cid.as_deref() {
        verify_release_metadata_cid(cid, &head_bytes)?;
    }

    // 4. Verify signature
    let head = verify_release_envelope(&head_bytes, "elastos.release.head.v1", &primary_publisher)
        .map_err(new_publisher_key_hint)?;

    let head_version = head["payload"]["version"].as_str().unwrap_or("unknown");
    verify_source_channel(&source, head["payload"]["channel"].as_str().unwrap_or(""))?;
    let release_cid = head["payload"]["latest_release_cid"].as_str().unwrap_or("");
    let release_object_cid = optional_release_object_cid(&head)?;

    run_upgrade_with_restart(
        fetch_fn,
        &head,
        &head_bytes,
        resolved_head_cid.as_deref(),
        head_version,
        release_cid,
        release_object_cid,
        &current_version,
        &source,
        data_dir,
        check_only,
        &ordered_gateways,
        auto_confirm,
        force,
        discovery_method,
        working_gateway.as_deref(),
        carrier_context,
        apply_mode,
        restart_owner,
    )
    .await
}

/// Bind a verified release envelope to the exact bytes chosen by its signed head.
/// This digest is additional envelope evidence; the CID remains content identity.
pub(crate) fn verify_release_binding(
    head: &serde_json::Value,
    release_bytes: &[u8],
    release: &serde_json::Value,
) -> anyhow::Result<()> {
    use sha2::Digest;
    let head = &head["payload"];
    let release = &release["payload"];
    if head["schema"].as_str() != Some("elastos.release.head/v1")
        || release["schema"].as_str() != Some("elastos.release/v1")
    {
        anyhow::bail!("Release head or release schema is unsupported");
    }
    let expected = head["release_sha256"]
        .as_str()
        .filter(|value| {
            value.len() == 64
                && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or_else(|| anyhow::anyhow!("Release head requires a lowercase SHA-256 envelope binding; ask the publisher to update its metadata"))?;
    let actual = hex::encode(sha2::Sha256::digest(release_bytes));
    if actual != expected {
        anyhow::bail!("Release envelope differs from the signed head; retry after the publisher finishes updating");
    }
    for field in ["version", "channel"] {
        let value = head[field]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Release head requires a nonempty {field}"))?;
        if release[field].as_str() != Some(value) {
            anyhow::bail!("Release head and release {field} must match");
        }
    }
    Ok(())
}

/// Execute upgrade from a verified release head.
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
async fn run_upgrade_from_head(
    fetch_fn: &FetchFn,
    head: &serde_json::Value,
    head_bytes: &[u8],
    resolved_head_cid: Option<&str>,
    version: &str,
    release_cid: &str,
    release_object_cid: Option<&str>,
    current_version: &str,
    source: &TrustedSource,
    data_dir: &std::path::Path,
    check_only: bool,
    ordered_gateways: &[String],
    auto_confirm: bool,
    force: bool,
    discovery_method: &str,
    working_gateway: Option<&str>,
    carrier_context: crate::setup::FirstPartyCarrierContext,
    apply_mode: ApplyMode,
) -> anyhow::Result<()> {
    run_upgrade_with_restart(
        fetch_fn,
        head,
        head_bytes,
        resolved_head_cid,
        version,
        release_cid,
        release_object_cid,
        current_version,
        source,
        data_dir,
        check_only,
        ordered_gateways,
        auto_confirm,
        force,
        discovery_method,
        working_gateway,
        carrier_context,
        apply_mode,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_upgrade_with_restart(
    fetch_fn: &FetchFn,
    head: &serde_json::Value,
    head_bytes: &[u8],
    resolved_head_cid: Option<&str>,
    version: &str,
    release_cid: &str,
    release_object_cid: Option<&str>,
    current_version: &str,
    source: &TrustedSource,
    data_dir: &Path,
    check_only: bool,
    ordered_gateways: &[String],
    auto_confirm: bool,
    force: bool,
    discovery_method: &str,
    working_gateway: Option<&str>,
    carrier_context: crate::setup::FirstPartyCarrierContext,
    apply_mode: ApplyMode,
    mut restart_owner: Option<&mut dyn RestartOwner>,
) -> anyhow::Result<()> {
    // Admit the exact signed publication before check-only/version success or artifacts.
    if release_cid.is_empty() {
        anyhow::bail!("Release head has no latest_release_cid");
    }
    println!(
        "  Fetching release: {}...",
        &release_cid[..12.min(release_cid.len())]
    );
    let release_bytes = if let Some(gateway) = working_gateway {
        println!(
            "  Using release manifest from explicit transport override: {}",
            gateway
        );
        fetch_release_manifest_via_gateway(gateway).await?
    } else {
        fetch_fn(release_cid.to_string(), ordered_gateways.to_vec()).await?
    };
    // Gateway bytes are bound by verify_release_binding; release_sha256 must stay mandatory.
    if working_gateway.is_none() {
        verify_release_metadata_cid(release_cid, &release_bytes)?;
    }

    // Verify the chosen envelope, then its binding to the already verified head.
    let (release, signer_did) = verify_release_envelope_against_dids(
        &release_bytes,
        "elastos.release.v1",
        &source.publisher_dids,
    )
    .map_err(new_publisher_key_hint)?;
    verify_release_binding(head, &release_bytes, &release)?;
    verify_source_channel(source, head["payload"]["channel"].as_str().unwrap_or(""))?;
    println!("  Release signer: {}", signer_did);

    // 5. Compare versions
    println!();
    println!("  Latest available:  {}", version);
    println!(
        "  Installed release: {}",
        if current_version.is_empty() {
            "unknown"
        } else {
            current_version
        }
    );

    let repair_version = force && installed_release_version(current_version).is_err();
    let version_order =
        compare_release_versions(if repair_version { "" } else { current_version }, version)?;
    match version_order {
        Ordering::Equal if !force => {
            println!();
            println!("  Installed release is up to date.");
            return Ok(());
        }
        Ordering::Less if !force => {
            anyhow::bail!(
                "Signed release {version} is older than installed release {current_version}; use an explicit rollback command if intended"
            );
        }
        _ => {}
    }

    // Show update plan
    let is_rollback = force && version_order != Ordering::Greater;
    println!();
    if is_rollback {
        println!("  Rollback plan:");
    } else {
        println!("  Update plan:");
    }
    println!(
        "    {} → {}{}",
        if current_version.is_empty() {
            "unknown"
        } else {
            current_version
        },
        version,
        if is_rollback { " (rollback)" } else { "" }
    );
    println!("    Source:     {}", source.name);
    println!(
        "    Channel:   {}",
        if source.channel.is_empty() {
            "stable"
        } else {
            &source.channel
        }
    );
    println!("    Discovery: {}", discovery_method);
    if let Some(cid) = resolved_head_cid {
        println!("    Head:      {}...", &cid[..12.min(cid.len())]);
    }
    println!(
        "    Release:   {}...",
        &release_cid[..12.min(release_cid.len())]
    );
    if let Some(cid) = release_object_cid {
        println!("    Release object: {}...", &cid[..12.min(cid.len())]);
    }

    if check_only {
        println!();
        println!("  Run `elastos update` to install.");
        return Ok(());
    }

    // Confirmation prompt (unless --yes or piped stdin)
    if !auto_confirm && std::io::stdin().is_terminal() {
        eprint!("\n  Proceed? [y/N] ");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("  Update cancelled.");
            return Ok(());
        }
    }

    println!();
    println!("  Installing {} → {}...", current_version, version);

    // 8. Download binary for current platform
    let release_platform = detect_release_platform();
    let component_platform = crate::setup::detect_platform();
    let binary_info = &release["payload"]["platforms"][release_platform]["binary"];
    let binary_cid = binary_info["cid"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No binary CID for platform {}", release_platform))?;
    let binary_sha256 = binary_info["sha256"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No binary SHA-256 for platform {}", release_platform))?;
    let binary_size = binary_info["size"].as_u64();

    println!(
        "  Downloading binary ({}){}...",
        release_platform,
        binary_size
            .map(|n| format!(" [{}]", format_bytes(n as usize)))
            .unwrap_or_default()
    );
    if let Some(owner) = restart_owner.as_ref() {
        owner.progress("downloading", &download_message(binary_size))?;
    }
    let binary_data = fetch_fn(binary_cid.to_string(), ordered_gateways.to_vec()).await?;
    if let Some(owner) = restart_owner.as_ref() {
        owner.progress(
            "verifying",
            "Verifying the update. Home restarts when the update is ready.",
        )?;
    }
    println!("  Downloaded binary: {}", format_bytes(binary_data.len()));

    // Verify SHA-256
    {
        use sha2::Digest;
        let hash = sha2::Sha256::digest(&binary_data);
        let actual = hex::encode(hash);
        if actual != binary_sha256 {
            anyhow::bail!(
                "Binary SHA-256 mismatch!\n  Expected: {}\n  Got:      {}",
                binary_sha256,
                actual
            );
        }
    }
    println!("  Binary verified (SHA-256 ✓)");

    // 9. Download components.json
    let comp_info = &release["payload"]["platforms"][release_platform]["components"];
    let comp_cid = comp_info["cid"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No components CID for platform {}", release_platform))?;
    let comp_sha256 = comp_info["sha256"].as_str().ok_or_else(|| {
        anyhow::anyhow!("No components SHA-256 for platform {}", release_platform)
    })?;
    let comp_size = comp_info["size"].as_u64();

    println!(
        "  Downloading components.json{}...",
        comp_size
            .map(|n| format!(" [{}]", format_bytes(n as usize)))
            .unwrap_or_default()
    );
    if let Some(owner) = restart_owner.as_ref() {
        owner.progress("downloading", &download_message(comp_size))?;
    }
    let comp_data = fetch_fn(comp_cid.to_string(), ordered_gateways.to_vec()).await?;
    if let Some(owner) = restart_owner.as_ref() {
        owner.progress(
            "verifying",
            "Verifying the update. Home restarts when the update is ready.",
        )?;
    }
    println!(
        "  Downloaded components.json: {}",
        format_bytes(comp_data.len())
    );

    {
        use sha2::Digest;
        let hash = sha2::Sha256::digest(&comp_data);
        let actual = hex::encode(hash);
        if actual != comp_sha256 {
            anyhow::bail!(
                "Components SHA-256 mismatch!\n  Expected: {}\n  Got:      {}",
                comp_sha256,
                actual
            );
        }
    }
    println!("  Components verified (SHA-256 ✓)");
    crate::installed_release::admit_descriptor(
        binary_info,
        binary_sha256,
        binary_data.len() as u64,
    )?;
    crate::installed_release::admit_descriptor(comp_info, comp_sha256, comp_data.len() as u64)?;
    anyhow::ensure!(
        comp_data.len() <= 4 * 1024 * 1024,
        "Installed components exceed their byte bound"
    );
    let manifest: crate::setup::ComponentsManifest = serde_json::from_slice(&comp_data)?;
    crate::setup::admit_release_components(&manifest, &component_platform)?;

    if apply_mode == ApplyMode::Normal {
        // 10. Atomic replace binary
        let bin_path = if source.install_path.is_empty() {
            default_install_path()
        } else {
            PathBuf::from(&source.install_path)
        };
        let transaction = InstallTransaction::acquire(data_dir, &bin_path)?;
        transaction.recover()?;
        anyhow::ensure!(
            transaction.uses_consumed_layout(),
            "Legacy recovery finished. Run the update again before preparing its new release."
        );
        let previous = crate::installed_release::read_without_migration_for_update(
            transaction.data_dir(),
            transaction.binary_path(),
            source,
            repair_version,
        )?;
        let data_dir = transaction.data_dir();
        let bin_path = transaction.binary_path().to_path_buf();
        let old_components = std::fs::read(data_dir.join("components.json")).ok();
        let changed_capsules =
            changed_capsule_names(old_components.as_deref(), &comp_data).unwrap_or_default();
        // Prepare all five original backups before the first live artifact changes.
        // Build gateway list with working gateway first (if discovered via gateway)
        let save_gateways = if ordered_gateways.is_empty() && working_gateway.is_none() {
            normalize_gateways(&source.gateways)
        } else if let Some(wg) = working_gateway {
            let wg_normalized = wg.trim_end_matches('/').to_string();
            let mut gws = vec![wg_normalized.clone()];
            for g in ordered_gateways {
                let normalized = g.trim_end_matches('/').to_string();
                if normalized != wg_normalized {
                    gws.push(normalized);
                }
            }
            normalize_gateways(&gws)
        } else {
            normalize_gateways(ordered_gateways)
        };

        let mut sources = load_trusted_sources(data_dir)?;
        if let Some(stored_source) = sources.source_named_mut(Some(&source.name)) {
            stored_source.gateways = save_gateways;
            stored_source.installed_version = version.to_string();
            stored_source.install_path = bin_path.display().to_string();
            stored_source.head_cid = resolved_head_cid
                .map(|s| s.to_string())
                .unwrap_or_else(|| stored_source.head_cid.clone());
        } else {
            let mut updated_source = source.clone();
            updated_source.gateways = save_gateways;
            updated_source.installed_version = version.to_string();
            updated_source.install_path = bin_path.display().to_string();
            updated_source.head_cid = resolved_head_cid
                .map(|s| s.to_string())
                .unwrap_or_else(|| updated_source.head_cid.clone());
            sources.upsert_source(updated_source);
        }
        let source_bytes = serde_json::to_vec_pretty(&sources)?;
        let files = [
            (ReleaseFile::RuntimeBinary, binary_data.as_slice()),
            (ReleaseFile::Components, comp_data.as_slice()),
            (ReleaseFile::Sources, source_bytes.as_slice()),
            (ReleaseFile::ReleaseHead, head_bytes),
            (ReleaseFile::ReleaseManifest, release_bytes.as_slice()),
        ];
        transaction.preflight_prepare(&files, &previous.head, &previous.release)?;
        verify_candidate_before_migration(&transaction, &binary_data, version).await?;
        crate::installed_release::load_or_migrate_for_update(
            data_dir,
            &bin_path,
            source,
            transaction.writer_guard(),
            repair_version,
        )?;
        transaction.prepare(&files)?;
        let activation = transaction.activate_artifacts_for_support()?;
        println!("  Installed binary: {}", bin_path.display());
        println!("  Installed binary verified (version ✓)");
        println!(
            "  Installed components: {}",
            data_dir.join("components.json").display()
        );
        // Existing support refresh remains inside the same writer guard. Complete
        // support staging/rollback is still required by the release activation gate.
        let support_result: anyhow::Result<()> = async {
            let refreshed_components =
                crate::setup::refresh_installed_components_for_update_in_context(
                    data_dir,
                    old_components.as_deref(),
                    &comp_data,
                    &component_platform,
                    carrier_context,
                )
                .await?;
            if refreshed_components.is_empty() {
                println!("  Installed support assets unchanged");
            } else {
                println!(
                    "  Refreshed installed support assets: {}",
                    refreshed_components.join(", ")
                );
            }

            // 12. Clear only changed capsule cache entries.
            let cleared = evict_changed_capsule_cache(data_dir, &changed_capsules);
            if cleared > 0 {
                println!(
                    "  Cleared {} changed cached capsule(s): {}",
                    cleared,
                    changed_capsules.join(", ")
                );
            } else if old_components.is_some() {
                println!("  Capsule cache unchanged");
            }

            // The replaced binary makes an existing host exit through its installed
            // binary supersession watch. Do not migrate protected objects until that
            // host has released the Runtime identity and all object owners are offline.
            wait_for_runtime_host_release(data_dir).await?;
            let principal_root_backup = data_dir.join("backups").join(format!(
                "principal-root-upgrade-{}-{}",
                crate::auth::now_ts(),
                std::process::id()
            ));
            let principal_root_receipt =
                crate::api::auth_gateway::migrate_configured_principal_roots_for_update(
                    data_dir,
                    &principal_root_backup,
                    &activation,
                )?;
            println!(
                "  Principal-root readiness: {} ({} object(s))",
                principal_root_receipt.status, principal_root_receipt.object_count
            );

            Ok(())
        }
        .await;
        if let Err(error) = support_result {
            return match transaction.recover() {
                Ok(()) => Err(error.context("update refused; original release files restored")),
                Err(recovery) => {
                    Err(error.context(format!("release recovery required: {recovery:#}")))
                }
            };
        }
        transaction.commit_checked(|| Ok(()))?;

        println!();
        println!("  ElastOS {} installed successfully!", version);
        println!();

        return Ok(());
    }

    // The restart owner stages support while Home runs. Standalone frozen updates
    // retain their offline fence and unchanged-support contract.
    let bin_path = if source.install_path.is_empty() {
        default_install_path()
    } else {
        PathBuf::from(&source.install_path)
    };
    let transaction = InstallTransaction::acquire(data_dir, &bin_path)?;
    let mut offline = if restart_owner.is_none() {
        Some(
            crate::host_lock::acquire_host_process_lock(
                transaction.data_dir(),
                "update",
                "offline",
            )
            .context("offline update requires the restart owner to stop Runtime first")?,
        )
    } else {
        None
    };
    transaction.recover()?;
    anyhow::ensure!(
        transaction.uses_consumed_layout(),
        "Legacy recovery finished. Run the update again before preparing its new release."
    );
    let previous = crate::installed_release::read_without_migration_for_update(
        transaction.data_dir(),
        transaction.binary_path(),
        source,
        repair_version,
    )?;
    let data_dir = transaction.data_dir();
    crate::api::auth_gateway::verify_configured_principal_roots_ready(data_dir)?;
    let old_components = std::fs::read(data_dir.join("components.json"))?;
    let staged_support = if let Some(owner) = restart_owner.as_mut() {
        Some(
            crate::setup::stage_update_support(
                data_dir,
                &old_components,
                &comp_data,
                &component_platform,
                fetch_fn,
                *owner,
            )
            .await?,
        )
    } else {
        None
    };
    if restart_owner.is_none() {
        frozen_support_snapshot(
            data_dir,
            &old_components,
            &comp_data,
            &component_platform,
            &transaction.excluded_paths(),
        )?;
    }
    let bin_path = transaction.binary_path();

    // Build gateway list with working gateway first (if discovered via gateway)
    let save_gateways = if ordered_gateways.is_empty() && working_gateway.is_none() {
        normalize_gateways(&source.gateways)
    } else if let Some(wg) = working_gateway {
        let wg_normalized = wg.trim_end_matches('/').to_string();
        let mut gws = vec![wg_normalized.clone()];
        for g in ordered_gateways {
            let normalized = g.trim_end_matches('/').to_string();
            if normalized != wg_normalized {
                gws.push(normalized);
            }
        }
        normalize_gateways(&gws)
    } else {
        normalize_gateways(ordered_gateways)
    };

    let mut sources = load_trusted_sources(data_dir)?;
    let installed_source = sources.source_named(Some(&source.name)).ok_or_else(|| {
        anyhow::anyhow!("update source changed while preparing the release; retry")
    })?;
    if serde_json::to_value(installed_source)? != serde_json::to_value(source)? {
        anyhow::bail!("update source changed while preparing the release; retry");
    }
    let stored_source = sources.source_named_mut(Some(&source.name)).unwrap();
    stored_source.gateways = save_gateways;
    stored_source.installed_version = version.to_string();
    stored_source.install_path = bin_path.display().to_string();
    if let Some(head_cid) = resolved_head_cid {
        stored_source.head_cid = head_cid.to_string();
    }
    let source_bytes = serde_json::to_vec_pretty(&sources)?;
    let files = [
        (ReleaseFile::RuntimeBinary, binary_data.as_slice()),
        (ReleaseFile::Components, comp_data.as_slice()),
        (ReleaseFile::Sources, source_bytes.as_slice()),
        (ReleaseFile::ReleaseHead, head_bytes),
        (ReleaseFile::ReleaseManifest, release_bytes.as_slice()),
    ];
    transaction.preflight_prepare(&files, &previous.head, &previous.release)?;
    verify_candidate_before_migration(&transaction, &binary_data, version).await?;
    crate::installed_release::load_or_migrate_for_update(
        data_dir,
        bin_path,
        source,
        transaction.writer_guard(),
        repair_version,
    )?;
    transaction.prepare(&files)?;
    if let Some((_, paths)) = &staged_support {
        if let Err(error) = transaction.prepare_support(paths) {
            transaction.abort()?;
            return Err(error);
        }
    }
    let prepared = (|| {
        let support = support_snapshot(
            data_dir,
            &old_components,
            &comp_data,
            &component_platform,
            &transaction.excluded_paths(),
        )?;
        let restart_plan = restart_owner
            .as_ref()
            .map(|owner| owner.plan(support.clone(), current_version, version))
            .transpose()?;
        if let Some(mut plan) = restart_plan {
            plan.support_paths = support_paths(&old_components, &comp_data, &component_platform)?;
            transaction.prepare_restart(plan)?;
        }
        Ok::<_, anyhow::Error>(support)
    })();
    let support = match prepared {
        Ok(support) => support,
        Err(error) => {
            transaction.abort()?;
            return Err(error);
        }
    };
    // All downloads, executable checks and rollback preparation finish while the
    // current Home owns its host lock. The offline fence starts at activation.
    if let Some(owner) = restart_owner.as_mut() {
        if let Err(error) =
            owner.progress("restarting", "Installing the update and restarting Home.")
        {
            transaction.abort_pre_activation_restart()?;
            return Err(error);
        }
        owner.stop().await?;
    }
    if restart_owner.is_some() {
        offline = Some(crate::host_lock::acquire_host_process_lock(
            data_dir, "update", "offline",
        )?);
    }
    let activation = transaction.commit_checked(|| {
        if support_snapshot(
            data_dir,
            &old_components,
            &comp_data,
            &component_platform,
            &transaction.excluded_paths(),
        )? != support
        {
            anyhow::bail!("installed support changed during activation");
        }
        Ok(())
    });
    if let Some(owner) = restart_owner.as_mut() {
        drop(offline);
        let candidate = match activation {
            Ok(()) => {
                let record = transaction.claim_start(false)?;
                owner.start(&transaction, record).await
            }
            Err(error) => Err(error),
        };
        if let Err(error) = candidate {
            owner
                .stop()
                .await
                .context("candidate cleanup failed; retain rollback")?;
            let offline = crate::host_lock::acquire_host_process_lock(
                data_dir,
                "update-recovery",
                "offline",
            )?;
            transaction.restore_for_restart()?;
            verify_restart_support(&transaction)?;
            drop(offline);
            let previous = transaction.claim_start(true)?;
            if let Err(previous_error) = owner.start(&transaction, previous).await {
                owner
                    .stop()
                    .await
                    .context("previous Home cleanup failed; retain recovery journal")?;
                return Err(previous_error
                    .context("previous Home did not become ready; retain recovery journal"));
            }
            finish_ready_restart(&transaction)?;
            transaction.finish_restart()?;
            return Err(
                error.context("previous release restored and Home restarted; user data preserved")
            );
        }
        finish_ready_restart(&transaction)?;
        transaction.finish_restart()?;
        return Ok(());
    }
    activation?;

    println!();
    println!(
        "  Runtime {} files are installed. Start Home to check the update.",
        version
    );
    println!();

    Ok(())
}

/// A ready host is accepted only while the frozen support still matches its durable plan.
pub(crate) fn verify_restart_support(transaction: &InstallTransaction) -> anyhow::Result<()> {
    let record = transaction.restart_record()?;
    let previous = matches!(
        record.phase,
        RestartPhase::Restored
            | RestartPhase::PreviousStartClaimed
            | RestartPhase::PreviousRunning
            | RestartPhase::PreviousReady
    );
    transaction.verify_support(previous)?;
    if support_paths_snapshot(
        transaction.data_dir(),
        &record.plan.support_paths,
        &transaction.excluded_paths(),
    )? != record.plan.support_sha256
    {
        anyhow::bail!("Installed support changed; retain recovery journal for repair.");
    }
    Ok(())
}

pub(crate) fn finish_ready_restart(transaction: &InstallTransaction) -> anyhow::Result<()> {
    verify_restart_support(transaction)?;
    let record = transaction.restart_record()?;
    let pid = record.pid.context("ready host identity is missing")?;
    transaction.record_ready(&record.generation, pid)
}

async fn wait_for_runtime_host_release(data_dir: &Path) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if crate::host_lock::active_host_process(data_dir)?.is_none() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "the active Runtime did not stop for principal-root upgrade; installation is not ready"
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// Frozen support proves byte preservation across this update window. Signed archive
/// qualification and complete installed acceptance remain with the release owner.
pub(crate) fn frozen_support_snapshot(
    data_dir: &Path,
    old: &[u8],
    new: &[u8],
    platform: &str,
    excluded: &std::collections::BTreeSet<PathBuf>,
) -> anyhow::Result<String> {
    let old_value: serde_json::Value = serde_json::from_slice(old)?;
    let new_value: serde_json::Value = serde_json::from_slice(new)?;
    if old_value["schema"] != "elastos.components/v1"
        || new_value["schema"] != "elastos.components/v1"
    {
        anyhow::bail!("unsupported components schema for offline update");
    }
    for field in ["external", "capsules", "profiles", "model_catalog"] {
        if old_value.get(field) != new_value.get(field) {
            anyhow::bail!(
                "support change in {field} requires complete staging by the release owner"
            );
        }
    }
    support_snapshot(data_dir, old, new, platform, excluded)
}

fn support_snapshot(
    data_dir: &Path,
    old: &[u8],
    new: &[u8],
    platform: &str,
    excluded: &std::collections::BTreeSet<PathBuf>,
) -> anyhow::Result<String> {
    support_paths_snapshot(data_dir, &support_paths(old, new, platform)?, excluded)
}

fn support_paths(
    old: &[u8],
    new: &[u8],
    platform: &str,
) -> anyhow::Result<std::collections::BTreeSet<PathBuf>> {
    let manifest: crate::setup::ComponentsManifest = serde_json::from_slice(new)?;
    let old_manifest: crate::setup::ComponentsManifest = serde_json::from_slice(old)?;
    let mut paths = std::collections::BTreeSet::from([
        PathBuf::from("bin"),
        PathBuf::from("capsules"),
        PathBuf::from("libexec"),
        PathBuf::from("tools"),
        PathBuf::from("model-catalog.json"),
    ]);
    for component in manifest
        .external
        .values()
        .chain(old_manifest.external.values())
    {
        let info = crate::setup::resolve_platform_info(component, platform);
        if let Some(path) = crate::setup::resolve_install_path(component, info) {
            paths.insert(PathBuf::from(path));
        }
    }
    Ok(paths)
}

fn support_paths_snapshot(
    data_dir: &Path,
    paths: &std::collections::BTreeSet<PathBuf>,
    excluded: &std::collections::BTreeSet<PathBuf>,
) -> anyhow::Result<String> {
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    for relative in paths {
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            anyhow::bail!("unsafe frozen support path");
        }
        // Directory ancestors stay within the owned data root; bundle symlinks are
        // fingerprinted as links, rather than followed into operator data.
        let mut parent = data_dir.to_path_buf();
        for part in relative.parent().unwrap().components() {
            parent.push(part);
            if let Ok(metadata) = std::fs::symlink_metadata(&parent) {
                if !metadata.is_dir() {
                    anyhow::bail!("frozen support parent is unsafe");
                }
            }
        }
        fingerprint_support_tree(&data_dir.join(relative), excluded, &mut digest)?;
    }
    Ok(hex::encode(digest.finalize()))
}

fn fingerprint_support_tree(
    path: &Path,
    excluded: &std::collections::BTreeSet<PathBuf>,
    digest: &mut sha2::Sha256,
) -> anyhow::Result<()> {
    use sha2::Digest;
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    if excluded.iter().any(|excluded| path.starts_with(excluded)) {
        return Ok(());
    }
    let path_bytes = path.as_os_str().as_encoded_bytes();
    digest.update((path_bytes.len() as u64).to_le_bytes());
    digest.update(path_bytes);
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            digest.update([0]);
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    digest.update(metadata.mode().to_le_bytes());
    if metadata.is_dir() {
        digest.update([1]);
        let mut children = std::fs::read_dir(path)?
            .map(|item| item.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()?;
        children.retain(|child| !excluded.contains(child));
        children.sort();
        digest.update((children.len() as u64).to_le_bytes());
        for child in children {
            fingerprint_support_tree(&child, excluded, digest)?;
        }
    } else if metadata.file_type().is_symlink() {
        digest.update([2]);
        let target = std::fs::read_link(path)?;
        let bytes = target.as_os_str().as_encoded_bytes();
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    } else if metadata.is_file() {
        digest.update([3]);
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        let before = file.metadata()?;
        if before.dev() != metadata.dev() || before.ino() != metadata.ino() {
            anyhow::bail!("frozen support changed while opening");
        }
        let mut buffer = [0; 16384];
        let mut content_hash = sha2::Sha256::new();
        loop {
            let length = file.read(&mut buffer)?;
            if length == 0 {
                break;
            }
            content_hash.update(&buffer[..length]);
        }
        digest.update(before.len().to_le_bytes());
        digest.update(content_hash.finalize());
        let after = file.metadata()?;
        if before.len() != after.len() || before.modified()? != after.modified()? {
            anyhow::bail!("frozen support changed while reading");
        }
    } else {
        anyhow::bail!("frozen support has unsupported file type");
    }
    Ok(())
}

fn optional_release_object_cid(head: &serde_json::Value) -> anyhow::Result<Option<&str>> {
    let Some(cid) = head["payload"]["release_object_cid"].as_str() else {
        return Ok(None);
    };
    if cid.trim().is_empty() {
        return Ok(None);
    }
    cid::Cid::try_from(cid).map_err(|err| {
        anyhow::anyhow!(
            "Release head has invalid release_object_cid {}: {}",
            cid,
            err
        )
    })?;
    Ok(Some(cid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::save_trusted_sources;
    use axum::body::Body;
    use axum::extract::Path as AxumPath;
    use axum::http::StatusCode;
    use axum::routing::get;
    use axum::Router;

    fn raw_cid(bytes: &[u8]) -> String {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(bytes);
        let hash = cid::multihash::Multihash::<64>::wrap(0x12, digest.as_slice()).unwrap();
        cid::Cid::new_v1(0x55, hash).to_string()
    }

    #[test]
    fn release_metadata_cid_checks_raw_and_legacy_kubo_unixfs_bytes() {
        let bytes = b"{\"payload\":{\"version\":\"0.7.1-rc.2\"}}\n";
        let raw = "bafkreieynbxlorpjjxg22vh5sic4plqiwg3dfcul4edj74xlfi2uv37v2q";
        let legacy = "QmZFMnZjkiTqy9VY5FDdKudvpsKtKxifk1poBrV7k4sqp8";
        assert_eq!(raw_cid(bytes), raw);
        verify_release_metadata_cid(raw, bytes).unwrap();
        verify_release_metadata_cid(legacy, bytes).unwrap();
        assert!(verify_release_metadata_cid(raw, b"wrong bytes").is_err());
        assert!(verify_release_metadata_cid(legacy, b"wrong bytes").is_err());
        assert!(verify_release_metadata_cid("release-a", bytes).is_err());
    }

    #[test]
    fn support_fingerprint_frames_file_content_and_exact_transaction_exclusions() {
        use sha2::Digest;
        use std::os::unix::fs::MetadataExt;
        let fixture = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(fixture.path()).unwrap();
        let tree = root.join("libexec");
        std::fs::create_dir(&tree).unwrap();
        let a = tree.join("a");
        let b = tree.join("b");
        std::fs::write(&a, b"old a").unwrap();
        std::fs::write(&b, b"old b").unwrap();
        let excluded = std::collections::BTreeSet::new();
        let mut before = sha2::Sha256::new();
        fingerprint_support_tree(&tree, &excluded, &mut before).unwrap();
        // This exact payload hid b's removal in the former unframed digest stream.
        let mut forged = b"old a".to_vec();
        forged.extend_from_slice(b.as_os_str().as_encoded_bytes());
        forged.extend_from_slice(&std::fs::metadata(&b).unwrap().mode().to_le_bytes());
        forged.extend_from_slice(b"fileold b");
        std::fs::write(&a, forged).unwrap();
        std::fs::remove_file(&b).unwrap();
        let mut after = sha2::Sha256::new();
        fingerprint_support_tree(&tree, &excluded, &mut after).unwrap();
        assert_ne!(before.finalize(), after.finalize());

        let bin = root.join("bin");
        std::fs::create_dir(&bin).unwrap();
        let transaction = InstallTransaction::acquire(&root, &bin.join("elastos")).unwrap();
        let nested_lock = tree.join(".elastos.install.lock");
        std::fs::write(&nested_lock, b"nested support").unwrap();
        let mut before = sha2::Sha256::new();
        fingerprint_support_tree(&tree, &transaction.excluded_paths(), &mut before).unwrap();
        std::fs::write(&nested_lock, b"changed nested support").unwrap();
        let mut after = sha2::Sha256::new();
        fingerprint_support_tree(&tree, &transaction.excluded_paths(), &mut after).unwrap();
        assert_ne!(before.finalize(), after.finalize());
    }

    #[test]
    fn real_staging_preserves_support_fingerprint_with_binary_inside_data_bin() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = tempfile::tempdir().unwrap();
        let data = std::fs::canonicalize(fixture.path()).unwrap();
        let binary = data.join("bin/elastos");
        std::fs::create_dir(binary.parent().unwrap()).unwrap();
        let old_binary = b"#!/bin/sh\nprintf 'elastos 0.7.0\\n'\n";
        std::fs::write(&binary, old_binary).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(data.join("bin/support"), b"frozen installed support").unwrap();
        let components = b"{\"schema\":\"elastos.components/v1\",\"external\":{},\"profiles\":{},\"capsules\":{}}";
        std::fs::write(data.join("components.json"), components).unwrap();
        let transaction = InstallTransaction::acquire(&data, &binary).unwrap();
        let before = frozen_support_snapshot(
            &data,
            components,
            components,
            &crate::setup::detect_platform(),
            &transaction.excluded_paths(),
        )
        .unwrap();
        transaction
            .prepare(&[
                (ReleaseFile::RuntimeBinary, b"candidate binary"),
                (ReleaseFile::Components, components),
                (ReleaseFile::Sources, b"{}"),
                (ReleaseFile::ReleaseHead, b"{}"),
                (ReleaseFile::ReleaseManifest, b"{}"),
            ])
            .unwrap();
        let staged = frozen_support_snapshot(
            &data,
            components,
            components,
            &crate::setup::detect_platform(),
            &transaction.excluded_paths(),
        )
        .unwrap();
        assert_eq!(before, staged);
        transaction.abort().unwrap();
        assert_eq!(std::fs::read(&binary).unwrap(), old_binary);
        assert_eq!(
            std::fs::read(data.join("bin/support")).unwrap(),
            b"frozen installed support"
        );
    }

    #[test]
    fn release_versions_use_exact_semver_precedence() {
        assert_eq!(
            compare_release_versions("0.7.1-rc.1", "0.7.1-rc.10").unwrap(),
            Ordering::Greater
        );
        assert_eq!(
            compare_release_versions("0.7.1-rc.10", "0.7.1-rc.1").unwrap(),
            Ordering::Less
        );
        assert_eq!(
            compare_release_versions("0.7.1+build.1", "0.7.1+build.2").unwrap(),
            Ordering::Equal
        );
        assert_eq!(
            compare_release_versions("", "0.7.1-rc.1").unwrap(),
            Ordering::Greater
        );
        for invalid in ["0.7", "0.7.01", "0.7.1-", "0.7.1-rc_1", "unknown"] {
            assert!(compare_release_versions("0.7.0", invalid).is_err());
            assert!(compare_release_versions(invalid, "0.7.1").is_err());
        }
    }

    #[test]
    fn source_subscription_must_match_signed_channel() {
        let source: TrustedSource = serde_json::from_value(serde_json::json!({
            "name": "fixture", "publisher_dids": ["did:key:fixture"], "channel": "canary"
        }))
        .unwrap();
        verify_source_channel(&source, "canary").unwrap();
        assert!(verify_source_channel(&source, "stable").is_err());

        let legacy: TrustedSource = serde_json::from_value(serde_json::json!({
            "name": "legacy", "publisher_dids": ["did:key:fixture"]
        }))
        .unwrap();
        verify_source_channel(&legacy, "stable").unwrap();
        assert!(verify_source_channel(&legacy, "canary").is_err());
    }

    #[test]
    fn release_platform_matches_publisher_keys_for_each_host() {
        // CPU architecture alone must not select a Linux executable on macOS.
        // These are release keys; components use setup's separate platform names.
        for (os, arch, expected) in [
            ("linux", "x86_64", "x86_64-linux"),
            ("linux", "aarch64", "aarch64-linux"),
            ("macos", "x86_64", "x86_64-darwin"),
            ("macos", "aarch64", "aarch64-darwin"),
            ("windows", "aarch64", "unknown"),
            ("linux", "riscv64", "unknown"),
        ] {
            assert_eq!(release_platform_for(os, arch), expected);
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        assert_eq!(detect_release_platform(), "aarch64-darwin");
    }

    fn binding_envelope(payload: serde_json::Value, domain: &str) -> Vec<u8> {
        // Disposable deterministic identity; never a live publisher key.
        let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &key,
            domain,
            &serde_json::to_vec(&payload).unwrap(),
        );
        serde_json::to_vec(&serde_json::json!({
            "payload": payload, "signature": signature, "signer_did": signer_did
        }))
        .unwrap()
    }

    fn binding_head(release: &[u8]) -> serde_json::Value {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(release);
        let multihash = cid::multihash::Multihash::<64>::wrap(0x12, digest.as_slice()).unwrap();
        serde_json::json!({
            "schema": "elastos.release.head/v1", "version": "0.7.1", "channel": "stable",
            "latest_release_cid": cid::Cid::new_v1(0x55, multihash).to_string(),
            "release_sha256": hex::encode(digest)
        })
    }

    fn discovery_fixture() -> (TrustedSource, Vec<u8>, Vec<u8>) {
        let did =
            crate::crypto::encode_signing_key_did(&ed25519_dalek::SigningKey::from_bytes(&[7; 32]));
        let source = serde_json::from_value(serde_json::json!({
            "name": "fixture", "publisher_dids": [did], "channel": "stable",
            "installed_version": "0.7.0"
        }))
        .unwrap();
        let release = binding_envelope(
            serde_json::json!({"schema":"elastos.release/v1", "version":"0.7.1", "channel":"stable"}),
            "elastos.release.v1",
        );
        let head = binding_envelope(binding_head(&release), "elastos.release.head.v1");
        (source, head, release)
    }

    #[tokio::test]
    async fn discovery_handles_old_new_and_missing_cid_announcements() {
        let (source, head, release) = discovery_fixture();
        let head_cid = raw_cid(&head);
        let release_cid = raw_cid(&release);
        for (announcement, fetch_cid, resolved_cid) in [
            (
                serde_json::json!({"head_cid":head_cid,"release_cid":release_cid}),
                Some(head_cid.clone()),
                Some(head_cid.clone()),
            ),
            // Actual older publisher shape with a completed publish receipt.
            (
                serde_json::json!({"head_cid":release_cid,"release_cid":release_cid}),
                None,
                None,
            ),
            // Older publisher without its publish receipt.
            (
                serde_json::json!({"head_cid":release_cid,"release_cid":""}),
                None,
                None,
            ),
            (
                serde_json::json!({"head_cid":"","release_cid":""}),
                None,
                None,
            ),
            (serde_json::json!({}), None, None),
            (serde_json::json!({"release_cid":release_cid}), None, None),
            (
                serde_json::json!({"head_cid":head_cid}),
                None,
                Some(head_cid.clone()),
            ),
        ] {
            let result = resolve_discovered_head(&announcement, &source, |cid| {
                assert_eq!(cid, fetch_cid, "{announcement}");
                std::future::ready(Ok(head.clone()))
            })
            .await
            .unwrap();
            assert_eq!(result.head_cid, resolved_cid, "{announcement}");
            assert_eq!(result.bytes, head);
        }
    }

    #[tokio::test]
    async fn discovery_refuses_inconsistent_or_unauthenticated_heads() {
        let (source, head, release) = discovery_fixture();
        let release_cid = raw_cid(&release);
        let head_cid = raw_cid(&head);
        let modern = serde_json::json!({"head_cid":head_cid,"release_cid":release_cid});
        let legacy = serde_json::json!({"head_cid":release_cid,"release_cid":release_cid});
        for (announcement, bytes) in [
            (modern.clone(), release.clone()), // Valid release signature is the wrong domain.
            (legacy.clone(), release.clone()),
            (modern.clone(), b"malformed head".to_vec()),
            (
                serde_json::json!({"head_cid":head_cid,"release_cid":"wrong-release"}),
                head.clone(),
            ),
            (
                serde_json::json!({"head_cid":"wrong-cid","release_cid":release_cid}),
                head.clone(),
            ),
            (serde_json::json!({"head_cid":null}), head.clone()),
        ] {
            assert!(
                resolve_discovered_head(&announcement, &source, |_| {
                    std::future::ready(Ok(bytes))
                })
                .await
                .is_err(),
                "{announcement}"
            );
        }
        // An exact modern head fetch failure stays a failure; no filename retry.
        assert!(resolve_discovered_head(&modern, &source, |cid| {
            assert_eq!(cid, Some(head_cid));
            std::future::ready(Err(anyhow::anyhow!("fixture fetch failure")))
        })
        .await
        .is_err());
        for changed_source in [
            TrustedSource {
                channel: "canary".into(),
                ..source.clone()
            },
            TrustedSource {
                publisher_dids: vec!["did:key:other".into()],
                ..source.clone()
            },
        ] {
            assert!(resolve_discovered_head(&legacy, &changed_source, |_| {
                std::future::ready(Ok(head.clone()))
            })
            .await
            .is_err());
        }
    }

    #[tokio::test]
    async fn legacy_discovery_keeps_version_admission_and_exact_explicit_cids() {
        let (source, head, release) = discovery_fixture();
        let release_cid = raw_cid(&release);
        let announcement = serde_json::json!({"head_cid":release_cid,"release_cid":release_cid});
        let discovered = resolve_discovered_head(&announcement, &source, |_| {
            std::future::ready(Ok(head.clone()))
        })
        .await
        .unwrap();
        let discovery: TryP2pFn =
            Box::new(move |_, _| Box::pin(std::future::ready(Ok(Some(discovered.clone())))));
        for installed in ["0.7.0", "0.7.2"] {
            let data = tempfile::tempdir().unwrap();
            let mut sources = crate::sources::TrustedSourcesConfig::empty();
            sources.upsert_source(TrustedSource {
                installed_version: installed.into(),
                ..source.clone()
            });
            save_trusted_sources(data.path(), &sources).unwrap();
            let bytes = release.clone();
            let expected = release_cid.clone();
            let fetch: FetchFn = Box::new(move |cid, _| {
                assert_eq!(cid, expected);
                Box::pin(std::future::ready(Ok(bytes.clone())))
            });
            let result = run_update_for_data_dir(
                data.path(),
                &fetch,
                Some(&discovery),
                true,
                None,
                false,
                Vec::new(),
                installed,
                true,
                false,
            )
            .await;
            if installed == "0.7.0" {
                result.unwrap();
            } else {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("older than installed"));
            }
            // An explicit release CID is not reinterpreted as legacy discovery.
            let err = run_update_for_data_dir(
                data.path(),
                &fetch,
                Some(&discovery),
                true,
                Some(release_cid.clone()),
                false,
                Vec::new(),
                installed,
                true,
                false,
            )
            .await
            .unwrap_err();
            assert_eq!(
                err.to_string(),
                "Signed envelope signature verification failed"
            );
        }
    }

    #[tokio::test]
    async fn discovery_verification_error_stops_transport_fallback() {
        let (source, head, release) = discovery_fixture();
        let announcement = serde_json::json!({
            "head_cid": raw_cid(&head), "release_cid": raw_cid(&release)
        });
        let data = tempfile::tempdir().unwrap();
        let mut sources = crate::sources::TrustedSourcesConfig::empty();
        sources.upsert_source(source);
        save_trusted_sources(data.path(), &sources).unwrap();
        let discovery: TryP2pFn = Box::new(move |source, _| {
            let announcement = announcement.clone();
            let bytes = release.clone();
            Box::pin(async move {
                resolve_discovered_head(&announcement, &source, |_| std::future::ready(Ok(bytes)))
                    .await
                    .map(Some)
            })
        });
        let fetch: FetchFn =
            Box::new(|_, _| panic!("invalid discovery must stop content fetching"));
        let err = run_update_for_data_dir(
            data.path(),
            &fetch,
            Some(&discovery),
            true,
            None,
            false,
            vec!["http://127.0.0.1:1".into()],
            "0.7.0",
            true,
            false,
        )
        .await
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Signed envelope signature verification failed"
        );
    }

    #[tokio::test]
    async fn explicit_older_head_cid_selects_its_bytes_and_rejects_other_content() {
        use std::sync::{Arc, Mutex};

        let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let did = crate::crypto::encode_signing_key_did(&key);
        let data = tempfile::tempdir().unwrap();
        let mut sources = crate::sources::TrustedSourcesConfig::empty();
        sources.upsert_source(
            serde_json::from_value(serde_json::json!({
                "name": "fixture", "publisher_dids": [did], "channel": "stable",
                "installed_version": "0.6.0"
            }))
            .unwrap(),
        );
        save_trusted_sources(data.path(), &sources).unwrap();

        let release = binding_envelope(
            serde_json::json!({
                "schema": "elastos.release/v1", "version": "0.7.0", "channel": "stable"
            }),
            "elastos.release.v1",
        );
        let release_cid = raw_cid(&release);
        let mut head_payload = binding_head(&release);
        head_payload["version"] = serde_json::json!("0.7.0");
        let old_head = binding_envelope(head_payload, "elastos.release.head.v1");
        let old_head_cid = raw_cid(&old_head);
        let mut newer_payload = binding_head(&release);
        newer_payload["version"] = serde_json::json!("0.7.1");
        let newer_head = binding_envelope(newer_payload, "elastos.release.head.v1");

        let requests = Arc::new(Mutex::new(Vec::new()));
        let requested = requests.clone();
        let expected_head = old_head_cid.clone();
        let expected_release = release_cid.clone();
        let old_bytes = old_head.clone();
        let release_bytes = release.clone();
        let fetch: FetchFn = Box::new(move |cid, _| {
            requested.lock().unwrap().push(cid.clone());
            let bytes = if cid == expected_head {
                old_bytes.clone()
            } else if cid == expected_release {
                release_bytes.clone()
            } else {
                panic!("unexpected CID {cid}")
            };
            Box::pin(async move { Ok(bytes) })
        });
        run_update_for_data_dir(
            data.path(),
            &fetch,
            None,
            true,
            Some(old_head_cid.clone()),
            true,
            Vec::new(),
            "0.6.0",
            true,
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            *requests.lock().unwrap(),
            vec![old_head_cid.clone(), release_cid]
        );

        let wrong_bytes: FetchFn = Box::new(move |_, _| {
            let bytes = newer_head.clone();
            Box::pin(async move { Ok(bytes) })
        });
        let err = run_update_for_data_dir(
            data.path(),
            &wrong_bytes,
            None,
            true,
            Some(old_head_cid),
            true,
            Vec::new(),
            "0.6.0",
            true,
            false,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("do not match requested CID"));
    }

    #[tokio::test]
    async fn signed_head_on_other_channel_stops_before_release_fetch() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let did = crate::crypto::encode_signing_key_did(&key);
        let data = tempfile::tempdir().unwrap();
        let mut sources = crate::sources::TrustedSourcesConfig::empty();
        sources.upsert_source(
            serde_json::from_value(serde_json::json!({
                "name": "fixture", "publisher_dids": [did], "channel": "stable",
                "installed_version": "0.7.0"
            }))
            .unwrap(),
        );
        save_trusted_sources(data.path(), &sources).unwrap();
        let release = binding_envelope(
            serde_json::json!({
                "schema": "elastos.release/v1", "version": "0.7.1", "channel": "canary"
            }),
            "elastos.release.v1",
        );
        let mut payload = binding_head(&release);
        payload["channel"] = serde_json::json!("canary");
        let head = binding_envelope(payload, "elastos.release.head.v1");
        let head_cid = raw_cid(&head);
        let requested_head_cid = head_cid.clone();
        let fetch: FetchFn = Box::new(move |cid, _| {
            assert_eq!(cid, requested_head_cid);
            let bytes = head.clone();
            Box::pin(async move { Ok(bytes) })
        });
        let err = run_update_for_data_dir(
            data.path(),
            &fetch,
            None,
            true,
            Some(head_cid),
            true,
            Vec::new(),
            "0.7.0",
            true,
            false,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("differs from trusted source"));
    }

    #[tokio::test]
    async fn older_signed_release_needs_explicit_rollback() {
        let release = binding_envelope(
            serde_json::json!({
                "schema": "elastos.release/v1", "version": "0.7.1-rc.1", "channel": "stable"
            }),
            "elastos.release.v1",
        );
        let mut head_payload = binding_head(&release);
        head_payload["version"] = serde_json::json!("0.7.1-rc.1");
        let head_bytes = binding_envelope(head_payload, "elastos.release.head.v1");
        let did =
            crate::crypto::encode_signing_key_did(&ed25519_dalek::SigningKey::from_bytes(&[7; 32]));
        let head = verify_release_envelope(&head_bytes, "elastos.release.head.v1", &did).unwrap();
        let source: TrustedSource = serde_json::from_value(serde_json::json!({
            "name": "fixture", "publisher_dids": [did], "channel": "stable"
        }))
        .unwrap();
        let release_cid = raw_cid(&release);
        let fetch: FetchFn = Box::new(move |_, _| {
            let bytes = release.clone();
            Box::pin(async move { Ok(bytes) })
        });
        let data = tempfile::tempdir().unwrap();
        let result = run_upgrade_from_head(
            &fetch,
            &head,
            &head_bytes,
            None,
            "0.7.1-rc.1",
            &release_cid,
            None,
            "0.7.1-rc.10",
            &source,
            data.path(),
            true,
            &[],
            true,
            false,
            "fixture",
            None,
            crate::setup::FirstPartyCarrierContext::Setup,
            ApplyMode::Normal,
        )
        .await;
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("older than installed"));
        run_upgrade_from_head(
            &fetch,
            &head,
            &head_bytes,
            None,
            "0.7.1-rc.1",
            &release_cid,
            None,
            "0.7.1-rc.10",
            &source,
            data.path(),
            true,
            &[],
            true,
            true,
            "fixture",
            None,
            crate::setup::FirstPartyCarrierContext::Setup,
            ApplyMode::Normal,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn wrong_signed_artifact_content_stops_before_install() {
        let release = binding_envelope(
            serde_json::json!({
                "schema": "elastos.release/v1", "version": "0.7.1", "channel": "stable",
                "platforms": {(detect_release_platform()): {
                    "binary": {"cid": "binary-a", "sha256": "a".repeat(64)},
                    "components": {"cid": "components-a", "sha256": "b".repeat(64)}
                }}
            }),
            "elastos.release.v1",
        );
        let head_bytes = binding_envelope(binding_head(&release), "elastos.release.head.v1");
        let did =
            crate::crypto::encode_signing_key_did(&ed25519_dalek::SigningKey::from_bytes(&[7; 32]));
        let head = verify_release_envelope(&head_bytes, "elastos.release.head.v1", &did).unwrap();
        let source: TrustedSource = serde_json::from_value(serde_json::json!({
            "name": "fixture", "publisher_dids": [did], "channel": "stable"
        }))
        .unwrap();
        let release_cid = raw_cid(&release);
        let requested_release_cid = release_cid.clone();
        let fetch: FetchFn = Box::new(move |cid, _| {
            let bytes = if cid == requested_release_cid {
                release.clone()
            } else if cid == "binary-a" {
                b"wrong binary".to_vec()
            } else {
                panic!("unexpected CID {cid}")
            };
            Box::pin(async move { Ok(bytes) })
        });
        let data = tempfile::tempdir().unwrap();
        let err = run_upgrade_from_head(
            &fetch,
            &head,
            &head_bytes,
            None,
            "0.7.1",
            &release_cid,
            None,
            "0.7.0",
            &source,
            data.path(),
            false,
            &[],
            true,
            false,
            "fixture",
            None,
            crate::setup::FirstPartyCarrierContext::Setup,
            ApplyMode::Normal,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Binary SHA-256 mismatch"));
        assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 0);
    }

    #[test]
    fn test_release_binding_requires_digest_and_consistent_metadata() {
        let payload = serde_json::json!({"schema": "elastos.release/v1", "version": "0.7.1", "channel": "stable"});
        let release = binding_envelope(payload.clone(), "elastos.release.v1");
        let envelope: serde_json::Value = serde_json::from_slice(&release).unwrap();
        let head = serde_json::json!({"payload": binding_head(&release)});
        verify_release_binding(&head, &release, &envelope).unwrap();
        for invalid in [
            serde_json::Value::Null,
            serde_json::json!(7),
            serde_json::json!(""),
            serde_json::json!("a".repeat(63)),
            serde_json::json!("G".repeat(64)),
            serde_json::json!("A".repeat(64)),
            serde_json::json!("0".repeat(64)),
        ] {
            let mut changed = head.clone();
            changed["payload"]["release_sha256"] = invalid;
            assert!(verify_release_binding(&changed, &release, &envelope).is_err());
        }
        for field in ["schema", "version", "channel"] {
            for invalid in [
                serde_json::Value::Null,
                serde_json::json!(7),
                serde_json::json!(""),
                serde_json::json!("other"),
            ] {
                let mut changed = payload.clone();
                changed[field] = invalid;
                let bytes = binding_envelope(changed, "elastos.release.v1");
                let envelope = serde_json::from_slice(&bytes).unwrap();
                // Rebind these bytes so the schema/identity check, not the
                // digest mismatch, is responsible for rejection.
                let head = serde_json::json!({"payload": binding_head(&bytes)});
                assert!(verify_release_binding(&head, &bytes, &envelope).is_err());
            }
        }
    }

    #[tokio::test]
    async fn test_release_binding_precedes_artifacts_check_and_same_version_on_both_transports() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let payload = serde_json::json!({
            "schema": "elastos.release/v1", "version": "0.7.1", "channel": "stable",
            "platforms": {(detect_release_platform()): {
                "binary": {"cid": "binary-a", "sha256": "a".repeat(64)},
                "components": {"cid": "components", "sha256": "b".repeat(64)}
            }}
        });
        let release = binding_envelope(payload.clone(), "elastos.release.v1");
        let head = binding_head(&release);
        let release_cid = head["latest_release_cid"].as_str().unwrap().to_string();
        let mut different_payload = payload;
        different_payload["platforms"][detect_release_platform()]["binary"]["cid"] =
            serde_json::json!("binary-b");
        let different = binding_envelope(different_payload, "elastos.release.v1");
        let mut whitespace = release.clone();
        whitespace.push(b' ');
        let mut byte_change = release.clone();
        let index = byte_change.iter().position(|byte| *byte == b'7').unwrap();
        byte_change[index] = b'8';
        let mut missing = head.clone();
        missing.as_object_mut().unwrap().remove("release_sha256");
        let mut invalid = head.clone();
        invalid["release_sha256"] = serde_json::json!("invalid");
        let mut schema = head.clone();
        schema["schema"] = serde_json::json!("other");
        let cases = [
            ("matching", head.clone(), release.clone(), true),
            ("different signed release", head.clone(), different, false),
            ("whitespace", head.clone(), whitespace, false),
            ("one byte", head.clone(), byte_change, false),
            ("missing digest", missing, release.clone(), false),
            ("invalid digest", invalid, release.clone(), false),
            ("invalid head schema", schema, release.clone(), false),
        ];
        for gateway_transport in [false, true] {
            for (name, payload, release_bytes, matching) in &cases {
                for mode in ["install", "check", "same-version"] {
                    let head_bytes = binding_envelope(payload.clone(), "elastos.release.head.v1");
                    let did = crate::crypto::encode_signing_key_did(
                        &ed25519_dalek::SigningKey::from_bytes(&[7; 32]),
                    );
                    let verified_head =
                        verify_release_envelope(&head_bytes, "elastos.release.head.v1", &did)
                            .unwrap();
                    let source: TrustedSource = serde_json::from_value(serde_json::json!({
                        "name": "test", "publisher_dids": [did], "channel": "stable"
                    }))
                    .unwrap();
                    let artifact_requests = Arc::new(AtomicUsize::new(0));
                    let cid_requests = Arc::new(AtomicUsize::new(0));
                    let counter = artifact_requests.clone();
                    let metadata_counter = cid_requests.clone();
                    let bytes = release_bytes.clone();
                    let requested_release_cid = release_cid.clone();
                    let fetch: FetchFn = Box::new(move |cid, _| {
                        let counter = counter.clone();
                        let metadata_counter = metadata_counter.clone();
                        let bytes = bytes.clone();
                        let requested_release_cid = requested_release_cid.clone();
                        Box::pin(async move {
                            if cid == requested_release_cid {
                                metadata_counter.fetch_add(1, Ordering::SeqCst);
                                return Ok(bytes);
                            }
                            counter.fetch_add(1, Ordering::SeqCst);
                            anyhow::bail!("artifact-request-blocked")
                        })
                    });
                    let bytes = release_bytes.clone();
                    let gateway_requests = Arc::new(AtomicUsize::new(0));
                    let metadata_counter = gateway_requests.clone();
                    let app = Router::new().route(
                        "/release.json",
                        get(move || {
                            let bytes = bytes.clone();
                            metadata_counter.fetch_add(1, Ordering::SeqCst);
                            async move { bytes }
                        }),
                    );
                    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                    let gateway = format!("http://{}", listener.local_addr().unwrap());
                    let server = tokio::spawn(async move {
                        axum::serve(listener, app).await.unwrap();
                    });
                    let data = tempfile::tempdir().unwrap();
                    let result = run_upgrade_from_head(
                        &fetch,
                        &verified_head,
                        &head_bytes,
                        None,
                        "0.7.1",
                        &release_cid,
                        None,
                        if mode == "same-version" {
                            "0.7.1"
                        } else {
                            "0.7.0"
                        },
                        &source,
                        data.path(),
                        mode == "check",
                        &[],
                        true,
                        false,
                        "test",
                        gateway_transport.then_some(gateway.as_str()),
                        crate::setup::FirstPartyCarrierContext::Setup,
                        ApplyMode::Normal,
                    )
                    .await;
                    server.abort();
                    assert_eq!(
                        cid_requests.load(Ordering::SeqCst),
                        usize::from(!gateway_transport)
                    );
                    assert_eq!(
                        gateway_requests.load(Ordering::SeqCst),
                        usize::from(gateway_transport)
                    );
                    let expects_artifact = *matching && mode == "install";
                    assert_eq!(
                        artifact_requests.load(Ordering::SeqCst),
                        usize::from(expects_artifact),
                        "{name}/{mode}/{gateway_transport}"
                    );
                    if expects_artifact {
                        assert!(result
                            .unwrap_err()
                            .to_string()
                            .contains("artifact-request-blocked"));
                    } else {
                        assert_eq!(
                            result.is_ok(),
                            *matching,
                            "{name}/{mode}/{gateway_transport}: {result:?}"
                        );
                    }
                    assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 0);
                }
            }
        }
    }

    #[test]
    fn test_is_ipfs_content_gateway_matches_known_hosts() {
        assert!(is_ipfs_content_gateway("https://ipfs.io"));
        assert!(is_ipfs_content_gateway("https://dweb.link"));
        assert!(!is_ipfs_content_gateway("https://publisher.example.com"));
    }

    #[test]
    fn test_optional_release_object_cid_validates_when_present() {
        let head = serde_json::json!({
            "payload": {
                "release_object_cid": "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG"
            }
        });
        assert_eq!(
            optional_release_object_cid(&head).unwrap(),
            Some("QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG")
        );

        let invalid = serde_json::json!({
            "payload": {
                "release_object_cid": "not-a-cid"
            }
        });
        assert!(optional_release_object_cid(&invalid).is_err());
    }

    #[test]
    fn test_changed_capsule_names_only_returns_changed_entries() {
        let old = serde_json::json!({
            "external": {},
            "profiles": {},
            "capsules": {
                "chat": { "cid": "cid-chat-1", "sha256": "a", "size": 1, "platforms": ["x86_64-linux"] },
                "did-provider": { "cid": "cid-did-1", "sha256": "b", "size": 1, "platforms": ["x86_64-linux"] }
            }
        });
        let new = serde_json::json!({
            "external": {},
            "profiles": {},
            "capsules": {
                "chat": { "cid": "cid-chat-2", "sha256": "c", "size": 1, "platforms": ["x86_64-linux"] },
                "did-provider": { "cid": "cid-did-1", "sha256": "b", "size": 1, "platforms": ["x86_64-linux"] },
                "chain-provider": { "cid": "cid-chain-1", "sha256": "d", "size": 1, "platforms": ["x86_64-linux"] }
            }
        });
        let old_bytes = serde_json::to_vec(&old).unwrap();
        let new_bytes = serde_json::to_vec(&new).unwrap();
        let changed = changed_capsule_names(Some(old_bytes.as_slice()), &new_bytes).unwrap();
        assert_eq!(
            changed,
            vec!["chain-provider".to_string(), "chat".to_string()]
        );
    }

    #[cfg(unix)]
    async fn apply_signed_executable_fixture(
        data_dir: &Path,
        source: &TrustedSource,
        executable: &[u8],
        components: &[u8],
    ) -> anyhow::Result<()> {
        apply_signed_fixture_with_mode(
            data_dir,
            source,
            executable,
            components,
            ApplyMode::FrozenOffline,
        )
        .await
    }

    #[cfg(unix)]
    async fn apply_signed_fixture_with_mode(
        data_dir: &Path,
        _source: &TrustedSource,
        executable: &[u8],
        components: &[u8],
        mode: ApplyMode,
    ) -> anyhow::Result<()> {
        apply_signed_fixture_with_force(data_dir, _source, executable, components, mode, false)
            .await
    }

    #[cfg(unix)]
    async fn apply_signed_fixture_with_force(
        data_dir: &Path,
        _source: &TrustedSource,
        executable: &[u8],
        components: &[u8],
        mode: ApplyMode,
        force: bool,
    ) -> anyhow::Result<()> {
        use sha2::Digest;

        let binary_cid = raw_cid(executable);
        let components_cid = raw_cid(components);
        let release = binding_envelope(
            serde_json::json!({
                "schema": "elastos.release/v1", "version": "0.7.1", "channel": "stable",
                "platforms": {(detect_release_platform()): {
                    "binary": {
                        "cid": binary_cid, "sha256": hex::encode(sha2::Sha256::digest(executable)),
                        "size": executable.len()
                    },
                    "components": {
                        "cid": components_cid, "sha256": hex::encode(sha2::Sha256::digest(components)),
                        "size": components.len()
                    }
                }}
            }),
            "elastos.release.v1",
        );
        let release_cid = raw_cid(&release);
        let head_bytes = binding_envelope(binding_head(&release), "elastos.release.head.v1");
        let head_cid = raw_cid(&head_bytes);
        let artifacts = std::collections::HashMap::from([
            (head_cid.clone(), head_bytes),
            (release_cid.clone(), release),
            (binary_cid, executable.to_vec()),
            (components_cid, components.to_vec()),
        ]);
        let fetch: FetchFn = Box::new(move |cid, gateways| {
            assert!(gateways.is_empty(), "fixture must use only its CID fetcher");
            let bytes = artifacts.get(&cid).expect("unexpected fixture CID").clone();
            Box::pin(async move { Ok(bytes) })
        });
        match mode {
            ApplyMode::Normal => {
                run_update_for_data_dir(
                    data_dir,
                    &fetch,
                    None,
                    false,
                    Some(head_cid),
                    true,
                    vec![],
                    "0.7.0",
                    true,
                    force,
                )
                .await
            }
            ApplyMode::FrozenOffline => {
                run_frozen_update_for_data_dir(
                    data_dir,
                    &fetch,
                    None,
                    false,
                    Some(head_cid),
                    true,
                    vec![],
                    "0.7.0",
                    true,
                    force,
                )
                .await
            }
        }
    }

    #[cfg(unix)]
    fn publish_installed_fixture(
        data: &Path,
        binary: &Path,
        source: &TrustedSource,
        components: &[u8],
    ) {
        use sha2::Digest;
        use std::os::unix::fs::PermissionsExt;
        let binary_bytes = std::fs::read(binary).unwrap();
        let descriptor = |bytes: &[u8]| {
            serde_json::json!({
                "cid":raw_cid(bytes), "sha256":hex::encode(sha2::Sha256::digest(bytes)), "size":bytes.len()
            })
        };
        let release = binding_envelope(
            serde_json::json!({
                "schema":"elastos.release/v1", "version":source.installed_version, "channel":source.channel,
                "platforms":{(detect_release_platform()):{
                    "binary":descriptor(&binary_bytes), "components":descriptor(components)
                }}
            }),
            "elastos.release.v1",
        );
        let mut head = binding_head(&release);
        head["version"] = serde_json::json!(source.installed_version);
        head["channel"] = serde_json::json!(source.channel);
        let head = binding_envelope(head, "elastos.release.head.v1");
        let directory = data.join("installation");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        for (path, bytes) in [
            (installation_release_head_path(data), head),
            (installation_release_manifest_path(data), release),
        ] {
            std::fs::write(&path, bytes).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    #[cfg(unix)]
    fn default_apply_fixture(
        components: &[u8],
    ) -> (tempfile::TempDir, PathBuf, PathBuf, TrustedSource) {
        use std::os::unix::fs::PermissionsExt;
        let fixture = tempfile::tempdir().unwrap();
        let data = fixture.path().join("data");
        let binary = fixture.path().join("bin/elastos");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, b"#!/bin/sh\nprintf 'elastos 0.7.0\\n'\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(data.join("components.json"), components).unwrap();
        std::fs::write(data.join("owner-data"), b"owner data").unwrap();
        let did =
            crate::crypto::encode_signing_key_did(&ed25519_dalek::SigningKey::from_bytes(&[7; 32]));
        let source: TrustedSource = serde_json::from_value(serde_json::json!({
            "name": "fixture", "publisher_dids": [did], "channel": "stable",
            "installed_version": "0.7.0", "install_path": binary
        }))
        .unwrap();
        let mut sources = crate::sources::TrustedSourcesConfig::empty();
        sources.upsert_source(source.clone());
        save_trusted_sources(&data, &sources).unwrap();
        publish_installed_fixture(&data, &binary, &source, components);
        (fixture, data, binary, source)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn component_admission_refuses_before_replacing_owner_files() {
        let empty =
            br#"{"schema":"elastos.components/v1","external":{},"profiles":{},"capsules":{}}"#;
        for info in [
            serde_json::json!({"cid":raw_cid(b"support")}),
            serde_json::json!({"strategy":"source-build"}),
            serde_json::json!({"strategy":"local-copy"}),
            serde_json::json!({"checksum":"sha256:bad"}),
        ] {
            for mode in [ApplyMode::Normal, ApplyMode::FrozenOffline] {
                let (_fixture, data, binary, source) = default_apply_fixture(empty);
                let before_binary = std::fs::read(&binary).unwrap();
                let before_sources = std::fs::read(data.join("sources.json")).unwrap();
                // Optional support still needs manifest admission, even when staging skips it.
                let components = serde_json::to_vec(&serde_json::json!({
                    "schema":"elastos.components/v1", "profiles":{}, "capsules":{},
                    "external":{"optional":{"platforms":{"*":info}}}
                }))
                .unwrap();
                let error = apply_signed_fixture_with_mode(
                    &data,
                    &source,
                    b"#!/bin/sh\nprintf 'elastos 0.7.1\\n'\n",
                    &components,
                    mode,
                )
                .await
                .unwrap_err();
                let message = format!("{error:#}");
                assert!(
                    message.contains("checksum") || message.contains("strategy"),
                    "{message}"
                );
                assert_eq!(std::fs::read(&binary).unwrap(), before_binary);
                assert_eq!(
                    std::fs::read(data.join("sources.json")).unwrap(),
                    before_sources
                );
                assert_eq!(std::fs::read(data.join("components.json")).unwrap(), empty);
                assert!(!InstallTransaction::has_pending_recovery(&binary));
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn invalid_installed_version_requires_force_and_repairs_signed_installation() {
        let components =
            br#"{"schema":"elastos.components/v1","external":{},"profiles":{},"capsules":{}}"#;
        let executable = b"#!/bin/sh\nprintf 'elastos 0.7.1\\n'\n";
        for mode in [ApplyMode::Normal, ApplyMode::FrozenOffline] {
            let (_fixture, data, binary, mut source) = default_apply_fixture(components);
            source.installed_version = "broken".into();
            let mut sources = load_trusted_sources(&data).unwrap();
            sources.upsert_source(source.clone());
            save_trusted_sources(&data, &sources).unwrap();
            let before_binary = std::fs::read(&binary).unwrap();
            let before_sources = std::fs::read(data.join("sources.json")).unwrap();
            let error = apply_signed_fixture_with_force(
                &data, &source, executable, components, mode, false,
            )
            .await
            .unwrap_err();
            assert!(error.is::<InvalidInstalledVersion>());
            assert!(error.to_string().contains("elastos update --force"));
            assert_eq!(std::fs::read(&binary).unwrap(), before_binary);
            assert_eq!(
                std::fs::read(data.join("sources.json")).unwrap(),
                before_sources
            );
            // Force still verifies the candidate before changing the installed record.
            assert!(apply_signed_fixture_with_force(
                &data,
                &source,
                b"#!/bin/sh\nprintf 'elastos 0.7.0\\n'\n",
                components,
                mode,
                true
            )
            .await
            .is_err());
            assert_eq!(std::fs::read(&binary).unwrap(), before_binary);
            assert_eq!(
                std::fs::read(data.join("sources.json")).unwrap(),
                before_sources
            );
            apply_signed_fixture_with_force(&data, &source, executable, components, mode, true)
                .await
                .unwrap();
            assert_eq!(std::fs::read(&binary).unwrap(), executable);
            let installed = load_trusted_sources(&data)
                .unwrap()
                .default_source()
                .unwrap()
                .clone();
            assert_eq!(installed.installed_version, "0.7.1");
            crate::installed_release::read_without_migration(
                &data,
                &binary.canonicalize().unwrap(),
                &installed,
            )
            .unwrap();
        }
    }

    #[test]
    fn invalid_installed_version_has_local_repair_hint() {
        let error = compare_release_versions("broken", "0.7.1").unwrap_err();
        assert!(error.to_string().contains("elastos update --force"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn default_signed_update_waits_for_online_host_after_binary_activation() {
        let components =
            br#"{"schema":"elastos.components/v1","external":{},"profiles":{},"capsules":{}}"#;
        let (_fixture, data, binary, source) = default_apply_fixture(components);
        let executable = b"#!/bin/sh\nprintf 'elastos 0.7.1\\n'\n";
        let host = crate::host_lock::acquire_host_process_lock(&data, "fixture", "online").unwrap();
        assert!(crate::host_lock::active_host_process(&data)
            .unwrap()
            .is_some());
        let selected_binary = binary.clone();
        let selected_data = data.clone();
        let release_host = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
            while std::fs::read(&selected_binary).unwrap() != executable {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "default apply never activated the binary while host was online"
                );
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            // Keep the host active briefly after activation so apply must wait.
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            assert_eq!(
                load_trusted_sources(&selected_data)
                    .unwrap()
                    .default_source()
                    .unwrap()
                    .installed_version,
                "0.7.0",
                "state must be saved only after the online host releases ownership"
            );
            let head: serde_json::Value = serde_json::from_slice(
                &std::fs::read(installation_release_head_path(&selected_data)).unwrap(),
            )
            .unwrap();
            assert_eq!(head["payload"]["version"], "0.7.0");
            drop(host);
        });
        let result = apply_signed_fixture_with_mode(
            &data,
            &source,
            executable,
            components,
            ApplyMode::Normal,
        )
        .await;
        release_host.await.unwrap();
        result.unwrap();
        assert!(crate::host_lock::active_host_process(&data)
            .unwrap()
            .is_none());
        assert_eq!(std::fs::read(&binary).unwrap(), executable);
        assert_eq!(
            load_trusted_sources(&data)
                .unwrap()
                .default_source()
                .unwrap()
                .installed_version,
            "0.7.1"
        );
        assert_eq!(
            std::fs::read(data.join("owner-data")).unwrap(),
            b"owner data"
        );
        assert!(!InstallTransaction::has_pending_recovery(&binary));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn default_signed_update_refuses_development_support_and_preserves_capsule_cache() {
        let empty =
            br#"{"schema":"elastos.components/v1","external":{},"profiles":{},"capsules":{}}"#;
        let (_fixture, data, binary, source) = default_apply_fixture(empty);
        let old_provider_source = data.join("old-provider-source");
        let new_provider_source = data.join("new-provider-source");
        std::fs::write(&old_provider_source, b"old provider").unwrap();
        std::fs::write(&new_provider_source, b"new provider").unwrap();
        std::fs::create_dir_all(data.join("bin")).unwrap();
        std::fs::write(data.join("bin/fixture-provider"), b"old provider").unwrap();
        for name in ["changed", "retained"] {
            std::fs::create_dir_all(data.join("capsules").join(name)).unwrap();
            std::fs::write(
                data.join("capsules").join(name).join("cached.wasm"),
                name.as_bytes(),
            )
            .unwrap();
        }
        let manifest = |provider_source: &Path, version: &str, changed_cid: &str| {
            serde_json::json!({
                "schema": "elastos.components/v1",
                "external": {"fixture-provider": {
                    "version": version, "install_path": "bin/fixture-provider",
                    "platforms": {(crate::setup::detect_platform()): {
                        "strategy": "local-copy", "source": provider_source,
                        "checksum": format!("sha256:{}", hex::encode(<sha2::Sha256 as sha2::Digest>::digest(std::fs::read(provider_source).unwrap()))),
                        "install_path": "bin/fixture-provider"
                    }}
                }},
                "profiles": {},
                "capsules": {
                    "changed": {"cid": changed_cid, "sha256": "a", "size": 1, "platforms": []},
                    "retained": {"cid": "retained-cid", "sha256": "b", "size": 1, "platforms": []}
                }
            })
        };
        let old = serde_json::to_vec(&manifest(&old_provider_source, "0.1.0", "old-cid")).unwrap();
        let new = serde_json::to_vec(&manifest(&new_provider_source, "0.2.0", "new-cid")).unwrap();
        std::fs::write(data.join("components.json"), &old).unwrap();
        publish_installed_fixture(&data, &binary, &source, &old);
        let executable = b"#!/bin/sh\nprintf 'elastos 0.7.1\\n'\n";
        apply_signed_fixture_with_mode(&data, &source, executable, &new, ApplyMode::Normal)
            .await
            .unwrap_err();
        assert_eq!(
            std::fs::read(data.join("bin/fixture-provider")).unwrap(),
            b"old provider"
        );
        assert_eq!(
            std::fs::read(data.join("capsules/changed/cached.wasm")).unwrap(),
            b"changed"
        );
        assert_eq!(
            std::fs::read(data.join("capsules/retained/cached.wasm")).unwrap(),
            b"retained"
        );
        assert_eq!(std::fs::read(data.join("components.json")).unwrap(), old);
        assert_eq!(
            std::fs::read(&binary).unwrap(),
            b"#!/bin/sh\nprintf 'elastos 0.7.0\\n'\n"
        );
        assert_eq!(
            std::fs::read(data.join("owner-data")).unwrap(),
            b"owner data"
        );
        assert_eq!(
            load_trusted_sources(&data)
                .unwrap()
                .default_source()
                .unwrap()
                .installed_version,
            "0.7.0"
        );
        assert!(!InstallTransaction::has_pending_recovery(&binary));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn staged_executable_refusals_preserve_installation_and_allow_corrected_retry() {
        let correct = b"#!/bin/sh\nprintf 'elastos 0.7.1\\n'\n";
        let components = b"{\"schema\":\"elastos.components/v1\",\"external\":{},\"profiles\":{},\"capsules\":{}}\n";
        let cases = [
            (
                "wrong version",
                b"#!/bin/sh\nprintf 'elastos 0.7.0\\n'\n".as_slice(),
                "version mismatch",
            ),
            (
                "version substring",
                b"#!/bin/sh\nprintf 'elastos 0.7.10\\n'\n".as_slice(),
                "version mismatch",
            ),
            (
                "prefixed version",
                b"#!/bin/sh\nprintf 'prefix elastos 0.7.1\\n'\n".as_slice(),
                "version mismatch",
            ),
            (
                "stderr-only version",
                b"#!/bin/sh\nprintf 'elastos 0.7.1\\n' >&2\n".as_slice(),
                "version mismatch",
            ),
            (
                "nonzero exit with matching stdout",
                b"#!/bin/sh\nprintf 'elastos 0.7.1\\n'\nexit 23\n".as_slice(),
                "version check failed",
            ),
            (
                "loader refusal",
                b"#!/nonexistent-elastos-fixture-interpreter\n".as_slice(),
                "failed to run installed binary",
            ),
            (
                "success with stderr",
                b"#!/bin/sh\nprintf 'elastos 0.7.1\\n'\nprintf 'unexpected diagnostic\\n' >&2\n"
                    .as_slice(),
                "version mismatch",
            ),
            (
                "hung version probe",
                b"#!/bin/sh\nexec sleep 60\n".as_slice(),
                "version check timed out",
            ),
            (
                "child retains version pipes",
                b"#!/bin/sh\nsleep 60 &\nprintf 'elastos 0.7.1\\n'\nexit 0\n".as_slice(),
                "version check timed out",
            ),
            (
                "excessive version output",
                b"#!/bin/sh\nprintf '%05000d' 0\n".as_slice(),
                "output limit",
            ),
        ];
        for mode in [ApplyMode::Normal, ApplyMode::FrozenOffline] {
            for (name, executable, error_marker) in cases {
                let name = format!(
                    "{}/{name}",
                    match mode {
                        ApplyMode::Normal => "normal",
                        ApplyMode::FrozenOffline => "frozen offline",
                    }
                );
                let fixture = tempfile::tempdir().unwrap();
                let data = fixture.path().join("data");
                let binary = fixture.path().join("bin/elastos");
                std::fs::create_dir_all(&data).unwrap();
                std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
                let old_binary = b"#!/bin/sh\nprintf 'elastos 0.7.0\\n'\n";
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    binary.parent().unwrap(),
                    std::fs::Permissions::from_mode(0o755),
                )
                .unwrap();
                std::fs::write(&binary, old_binary).unwrap();
                std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
                let old_components = b"{\"schema\":\"elastos.components/v1\",\"external\":{},\"profiles\":{},\"capsules\":{}}";
                std::fs::write(data.join("components.json"), old_components).unwrap();
                let did = crate::crypto::encode_signing_key_did(
                    &ed25519_dalek::SigningKey::from_bytes(&[7; 32]),
                );
                let source: TrustedSource = serde_json::from_value(serde_json::json!({
                    "name": "fixture", "publisher_dids": [did], "channel": "stable",
                    "installed_version": "0.7.0", "install_path": binary
                }))
                .unwrap();
                let mut sources = crate::sources::TrustedSourcesConfig::empty();
                sources.upsert_source(source.clone());
                save_trusted_sources(&data, &sources).unwrap();
                publish_installed_fixture(&data, &binary, &source, old_components);
                let old_sources = std::fs::read(data.join("sources.json")).unwrap();
                let assert_cleanup = || {
                    assert!(
                        !InstallTransaction::has_pending_recovery(&binary),
                        "{name}: journal retained"
                    );
                    match mode {
                        ApplyMode::Normal => {
                            assert!(
                                !binary
                                    .parent()
                                    .unwrap()
                                    .join(".elastos.upgrade.tmp")
                                    .exists(),
                                "{name}: binary temp retained"
                            );
                            assert!(
                                !data.join(".components.upgrade.tmp").exists(),
                                "{name}: components temp retained"
                            );
                        }
                        ApplyMode::FrozenOffline => {
                            for parent in [
                                binary.parent().unwrap().to_path_buf(),
                                data.clone(),
                                installation_release_head_path(&data)
                                    .parent()
                                    .unwrap()
                                    .to_path_buf(),
                                installation_release_manifest_path(&data)
                                    .parent()
                                    .unwrap()
                                    .to_path_buf(),
                            ] {
                                for scratch in [".elastos.update-stage", ".elastos.update-rollback"]
                                {
                                    assert!(
                                        !parent.join(scratch).exists(),
                                        "{name}: {scratch} retained"
                                    );
                                }
                            }
                        }
                    }
                };
                let preserved = [
                    ("config.json", b"owner configuration\n".as_slice()),
                    ("user-data.txt", b"owner data\n".as_slice()),
                ];
                for (path, bytes) in preserved {
                    std::fs::write(data.join(path), bytes).unwrap();
                }

                let error =
                    apply_signed_fixture_with_mode(&data, &source, executable, components, mode)
                        .await
                        .unwrap_err();
                assert!(
                    error.to_string().contains(error_marker),
                    "{name}: {error:#}"
                );
                assert_eq!(std::fs::read(&binary).unwrap(), old_binary, "{name}");
                let restored = std::process::Command::new(&binary)
                    .arg("--version")
                    .output()
                    .unwrap();
                assert!(
                    restored.status.success(),
                    "{name}: previous executable failed"
                );
                assert_eq!(restored.stdout, b"elastos 0.7.0\n", "{name}");
                assert_eq!(
                    std::fs::read(data.join("components.json")).unwrap(),
                    old_components,
                    "{name}"
                );
                assert_eq!(
                    std::fs::read(data.join("sources.json")).unwrap(),
                    old_sources,
                    "{name}"
                );
                assert_cleanup();
                for (path, bytes) in preserved {
                    assert_eq!(
                        std::fs::read(data.join(path)).unwrap(),
                        bytes,
                        "{name}: {path}"
                    );
                }

                apply_signed_fixture_with_mode(&data, &source, correct, components, mode)
                    .await
                    .unwrap_or_else(|error| panic!("{name}: corrected retry failed: {error:#}"));
                assert_eq!(std::fs::read(&binary).unwrap(), correct, "{name}");
                assert_eq!(
                    std::fs::read(data.join("components.json")).unwrap(),
                    components,
                    "{name}"
                );
                let updated = load_trusted_sources(&data).unwrap();
                let updated_source = updated.default_source().unwrap();
                assert_eq!(updated_source.installed_version, "0.7.1", "{name}");
                assert_eq!(
                    PathBuf::from(&updated_source.install_path),
                    std::fs::canonicalize(binary.parent().unwrap())
                        .unwrap()
                        .join("elastos"),
                    "{name}"
                );
                assert_cleanup();
                for (path, bytes) in preserved {
                    assert_eq!(
                        std::fs::read(data.join(path)).unwrap(),
                        bytes,
                        "{name}: {path}"
                    );
                }
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn offline_signed_update_refusals_preserve_all_owner_files() {
        let old = b"{\"schema\":\"elastos.components/v1\",\"external\":{},\"profiles\":{},\"capsules\":{}}";
        let changed = b"{\"schema\":\"elastos.components/v1\",\"external\":{},\"profiles\":{\"home\":{\"components\":[]}},\"capsules\":{}}";
        let executable = b"#!/bin/sh\nprintf 'elastos 0.7.1\\n'\n";
        for (components, active_host, expected) in [
            (b"invalid JSON".as_slice(), false, "expected value"),
            (changed.as_slice(), false, "support change"),
            (old.as_slice(), true, "offline update requires"),
        ] {
            let fixture = tempfile::tempdir().unwrap();
            let data = fixture.path().join("data");
            let binary = fixture.path().join("bin/elastos");
            std::fs::create_dir_all(&data).unwrap();
            std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
            std::fs::write(&binary, b"previous runtime").unwrap();
            std::fs::write(data.join("components.json"), old).unwrap();
            std::fs::write(data.join("owner-data"), b"owner private data").unwrap();
            let did = crate::crypto::encode_signing_key_did(
                &ed25519_dalek::SigningKey::from_bytes(&[7; 32]),
            );
            let source: TrustedSource = serde_json::from_value(serde_json::json!({
                "name": "fixture", "publisher_dids": [did], "channel": "stable", "installed_version": "0.7.0", "install_path": binary
            })).unwrap();
            let mut sources = crate::sources::TrustedSourcesConfig::empty();
            sources.upsert_source(source.clone());
            save_trusted_sources(&data, &sources).unwrap();
            publish_installed_fixture(&data, &binary, &source, old);
            let old_head = std::fs::read(installation_release_head_path(&data)).unwrap();
            let source_bytes = std::fs::read(data.join("sources.json")).unwrap();
            let _host = if active_host {
                Some(
                    crate::host_lock::acquire_host_process_lock(&data, "fixture", "offline")
                        .unwrap(),
                )
            } else {
                None
            };
            let error = apply_signed_executable_fixture(&data, &source, executable, components)
                .await
                .unwrap_err();
            assert!(format!("{error:#}").contains(expected), "{error:#}");
            assert_eq!(std::fs::read(&binary).unwrap(), b"previous runtime");
            assert_eq!(std::fs::read(data.join("components.json")).unwrap(), old);
            assert_eq!(
                std::fs::read(data.join("sources.json")).unwrap(),
                source_bytes
            );
            assert_eq!(
                std::fs::read(data.join("owner-data")).unwrap(),
                b"owner private data"
            );
            assert_eq!(
                std::fs::read(installation_release_head_path(&data)).unwrap(),
                old_head
            );
            assert!(!InstallTransaction::has_pending_recovery(&binary));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn normal_post_activation_owner_refusal_restores_all_five_original_inputs() {
        let components =
            br#"{"schema":"elastos.components/v1","external":{},"profiles":{},"capsules":{}}"#;
        let (_fixture, data, binary, source) = default_apply_fixture(components);
        let auth = crate::auth::auth_state_path(&data).unwrap();
        std::fs::create_dir_all(auth.parent().unwrap()).unwrap();
        std::fs::write(&auth, b"invalid owner state").unwrap();
        let publisher = elastos_common::localhost::publisher_release_head_path(&data);
        std::fs::create_dir_all(publisher.parent().unwrap()).unwrap();
        std::fs::write(&publisher, b"Publisher publication sentinel").unwrap();
        let preserved = [
            binary.clone(),
            data.join("components.json"),
            data.join("sources.json"),
            installation_release_head_path(&data),
            installation_release_manifest_path(&data),
        ]
        .map(|path| {
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        });
        let error = apply_signed_fixture_with_mode(
            &data,
            &source,
            b"#!/bin/sh\nprintf 'elastos 0.7.1\\n'\n",
            components,
            ApplyMode::Normal,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("original release files restored"),
            "{error:#}"
        );
        for (path, bytes) in preserved {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
        assert_eq!(std::fs::read(&auth).unwrap(), b"invalid owner state");
        assert_eq!(
            std::fs::read(&publisher).unwrap(),
            b"Publisher publication sentinel"
        );
        assert!(!InstallTransaction::has_pending_recovery(&binary));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn legacy_candidate_and_support_refusals_preserve_absent_consumed_custody() {
        let components =
            br#"{"schema":"elastos.components/v1","external":{},"profiles":{},"capsules":{}}"#;
        let support_change = br#"{"schema":"elastos.components/v1","external":{},"profiles":{"home":{"components":[]}},"capsules":{}}"#;
        for (mode, executable, candidate_components, expected) in [
            (
                ApplyMode::Normal,
                b"#!/bin/sh\nprintf 'elastos 0.7.0\\n'\n".as_slice(),
                components.as_slice(),
                "version mismatch",
            ),
            (
                ApplyMode::FrozenOffline,
                b"#!/bin/sh\nprintf 'elastos 0.7.0\\n'\n".as_slice(),
                components.as_slice(),
                "version mismatch",
            ),
            (
                ApplyMode::FrozenOffline,
                b"#!/bin/sh\nprintf 'elastos 0.7.1\\n'\n".as_slice(),
                support_change.as_slice(),
                "support change",
            ),
        ] {
            let (_fixture, data, binary, source) = default_apply_fixture(components);
            let publisher = elastos_common::localhost::publisher_release_head_path(&data)
                .parent()
                .unwrap()
                .to_path_buf();
            std::fs::create_dir_all(publisher.parent().unwrap()).unwrap();
            std::fs::rename(data.join("installation"), &publisher).unwrap();
            let preserved = [
                binary.clone(),
                data.join("components.json"),
                data.join("sources.json"),
                publisher.join("release-head.json"),
                publisher.join("release.json"),
            ]
            .map(|path| {
                let bytes = std::fs::read(&path).unwrap();
                (path, bytes)
            });
            let error = apply_signed_fixture_with_mode(
                &data,
                &source,
                executable,
                candidate_components,
                mode,
            )
            .await
            .unwrap_err();
            assert!(format!("{error:#}").contains(expected), "{error:#}");
            for (path, bytes) in preserved {
                assert_eq!(std::fs::read(path).unwrap(), bytes);
            }
            assert!(!data.join("installation").exists());
            assert!(!data.join(".elastos.installation-migrate").exists());
            assert!(!InstallTransaction::has_pending_recovery(&binary));
            for entry in std::fs::read_dir(binary.parent().unwrap()).unwrap() {
                assert!(!entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".elastos.candidate-check-"));
            }
        }
    }

    #[test]
    fn test_verify_installed_binary_version_accepts_matching_output() {
        let bin = Path::new("/tmp/mock-elastos");
        verify_installed_binary_version_output(bin, "0.1.0", true, b"elastos 0.1.0\n", b"")
            .unwrap();
    }

    #[test]
    fn test_verify_installed_binary_version_rejects_mismatch() {
        let bin = Path::new("/tmp/mock-elastos");
        let err =
            verify_installed_binary_version_output(bin, "0.1.0", true, b"elastos 0.0.9\n", b"")
                .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(&bin.display().to_string()));
        assert!(msg.contains("Installed binary version mismatch"));
        assert!(msg.contains("Expected: 0.1.0"));
        assert!(msg.contains("elastos 0.0.9"));
    }

    #[test]
    fn test_verify_installed_binary_version_requires_exact_stdout() {
        let bin = Path::new("/tmp/mock-elastos");
        for stdout in [
            b"elastos 0.1.00\n".as_slice(),
            b"elastos 0.1.0-extra\n".as_slice(),
            b"other 0.1.0\n".as_slice(),
            b"prefix elastos 0.1.0\n".as_slice(),
            b"elastos 0.1.0\nextra\n".as_slice(),
            b"\xffelastos 0.1.0\n".as_slice(),
            b"".as_slice(),
        ] {
            assert!(
                verify_installed_binary_version_output(
                    bin,
                    "0.1.0",
                    true,
                    stdout,
                    b"elastos 0.1.0\n",
                )
                .is_err(),
                "unexpected version output: {stdout:?}",
            );
        }
    }

    #[test]
    fn test_verify_installed_binary_version_rejects_failed_invocation() {
        let bin = Path::new("/tmp/mock-elastos");
        let err =
            verify_installed_binary_version_output(bin, "0.1.0", false, b"", b"permission denied")
                .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(&bin.display().to_string()));
        assert!(msg.contains("Installed binary version check failed"));
        assert!(msg.contains("permission denied"));
    }

    #[tokio::test]
    async fn test_fetch_cid_via_gateways_uses_ipfs_path() {
        async fn handler(AxumPath(cid): AxumPath<String>) -> (StatusCode, Body) {
            assert_eq!(cid, "bafy-test-cid");
            (StatusCode::OK, Body::from("gateway-bytes"))
        }

        let app = Router::new().route("/ipfs/:cid", get(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let bytes = fetch_cid_via_gateways("bafy-test-cid", &[format!("http://{}", addr)])
            .await
            .unwrap();
        assert_eq!(bytes, b"gateway-bytes");
    }

    #[tokio::test]
    async fn test_fetch_release_manifest_via_gateway_uses_release_json_path() {
        async fn handler() -> (StatusCode, Body) {
            (StatusCode::OK, Body::from("release-manifest"))
        }

        let app = Router::new().route("/release.json", get(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let bytes = fetch_release_manifest_via_gateway(&format!("http://{}", addr))
            .await
            .unwrap();
        assert_eq!(bytes, b"release-manifest");
    }

    /// install.sh admission and recovery through the shared installation writer.
    #[cfg(unix)]
    mod installer {
        use super::*;
        use std::collections::BTreeMap;
        use std::os::unix::fs::PermissionsExt;

        fn envelope(seed: u8, payload: serde_json::Value, domain: &str) -> Vec<u8> {
            let key = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
            let (signature, signer_did) = crate::crypto::domain_separated_sign(
                &key,
                domain,
                &serde_json::to_vec(&payload).unwrap(),
            );
            serde_json::to_vec(&serde_json::json!({
                "payload": payload, "signature": signature, "signer_did": signer_did
            }))
            .unwrap()
        }

        fn did(seed: u8) -> String {
            crate::crypto::encode_signing_key_did(&ed25519_dalek::SigningKey::from_bytes(
                &[seed; 32],
            ))
        }

        fn runtime(version: &str) -> Vec<u8> {
            format!("#!/bin/sh\nprintf 'elastos {version}\\n'\n").into_bytes()
        }

        /// The signed pair and source install.sh hands to the writer.
        fn candidate(root: &Path, binary: &Path, version: &str, seed: u8) -> [PathBuf; 4] {
            use sha2::Digest;
            let executable = runtime(version);
            let release = envelope(
                seed,
                serde_json::json!({
                    "schema": "elastos.release/v1", "version": version, "channel": "stable",
                    "platforms": {(detect_release_platform()): {"binary": {
                        "cid": raw_cid(&executable), "size": executable.len(),
                        "sha256": hex::encode(sha2::Sha256::digest(&executable))
                    }}}
                }),
                "elastos.release.v1",
            );
            let mut head = binding_head(&release);
            head["version"] = serde_json::json!(version);
            let head = envelope(seed, head, "elastos.release.head.v1");
            let sources = serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "elastos.trusted-sources/v1", "default_source": "default",
                "sources": [{"name": "default", "publisher_dids": [did(seed)], "channel": "stable",
                    "install_path": binary, "installed_version": version, "head_cid": ""}]
            }))
            .unwrap();
            let directory = root.join(format!("candidate-{version}-{seed}"));
            std::fs::create_dir_all(&directory).unwrap();
            let paths = [
                "runtime",
                "sources.json",
                "release-head.json",
                "release.json",
            ]
            .map(|name| directory.join(name));
            for (path, bytes) in paths.iter().zip([executable, sources, head, release]) {
                std::fs::write(path, bytes).unwrap();
            }
            paths
        }

        fn install(data: &Path, binary: &Path, candidate: &[PathBuf; 4]) -> anyhow::Result<()> {
            let paths = candidate.each_ref().map(PathBuf::as_path);
            install_release(data, binary, paths, true)?;
            install_release(data, binary, paths, false)
        }

        /// Stops the writer after staging, or after it activated every file.
        fn interrupt(data: &Path, binary: &Path, candidate: &[PathBuf; 4], activated: bool) {
            let bytes = candidate
                .each_ref()
                .map(|path| std::fs::read(path).unwrap());
            let transaction = InstallTransaction::acquire(data, binary).unwrap();
            transaction
                .prepare(&[
                    (ReleaseFile::RuntimeBinary, &bytes[0][..]),
                    (ReleaseFile::Sources, &bytes[1][..]),
                    (ReleaseFile::ReleaseHead, &bytes[2][..]),
                    (ReleaseFile::ReleaseManifest, &bytes[3][..]),
                ])
                .unwrap();
            if activated {
                // Unwinding skips the writer's own restore, as a killed process would.
                let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    transaction.commit_checked(|| panic!("installer stopped"))
                }));
                assert!(stopped.is_err());
                assert_eq!(std::fs::read(binary).unwrap(), bytes[0]);
            }
            assert!(InstallTransaction::has_pending_recovery(binary));
        }

        fn empty() -> (tempfile::TempDir, PathBuf, PathBuf) {
            let fixture = tempfile::tempdir().unwrap();
            let data = fixture.path().join("data");
            let binary = fixture.path().join("bin/elastos");
            std::fs::create_dir_all(&data).unwrap();
            std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
            (fixture, data, binary)
        }

        /// Every file's mode and bytes; unchanged means identical.
        fn files(root: &Path, skip_locks: bool) -> BTreeMap<PathBuf, (u32, Vec<u8>)> {
            let mut files = BTreeMap::new();
            let mut pending = vec![root.to_path_buf()];
            while let Some(directory) = pending.pop() {
                for entry in std::fs::read_dir(directory).unwrap() {
                    let path = entry.unwrap().path();
                    let metadata = std::fs::symlink_metadata(&path).unwrap();
                    if metadata.is_dir() {
                        pending.push(path);
                    } else if !(skip_locks && path.extension().is_some_and(|name| name == "lock")) {
                        files.insert(
                            path.clone(),
                            (
                                metadata.permissions().mode() & 0o7777,
                                std::fs::read(&path).unwrap(),
                            ),
                        );
                    }
                }
            }
            files
        }

        /// 0.7.0 installed and then updated to 0.7.1 by the signed update path.
        async fn migrated() -> (tempfile::TempDir, PathBuf, PathBuf) {
            let fixture = tempfile::tempdir().unwrap();
            let data = fixture.path().join("data");
            let binary = fixture.path().join("bin/elastos");
            std::fs::create_dir_all(&data).unwrap();
            std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
            std::fs::write(&binary, runtime("0.7.0")).unwrap();
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
            let components = b"{\"schema\":\"elastos.components/v1\",\"external\":{},\"profiles\":{},\"capsules\":{}}";
            std::fs::write(data.join("components.json"), components).unwrap();
            std::fs::write(data.join("user-data.txt"), b"owner data\n").unwrap();
            let source: TrustedSource = serde_json::from_value(serde_json::json!({
                "name": "fixture", "publisher_dids": [did(7)], "channel": "stable",
                "installed_version": "0.7.0", "install_path": binary
            }))
            .unwrap();
            let mut sources = crate::sources::TrustedSourcesConfig::empty();
            sources.upsert_source(source.clone());
            save_trusted_sources(&data, &sources).unwrap();
            publish_installed_fixture(&data, &binary, &source, components);
            apply_signed_executable_fixture(&data, &source, &runtime("0.7.1"), components)
                .await
                .unwrap();
            (fixture, data, binary)
        }

        #[tokio::test]
        async fn older_installer_after_update_is_refused_without_changes() {
            let (fixture, data, binary) = migrated().await;
            let candidates = tempfile::tempdir().unwrap();
            let older = candidate(candidates.path(), &binary, "0.7.0", 7);
            let before = files(fixture.path(), false);
            for check in [true, false] {
                let paths = older.each_ref().map(PathBuf::as_path);
                let error = install_release(&data, &binary, paths, check).unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("older than the installed release 0.7.1"),
                    "{error:#}"
                );
                assert_eq!(files(fixture.path(), false), before);
            }
        }

        #[tokio::test]
        async fn installer_is_refused_while_a_home_update_is_pending() {
            for pending in ["restart journal", "queued request"] {
                let (fixture, data, binary) = migrated().await;
                if pending == "restart journal" {
                    use sha2::Digest;
                    let transaction = InstallTransaction::acquire(&data, &binary).unwrap();
                    transaction
                        .prepare(
                            &[
                                ReleaseFile::RuntimeBinary,
                                ReleaseFile::Components,
                                ReleaseFile::Sources,
                                ReleaseFile::ReleaseHead,
                                ReleaseFile::ReleaseManifest,
                            ]
                            .map(|id| (id, b"candidate".as_slice())),
                        )
                        .unwrap();
                    transaction
                        .prepare_restart(RestartPlan {
                            request_id: "a".repeat(32),
                            controller_sha256: "b".repeat(64),
                            launch_plan_sha256: "c".repeat(64),
                            support_sha256: "d".repeat(64),
                            previous_version: "0.7.1".into(),
                            candidate_version: "0.7.2".into(),
                            previous_binary_sha256: hex::encode(sha2::Sha256::digest(
                                std::fs::read(&binary).unwrap(),
                            )),
                            support_paths: Default::default(),
                        })
                        .unwrap();
                } else {
                    std::fs::create_dir_all(data.join("update-controller")).unwrap();
                    std::fs::write(data.join("update-controller/request.json"), b"{}").unwrap();
                }
                let candidates = tempfile::tempdir().unwrap();
                let newer = candidate(candidates.path(), &binary, "0.7.2", 7);
                let before = files(fixture.path(), false);
                for check in [true, false] {
                    let paths = newer.each_ref().map(PathBuf::as_path);
                    let error = install_release(&data, &binary, paths, check).unwrap_err();
                    assert!(
                        error
                            .to_string()
                            .contains("Home update is still in progress"),
                        "{pending}: {error:#}"
                    );
                    assert_eq!(files(fixture.path(), false), before, "{pending}");
                }
            }
        }

        #[tokio::test]
        async fn fresh_install_then_concurrent_writer_is_refused() {
            let (fixture, data, binary) = empty();
            let candidates = tempfile::tempdir().unwrap();
            let first = candidate(candidates.path(), &binary, "0.7.1", 7);
            install(&data, &binary, &first).unwrap();
            assert_eq!(std::fs::read(&binary).unwrap(), runtime("0.7.1"));
            assert!(!data.join("components.json").exists());
            let installation = installation_release_head_path(&data);
            assert_eq!(
                std::fs::metadata(installation.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            for (path, source) in [
                (installation, &first[2]),
                (installation_release_manifest_path(&data), &first[3]),
                (data.join("sources.json"), &first[1]),
            ] {
                assert_eq!(
                    std::fs::read(&path).unwrap(),
                    std::fs::read(source).unwrap()
                );
                assert_eq!(
                    std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }

            let next = candidate(candidates.path(), &binary, "0.7.2", 7);
            let writer = crate::install_transaction::InstallationGuard::acquire(
                &std::fs::canonicalize(binary.parent().unwrap()).unwrap(),
            )
            .unwrap();
            let before = files(fixture.path(), false);
            for check in [true, false] {
                let paths = next.each_ref().map(PathBuf::as_path);
                let error = install_release(&data, &binary, paths, check).unwrap_err();
                assert!(
                    format!("{error:#}").contains("another writer owns the installation lock"),
                    "{error:#}"
                );
                assert_eq!(files(fixture.path(), false), before);
            }
            drop(writer);
            install(&data, &binary, &next).unwrap();
            assert_eq!(std::fs::read(&binary).unwrap(), runtime("0.7.2"));
        }

        /// A 0.7.x Home: legacy publisher pair, Runtime pinned to the old key, user data.
        fn legacy() -> (tempfile::TempDir, PathBuf, PathBuf) {
            use elastos_common::localhost::{
                publisher_release_head_path, publisher_release_manifest_path,
            };
            let fixture = tempfile::tempdir().unwrap();
            let data = fixture.path().join("data");
            let binary = fixture.path().join("bin/elastos");
            std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
            std::fs::write(&binary, runtime("0.7.0")).unwrap();
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
            let legacy = candidate(fixture.path(), &binary, "0.7.0", 7);
            for (path, bytes) in [
                (
                    data.join("sources.json"),
                    std::fs::read(&legacy[1]).unwrap(),
                ),
                (
                    publisher_release_head_path(&data),
                    std::fs::read(&legacy[2]).unwrap(),
                ),
                (
                    publisher_release_manifest_path(&data),
                    std::fs::read(&legacy[3]).unwrap(),
                ),
                (data.join("components.json"), b"{\"note\":\"old\"}".to_vec()),
                (
                    data.join("identity/device.key"),
                    b"device identity".to_vec(),
                ),
                (
                    data.join("auth-state.json"),
                    b"{\"accounts\":[\"alice\"]}".to_vec(),
                ),
                (
                    data.join("Users/alice/notes.txt"),
                    b"user files stay\n".to_vec(),
                ),
            ] {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, bytes).unwrap();
            }
            (fixture, data, binary)
        }

        #[tokio::test]
        async fn legacy_first_hop_retrusts_only_through_installer_and_survives_interruption() {
            let (fixture, data, binary) = legacy();
            let candidates = tempfile::tempdir().unwrap();
            let new_key = candidate(candidates.path(), &binary, "0.7.1", 9);
            let old_install = files(fixture.path(), true);

            // `elastos update` keeps refusing the new key and names install.sh.
            let head = std::fs::read(&new_key[2]).unwrap();
            let head_cid = raw_cid(&head);
            let fetch: FetchFn = Box::new(move |_, _| {
                let bytes = head.clone();
                Box::pin(async move { Ok(bytes) })
            });
            let error = run_update_for_data_dir(
                &data,
                &fetch,
                None,
                true,
                Some(head_cid),
                true,
                Vec::new(),
                "0.7.0",
                true,
                false,
            )
            .await
            .unwrap_err()
            .to_string();
            assert!(error.contains("Signer DID mismatch"), "{error}");
            assert!(error.contains("Run the publisher's install.sh"), "{error}");
            assert_eq!(files(fixture.path(), false), old_install);

            // The installer stops after activating; recovery leaves the old Home working.
            interrupt(&data, &binary, &new_key, true);
            recover_pending_installation(&data).unwrap();
            assert_eq!(files(fixture.path(), true), old_install);
            let version = std::process::Command::new(&binary)
                .arg("--version")
                .output()
                .unwrap();
            assert_eq!(version.stdout, b"elastos 0.7.0\n");

            // Rerunning install.sh upgrades, re-trusts and keeps every user file.
            install(&data, &binary, &new_key).unwrap();
            assert_eq!(std::fs::read(&binary).unwrap(), runtime("0.7.1"));
            let source = load_trusted_sources(&data).unwrap();
            assert_eq!(source.default_source().unwrap().publisher_dids, [did(9)]);
            assert_eq!(
                std::fs::read(installation_release_head_path(&data)).unwrap(),
                std::fs::read(&new_key[2]).unwrap()
            );
            let after = files(fixture.path(), true);
            let changed = old_install
                .keys()
                .chain(after.keys())
                .filter(|path| old_install.get(*path) != after.get(*path))
                .map(|path| path.strip_prefix(fixture.path()).unwrap().to_path_buf())
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(
                changed,
                [
                    "bin/elastos",
                    "data/sources.json",
                    "data/installation/release-head.json",
                    "data/installation/release.json",
                ]
                .into_iter()
                .map(PathBuf::from)
                .collect()
            );
        }

        #[test]
        fn interrupted_fresh_install_blocks_home_and_installer_rerun_completes() {
            for activated in [false, true] {
                let (_fixture, data, binary) = empty();
                let candidates = tempfile::tempdir().unwrap();
                let first = candidate(candidates.path(), &binary, "0.7.1", 7);
                interrupt(&data, &binary, &first, activated);
                let error = crate::install_transaction::authorize_host_start(&data, &binary)
                    .unwrap_err()
                    .to_string();
                assert!(error.contains("Run install.sh again"), "{error}");
                install(&data, &binary, &first).unwrap();
                assert_eq!(std::fs::read(&binary).unwrap(), runtime("0.7.1"));
                assert!(!InstallTransaction::has_pending_recovery(&binary));
                crate::install_transaction::authorize_host_start(&data, &binary).unwrap();
            }
        }

        #[test]
        fn existing_runtime_without_a_release_record_is_refused() {
            for record in [
                None,
                Some(b"not json".as_slice()),
                Some(br#"{"schema":"elastos.trusted-sources/v1","sources":[{"name":"default"}]}"#),
            ] {
                let (fixture, data, binary) = empty();
                std::fs::write(&binary, runtime("0.7.2")).unwrap();
                if let Some(record) = record {
                    std::fs::write(data.join("sources.json"), record).unwrap();
                }
                let candidates = tempfile::tempdir().unwrap();
                let older = candidate(candidates.path(), &binary, "0.7.1", 7);
                let before = files(fixture.path(), true);
                for check in [true, false] {
                    let paths = older.each_ref().map(PathBuf::as_path);
                    let error = install_release(&data, &binary, paths, check).unwrap_err();
                    assert!(error.to_string().contains("release record"), "{error:#}");
                    assert_eq!(files(fixture.path(), true), before);
                }
            }
        }

        #[test]
        fn tampered_or_foreign_signed_candidates_are_refused_without_changes() {
            let (fixture, data, binary) = empty();
            let candidates = tempfile::tempdir().unwrap();
            install(
                &data,
                &binary,
                &candidate(candidates.path(), &binary, "0.7.1", 7),
            )
            .unwrap();
            for (case, expected) in [
                ("tampered binary", "differs from its signed checksum"),
                ("foreign release signer", "Signer DID mismatch"),
                ("foreign head signer", "Signer DID mismatch"),
            ] {
                let next = candidate(candidates.path(), &binary, "0.7.2", 7);
                let foreign = candidate(candidates.path(), &binary, "0.7.2", 9);
                match case {
                    "tampered binary" => std::fs::write(&next[0], runtime("0.7.3")).unwrap(),
                    "foreign release signer" => {
                        std::fs::copy(&foreign[3], &next[3]).map(drop).unwrap()
                    }
                    _ => std::fs::copy(&foreign[2], &next[2]).map(drop).unwrap(),
                }
                let before = files(fixture.path(), false);
                for check in [true, false] {
                    let paths = next.each_ref().map(PathBuf::as_path);
                    let error = install_release(&data, &binary, paths, check).unwrap_err();
                    assert!(format!("{error:#}").contains(expected), "{case}: {error:#}");
                    assert_eq!(files(fixture.path(), false), before, "{case}");
                }
            }
        }
    }
}
