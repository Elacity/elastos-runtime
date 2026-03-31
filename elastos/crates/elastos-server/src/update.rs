//! Update/upgrade logic: Carrier-first release fetch, explicit transport
//! overrides, platform detection, cache management, and the main update flow.
//!
//! `try_p2p_discovery` and `start_release_discovery_responder` remain in
//! main.rs because they depend on binary-only infrastructure (`IpfsBridge`,
//! `find_installed_provider_binary`).

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
pub type TryP2pFn = Box<
    dyn Fn(TrustedSource, String) -> Pin<Box<dyn Future<Output = Option<String>> + Send>>
        + Send
        + Sync,
>;

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
    if cfg!(target_arch = "x86_64") {
        "x86_64-linux"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64-linux"
    } else {
        "unknown"
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
    let sources = load_trusted_sources(&data_dir)?;
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
                    Ok(Some(cid)) => {
                        // Found via Carrier — will be reported below
                        resolved_head_cid = Some(cid);
                        Some("Carrier")
                    }
                    Ok(None) => None,
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

    // 4. Verify signature
    let head = verify_release_envelope(&head_bytes, "elastos.release.head.v1", &primary_publisher)?;

    let head_version = head["payload"]["version"].as_str().unwrap_or("unknown");
    let release_cid = head["payload"]["latest_release_cid"].as_str().unwrap_or("");

    run_upgrade_from_head(
        fetch_fn,
        &head,
        &head_bytes,
        resolved_head_cid.as_deref(),
        head_version,
        release_cid,
        &current_version,
        &source,
        &data_dir,
        check_only,
        &ordered_gateways,
        auto_confirm,
        force,
        discovery_method,
        working_gateway.as_deref(),
    )
    .await
}

/// Execute upgrade from a verified release head.
#[allow(clippy::too_many_arguments)]
async fn run_upgrade_from_head(
    fetch_fn: &FetchFn,
    _head: &serde_json::Value,
    head_bytes: &[u8],
    resolved_head_cid: Option<&str>,
    version: &str,
    release_cid: &str,
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

    if version == current_version && !force {
        println!();
        println!("  Installed release is up to date.");
        return Ok(());
    }

    // Show update plan
    let is_rollback = force && version == current_version;
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

    // 6. Fetch release.json
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

    // 7. Verify release.json
    let (release, signer_did) = verify_release_envelope_against_dids(
        &release_bytes,
        "elastos.release.v1",
        &source.publisher_dids,
    )?;
    println!("  Release signer: {}", signer_did);

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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::Path as AxumPath;
    use axum::http::StatusCode;
    use axum::routing::get;
    use axum::Router;

    #[test]
    fn test_is_ipfs_content_gateway_matches_known_hosts() {
        assert!(is_ipfs_content_gateway("https://ipfs.io"));
        assert!(is_ipfs_content_gateway("https://dweb.link"));
        assert!(!is_ipfs_content_gateway("https://publisher.example.com"));
    }

    #[test]
    fn test_changed_capsule_names_only_returns_changed_entries() {
        let old = serde_json::json!({
            "external": {},
            "profiles": {},
            "capsules": {
                "chat": { "cid": "cid-chat-1", "sha256": "a", "size": 1, "platforms": ["x86_64-linux"] },
                "peer-provider": { "cid": "cid-peer-1", "sha256": "b", "size": 1, "platforms": ["x86_64-linux"] }
            }
        });
        let new = serde_json::json!({
            "external": {},
            "profiles": {},
            "capsules": {
                "chat": { "cid": "cid-chat-2", "sha256": "c", "size": 1, "platforms": ["x86_64-linux"] },
                "peer-provider": { "cid": "cid-peer-1", "sha256": "b", "size": 1, "platforms": ["x86_64-linux"] },
                "did-provider": { "cid": "cid-did-1", "sha256": "d", "size": 1, "platforms": ["x86_64-linux"] }
            }
        });
        let old_bytes = serde_json::to_vec(&old).unwrap();
        let new_bytes = serde_json::to_vec(&new).unwrap();
        let changed = changed_capsule_names(Some(old_bytes.as_slice()), &new_bytes).unwrap();
        assert_eq!(
            changed,
            vec!["chat".to_string(), "did-provider".to_string()]
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
