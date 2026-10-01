//! Update/upgrade logic: Carrier-first release fetch, explicit transport
//! overrides, platform detection, cache management, and the main update flow.
//!
//! CLI and operator entry points share signed head discovery and admission.

use std::cmp::Ordering;
use std::future::Future;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use elastos_common::localhost::{publisher_release_head_path, publisher_release_manifest_path};

use crate::crypto::{verify_release_envelope, verify_release_envelope_against_dids};
use crate::sources::{
    default_data_dir, default_install_path, load_trusted_sources, normalize_gateways,
    save_trusted_sources, TrustedSource,
};

/// Async callback for fetching content by CID from the trusted source.
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
        .await?
        .ok_or_else(|| anyhow::anyhow!("Trusted source returned no release head"))?;
    resolve_discovered_head(&announcement, source, |cid| async move {
        match cid {
            Some(cid) => client.fetch_content(&cid, None).await,
            None => client.fetch_file("release-head.json").await,
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
    let head = verify_release_envelope(&bytes, "elastos.release.head.v1", publisher)?;
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

/// An empty installed version is the legacy first-install state. Every
/// nonempty version must have exact SemVer syntax before it is compared.
pub(crate) fn parse_installed_release_version(
    installed: &str,
) -> anyhow::Result<Option<semver::Version>> {
    if installed.is_empty() {
        return Ok(None);
    }
    semver::Version::parse(installed).map(Some).map_err(|err| {
        anyhow::anyhow!(
            "Invalid installed release version '{installed}': {err}. Repair: run 'elastos update --force' locally on this Home to install the signed release and restore installed_version"
        )
    })
}

pub fn compare_release_versions(installed: &str, offered: &str) -> anyhow::Result<Ordering> {
    let offered = semver::Version::parse(offered)
        .map_err(|err| anyhow::anyhow!("Invalid signed release version '{offered}': {err}"))?;
    let Some(installed) = parse_installed_release_version(installed)? else {
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

fn verify_installed_binary_version(bin_path: &Path, expected_version: &str) -> anyhow::Result<()> {
    let output = std::process::Command::new(bin_path)
        .arg("--version")
        .output()
        .map_err(|e| {
            anyhow::anyhow!(
                "failed to run installed binary {}: {}",
                bin_path.display(),
                e
            )
        })?;

    verify_installed_binary_version_output(
        bin_path,
        expected_version,
        output.status.success(),
        &output.stdout,
        &output.stderr,
    )
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

    if !combined.contains(expected_version) {
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
    let head = verify_release_envelope(&head_bytes, "elastos.release.head.v1", &primary_publisher)?;

    let head_version = head["payload"]["version"].as_str().unwrap_or("unknown");
    verify_source_channel(&source, head["payload"]["channel"].as_str().unwrap_or(""))?;
    let release_cid = head["payload"]["latest_release_cid"].as_str().unwrap_or("");
    let release_object_cid = optional_release_object_cid(&head)?;

    run_upgrade_from_head(
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
    )
    .await
}

/// Bind a verified release envelope to the exact bytes chosen by its signed head.
/// This digest is additional envelope evidence; the CID remains content identity.
fn verify_release_binding(
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
    )?;
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

    let comparison_version = if force
        && !current_version.is_empty()
        && semver::Version::parse(current_version).is_err()
    {
        println!("  Repairing invalid installed release version '{current_version}' with the signed release.");
        ""
    } else {
        current_version
    };
    let version_order = compare_release_versions(comparison_version, version)?;
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
    let binary_data = fetch_fn(binary_cid.to_string(), ordered_gateways.to_vec()).await?;
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
    let comp_data = fetch_fn(comp_cid.to_string(), ordered_gateways.to_vec()).await?;
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
    crate::setup::validate_update_components_manifest(
        &comp_data,
        &crate::setup::detect_platform(),
    )?;

    // 10. Atomic replace binary
    let bin_path = if source.install_path.is_empty() {
        default_install_path()
    } else {
        PathBuf::from(&source.install_path)
    };
    let bin_dir = bin_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let tmp_bin = bin_dir.join(".elastos.upgrade.tmp");

    std::fs::create_dir_all(&bin_dir)?;
    std::fs::write(&tmp_bin, &binary_data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_bin, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp_bin, &bin_path)?;
    println!("  Installed binary: {}", bin_path.display());
    verify_installed_binary_version(&bin_path, version)?;
    println!("  Installed binary verified (version ✓)");

    // 11. Atomic replace components.json
    let comp_path = data_dir.join("components.json");
    let old_components = std::fs::read(&comp_path).ok();
    let changed_capsules =
        changed_capsule_names(old_components.as_deref(), &comp_data).unwrap_or_default();
    let tmp_comp = data_dir.join(".components.upgrade.tmp");
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(&tmp_comp, &comp_data)?;
    std::fs::rename(&tmp_comp, &comp_path)?;
    println!("  Installed components: {}", comp_path.display());

    let refreshed_components = crate::setup::refresh_installed_components_for_update(
        data_dir,
        old_components.as_deref(),
        &comp_data,
        &component_platform,
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
        crate::api::auth_gateway::migrate_configured_principal_roots_offline(
            data_dir,
            &principal_root_backup,
        )?;
    println!(
        "  Principal-root readiness: {} ({} object(s))",
        principal_root_receipt.status, principal_root_receipt.object_count
    );

    // 13. Save new state
    if let Some(parent) = publisher_release_head_path(data_dir).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(publisher_release_head_path(data_dir), head_bytes)?;
    std::fs::write(publisher_release_manifest_path(data_dir), &release_bytes)?;

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
    save_trusted_sources(data_dir, &sources)?;

    println!();
    println!("  ElastOS {} installed successfully!", version);
    println!();

    Ok(())
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

    #[tokio::test]
    async fn update_admission_refusals_preserve_binary_manifest_and_source() {
        for (installed, force, checksum, strategy, expected) in [
            (
                "unknown",
                false,
                Some(format!("sha256:{}", "a".repeat(64))),
                None,
                "elastos update --force",
            ),
            (
                "0.7.0",
                false,
                None,
                None,
                "requires a SHA-256 or SHA-512 checksum",
            ),
            (
                "unknown",
                true,
                None,
                None,
                "requires a SHA-256 or SHA-512 checksum",
            ),
            (
                "0.7.0",
                false,
                Some("sha256:bad".to_string()),
                None,
                "requires a SHA-256 or SHA-512 checksum",
            ),
            (
                "0.7.0",
                false,
                Some(format!("sha256:{}", "a".repeat(64))),
                Some("local-copy"),
                "development installation strategy",
            ),
            (
                "0.7.0",
                false,
                Some(format!("sha256:{}", "a".repeat(64))),
                Some("source-build"),
                "development installation strategy",
            ),
        ] {
            let components = serde_json::to_vec(&serde_json::json!({
                "external": {"model-provider": {"platforms": {
                    (crate::setup::detect_platform()): {"checksum": checksum, "strategy": strategy}
                }}}, "profiles": {}
            }))
            .unwrap();
            let (data, source, head, head_bytes, release_cid, fetch) =
                admission_fixture(installed, components);
            let before_source = std::fs::read(data.path().join("sources.json")).unwrap();
            let result = run_upgrade_from_head(
                &fetch,
                &head,
                &head_bytes,
                None,
                "0.7.1",
                &release_cid,
                None,
                installed,
                &source,
                data.path(),
                false,
                &[],
                true,
                force,
                "fixture",
                None,
            )
            .await
            .unwrap_err();
            assert!(result.to_string().contains(expected), "{result:#}");
            assert_eq!(std::fs::read(&source.install_path).unwrap(), b"old binary");
            assert_eq!(
                std::fs::read(data.path().join("components.json")).unwrap(),
                b"{ \"external\": {}, \"profiles\": {} }"
            );
            assert_eq!(
                std::fs::read(data.path().join("sources.json")).unwrap(),
                before_source
            );
        }
    }

    #[test]
    fn update_manifest_admission_accepts_checksums_on_alias_and_wildcard_platforms() {
        for platform_key in ["x86_64-linux", "*"] {
            for checksum in [
                format!("sha256:{}", "a".repeat(64)),
                format!("sha512:{}", "A".repeat(128)),
            ] {
                let manifest = serde_json::to_vec(&serde_json::json!({
                    "external": {"provider": {"platforms": {
                        (platform_key): {"checksum": checksum},
                        "darwin-arm64": {"strategy": "source-build"}
                    }}}, "profiles": {}
                }))
                .unwrap();
                crate::setup::validate_update_components_manifest(&manifest, "linux-amd64")
                    .unwrap();
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn forced_signed_update_repairs_invalid_installed_version() {
        let components = b"{\"external\":{},\"profiles\":{}}".to_vec();
        let (data, source, head, head_bytes, release_cid, fetch) =
            admission_fixture("unknown", components.clone());
        run_upgrade_from_head(
            &fetch,
            &head,
            &head_bytes,
            None,
            "0.7.1",
            &release_cid,
            None,
            "unknown",
            &source,
            data.path(),
            false,
            &[],
            true,
            true,
            "fixture",
            None,
        )
        .await
        .unwrap();
        let sources = load_trusted_sources(data.path()).unwrap();
        assert_eq!(sources.default_source().unwrap().installed_version, "0.7.1");
        assert_eq!(
            std::fs::read(data.path().join("components.json")).unwrap(),
            components
        );
        assert_eq!(
            std::fs::read(&source.install_path).unwrap(),
            b"#!/bin/sh\nprintf 'elastos 0.7.1\\n'\n"
        );
    }

    fn admission_fixture(
        installed: &str,
        components: Vec<u8>,
    ) -> (
        tempfile::TempDir,
        TrustedSource,
        serde_json::Value,
        Vec<u8>,
        String,
        FetchFn,
    ) {
        use sha2::Digest;
        let data = tempfile::tempdir().unwrap();
        let binary = b"#!/bin/sh\nprintf 'elastos 0.7.1\\n'\n".to_vec();
        let release = binding_envelope(
            serde_json::json!({
                "schema": "elastos.release/v1", "version": "0.7.1", "channel": "stable",
                "platforms": {(detect_release_platform()): {
                    "binary": {"cid": "binary", "sha256": hex::encode(sha2::Sha256::digest(&binary))},
                    "components": {"cid": "components", "sha256": hex::encode(sha2::Sha256::digest(&components))}
                }}
            }),
            "elastos.release.v1",
        );
        let head_bytes = binding_envelope(binding_head(&release), "elastos.release.head.v1");
        let did =
            crate::crypto::encode_signing_key_did(&ed25519_dalek::SigningKey::from_bytes(&[7; 32]));
        let head = verify_release_envelope(&head_bytes, "elastos.release.head.v1", &did).unwrap();
        let source: TrustedSource = serde_json::from_value(serde_json::json!({
            "name": "fixture", "publisher_dids": [did], "channel": "stable",
            "installed_version": installed, "install_path": data.path().join("elastos")
        }))
        .unwrap();
        let mut sources = crate::sources::TrustedSourcesConfig::empty();
        sources.upsert_source(source.clone());
        save_trusted_sources(data.path(), &sources).unwrap();
        std::fs::write(&source.install_path, b"old binary").unwrap();
        std::fs::write(
            data.path().join("components.json"),
            b"{ \"external\": {}, \"profiles\": {} }",
        )
        .unwrap();
        let release_cid = raw_cid(&release);
        let requested_release = release_cid.clone();
        let fetch: FetchFn = Box::new(move |cid, _| {
            let bytes = if cid == requested_release {
                release.clone()
            } else if cid == "binary" {
                binary.clone()
            } else if cid == "components" {
                components.clone()
            } else {
                panic!("unexpected artifact request")
            };
            Box::pin(async move { Ok(bytes) })
        });
        (data, source, head, head_bytes, release_cid, fetch)
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
}
