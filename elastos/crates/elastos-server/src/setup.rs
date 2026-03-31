//! `elastos setup` — Default component provisioning
//!
//! Downloads and installs external components (kubo, cloudflared, llama-server,
//! models) into `~/.local/share/elastos/`. Profiles group components for common
//! use-cases. Providers fail fast and point users here instead of auto-downloading.

use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::HashMap;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;

const DEFAULT_SETUP_PROFILE: &str = "pc2";
const CACHED_CID_FILE: &str = ".elastos-cid";
const CACHED_ARTIFACT_SHA_FILE: &str = ".elastos-artifact-sha256";

// ── Manifest types ──────────────────────────────────────────────────

#[derive(Deserialize, Serialize, Clone)]
pub struct ComponentsManifest {
    /// External tools (kubo, cloudflared, llama-server, models).
    pub external: HashMap<String, Component>,

    /// Capsule registry (CID-based entries). Empty in legacy format.
    /// Consumed by supervisor in M2+ (ensure_capsule, launch_capsule).
    #[serde(default)]
    pub capsules: HashMap<String, CapsuleEntry>,

    pub profiles: HashMap<String, Profile>,
}

/// An external tool component.
///
/// First-party components should be CID-backed and resolved as `elastos://...`.
/// Explicit vendor URLs remain allowed only for specific approved external tools.
#[derive(Deserialize, Serialize, Clone)]
pub struct Component {
    pub version: Option<String>,
    #[serde(default)]
    pub install_path: Option<String>,
    #[serde(default)]
    pub size_mb: Option<u64>,
    #[serde(default)]
    pub description: Option<String>,
    pub platforms: HashMap<String, PlatformInfo>,
}

/// A capsule registry entry (CID-based, resolved via IPFS gateways).
/// Consumed by supervisor in M2+ (ensure_capsule, launch_capsule).
#[derive(Deserialize, Serialize, Clone)]
pub struct CapsuleEntry {
    pub cid: String,
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub platforms: Vec<String>,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct PlatformInfo {
    pub url: Option<String>,
    /// IPFS CID for content-addressed downloads (used instead of url).
    pub cid: Option<String>,
    #[serde(default)]
    pub release_path: Option<String>,
    pub checksum: Option<String>,
    pub extract_path: Option<String>,
    #[serde(default)]
    pub install_path: Option<String>,
    pub strategy: Option<String>,
    /// Local filesystem path to copy from (for "local-copy" strategy).
    pub source: Option<String>,
    pub note: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct Profile {
    pub description: Option<String>,
    pub components: Vec<String>,
}

// ── Entry point ─────────────────────────────────────────────────────

pub async fn run(
    profile: Option<String>,
    with: Vec<String>,
    without: Vec<String>,
    list: bool,
) -> anyhow::Result<()> {
    let manifest = load_manifest()?;
    let data_dir = data_dir()?;
    let platform = detect_platform();

    eprintln!(
        "ElastOS v{} — setup for {}",
        env!("ELASTOS_VERSION"),
        platform
    );

    if list {
        list_components(&manifest, &data_dir, &platform);
        return Ok(());
    }

    // Local Elastos pin path only. Trusted source fetch happens over Carrier.
    let ipfs_gateways = build_gateway_list(&data_dir);

    let selected_profile = profile.as_deref().map(normalize_profile_name).or({
        if with.is_empty() {
            Some(DEFAULT_SETUP_PROFILE)
        } else {
            None
        }
    });

    if profile.is_none() && with.is_empty() {
        println!("Using default setup profile: {}", DEFAULT_SETUP_PROFILE);
        println!();
    }

    let components = resolve_components(&manifest, selected_profile, &with, &without)?;

    if components.is_empty() {
        println!("No components selected.");
        println!("Use --with <component> to add components, or --list to see available profiles/components.");
        return Ok(());
    }

    println!("Components to install:");
    for name in &components {
        let comp = &manifest.external[name];
        let platform_info = resolve_platform_info(comp, &platform);
        let status =
            match component_install_state_for_name(&manifest, &data_dir, name, comp, platform_info)
            {
                InstallState::Installed => " [already installed]",
                InstallState::Stale(_) => " [stale: will refresh]",
                InstallState::Missing => "",
            };
        let size = comp
            .size_mb
            .map(|s| format!(" (~{} MB)", s))
            .unwrap_or_default();
        println!("  - {}{}{}", name, size, status);
    }
    println!();

    let mut installed_count = 0u32;
    let mut skipped_count = 0u32;

    for name in &components {
        let comp = &manifest.external[name];
        let platform_info = resolve_platform_info(comp, &platform);

        match component_install_state_for_name(&manifest, &data_dir, name, comp, platform_info) {
            InstallState::Installed => {
                println!("[skip] {} — already installed", name);
                skipped_count += 1;
                continue;
            }
            InstallState::Stale(reason) => {
                println!("[refresh] {} — {}", name, reason);
            }
            InstallState::Missing => {}
        }

        let platform_info = match platform_info {
            Some(info) => info,
            None => {
                println!("[skip] {} — not available for {}", name, platform);
                skipped_count += 1;
                continue;
            }
        };

        if platform_info.strategy.as_deref() == Some("source-build") {
            let note = platform_info
                .note
                .as_deref()
                .unwrap_or("Source build required");
            println!("[skip] {} — {}", name, note);
            skipped_count += 1;
            continue;
        }

        if platform_info.strategy.as_deref() == Some("local-copy") {
            let source = match &platform_info.source {
                Some(s) => PathBuf::from(s),
                None => {
                    println!("[skip] {} — local-copy strategy but no source path", name);
                    skipped_count += 1;
                    continue;
                }
            };
            if !source.is_file() {
                let note = platform_info
                    .note
                    .as_deref()
                    .unwrap_or("Local source file not found");
                println!(
                    "[skip] {} — {} (expected: {})",
                    name,
                    note,
                    source.display()
                );
                // Actionable guidance for the most common local-copy case: vmlinux on aarch64.
                if name == "vmlinux" {
                    println!("       MicroVM capsules will not work without a guest kernel.");
                    println!(
                        "       On Jetson/aarch64, ensure {} exists (the host kernel).",
                        source.display()
                    );
                    println!(
                        "       On other aarch64 hosts, copy a compatible kernel to {}.",
                        source.display()
                    );
                }
                skipped_count += 1;
                continue;
            }
            let install_path = match resolve_install_path(comp, Some(platform_info)) {
                Some(p) => p,
                None => {
                    println!("[skip] {} — no install_path configured", name);
                    skipped_count += 1;
                    continue;
                }
            };
            let dest = data_dir.join(install_path);
            println!("[install] {} — copying from {}", name, source.display());
            atomic_copy_file(&source, &dest)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o644));
            }
            maybe_write_component_cache_metadata(&manifest, Some(platform_info), name, &dest)?;
            println!("  Installed: {}", dest.display());
            installed_count += 1;
            continue;
        }

        let resolved_url = match resolve_component_download_url(platform_info) {
            Some(url) => url,
            None => {
                println!("[skip] {} — no download URL or CID for {}", name, platform);
                skipped_count += 1;
                continue;
            }
        };

        let install_path = match resolve_install_path(comp, Some(platform_info)) {
            Some(p) => p,
            None => {
                println!("[skip] {} — no install_path configured", name);
                skipped_count += 1;
                continue;
            }
        };
        let dest = data_dir.join(install_path);
        println!("[install] {} ...", name);
        download_component(
            &data_dir,
            name,
            &resolved_url,
            platform_info,
            &dest,
            &ipfs_gateways,
        )
        .await?;
        maybe_write_component_cache_metadata(&manifest, Some(platform_info), name, &dest)?;
        installed_count += 1;
    }

    let stamped = write_installed_manifest(&data_dir, &manifest, &platform)?;

    println!();
    if !stamped.is_empty() {
        println!(
            "Stamped installed manifest checksums: {}",
            stamped.join(", ")
        );
    }
    println!(
        "Done. {} installed, {} skipped.",
        installed_count, skipped_count
    );

    Ok(())
}

// ── Manifest loading ────────────────────────────────────────────────

fn load_manifest() -> anyhow::Result<ComponentsManifest> {
    let exe_path = std::env::current_exe().ok();
    let installed_paths = [
        // Installed layout
        dirs::data_dir().map(|d| d.join("elastos/components.json")),
        // Exe-relative (release tarball)
        exe_path.and_then(|p| p.parent().map(|d| d.join("components.json"))),
    ];

    for path in installed_paths.iter().flatten() {
        if let Ok(content) = fs::read_to_string(path) {
            let manifest: ComponentsManifest = serde_json::from_str(&content).map_err(|e| {
                anyhow::anyhow!("Invalid components.json at {}: {}", path.display(), e)
            })?;
            return Ok(manifest);
        }
    }

    anyhow::bail!(
        "components.json not found. Searched:\n  \
         ~/.local/share/elastos/components.json\n  \
         <exe-dir>/components.json"
    )
}

fn data_dir() -> anyhow::Result<PathBuf> {
    let dir = dirs::data_dir()
        .map(|d| d.join("elastos"))
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".local/share/elastos")
        });
    Ok(dir)
}

// ── Platform detection ──────────────────────────────────────────────

pub fn detect_platform() -> String {
    let os = if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    };

    let arch = if cfg!(target_arch = "x86_64") {
        "amd64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "unknown"
    };

    format!("{}-{}", os, arch)
}

pub fn verify_installed_component_binary(
    data_dir: &Path,
    name: &str,
    path: &Path,
) -> anyhow::Result<String> {
    let installed_bin = data_dir.join("bin").join(name);
    let installed_capsule = data_dir.join("capsules").join(name).join(name);
    let exe_relative = std::env::current_exe().ok().and_then(|exe| {
        exe.parent()
            .map(|dir| dir.join("../share/elastos/bin").join(name))
    });

    let is_installed_path = path == installed_bin
        || path == installed_capsule
        || exe_relative
            .as_deref()
            .is_some_and(|candidate| candidate == path);

    if !is_installed_path {
        anyhow::bail!(
            "{} must resolve from an installed runtime path, got dev/override path {}",
            name,
            path.display()
        );
    }

    let manifest_path = data_dir.join("components.json");
    let manifest_bytes = fs::read(&manifest_path).map_err(|e| {
        anyhow::anyhow!(
            "cannot verify installed component '{}' at {}: failed to read {}: {}",
            name,
            path.display(),
            manifest_path.display(),
            e
        )
    })?;
    let manifest: ComponentsManifest = serde_json::from_slice(&manifest_bytes).map_err(|e| {
        anyhow::anyhow!(
            "cannot verify installed component '{}' at {}: invalid {}: {}",
            name,
            path.display(),
            manifest_path.display(),
            e
        )
    })?;
    let component = manifest.external.get(name).ok_or_else(|| {
        anyhow::anyhow!(
            "cannot verify installed component '{}' at {}: missing entry in {}",
            name,
            path.display(),
            manifest_path.display()
        )
    })?;
    let platform = detect_platform();
    let platform_info = resolve_platform_info(component, &platform).ok_or_else(|| {
        anyhow::anyhow!(
            "cannot verify installed component '{}' at {}: no platform entry for {}",
            name,
            path.display(),
            platform
        )
    })?;
    let checksum = platform_info
        .checksum
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "cannot verify installed component '{}' at {}: missing checksum for {} in {}",
                name,
                path.display(),
                platform,
                manifest_path.display()
            )
        })?;
    if !file_matches_checksum(path, checksum)? {
        anyhow::bail!(
            "installed component '{}' at {} failed checksum verification against {}",
            name,
            path.display(),
            manifest_path.display()
        );
    }

    Ok(checksum.to_string())
}

// ── Component status ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum InstallState {
    Missing,
    Installed,
    Stale(String),
}

fn component_install_state_for_name(
    manifest: &ComponentsManifest,
    data_dir: &Path,
    name: &str,
    component: &Component,
    platform_info: Option<&PlatformInfo>,
) -> InstallState {
    let base = component_install_state(data_dir, component, platform_info);
    if !matches!(base, InstallState::Installed) {
        return base;
    }

    let Some(entry) = manifest.capsules.get(name) else {
        return base;
    };

    let Some(path) = resolve_install_path(component, platform_info) else {
        return base;
    };
    let install_root = data_dir.join(path);
    if !install_root.is_dir() {
        return base;
    }
    if !install_root.join("capsule.json").is_file() {
        return InstallState::Stale("capsule metadata missing from installed bundle".to_string());
    }

    let cached_cid = fs::read_to_string(install_root.join(CACHED_CID_FILE))
        .ok()
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if cached_cid != entry.cid {
        return InstallState::Stale("capsule cache CID metadata missing or stale".to_string());
    }

    if !entry.sha256.is_empty() {
        let cached_sha = fs::read_to_string(install_root.join(CACHED_ARTIFACT_SHA_FILE))
            .ok()
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        if cached_sha != entry.sha256 {
            return InstallState::Stale(
                "capsule cache checksum metadata missing or stale".to_string(),
            );
        }
    }

    base
}

fn maybe_write_component_cache_metadata(
    manifest: &ComponentsManifest,
    platform_info: Option<&PlatformInfo>,
    name: &str,
    dest: &Path,
) -> anyhow::Result<()> {
    if !dest.is_dir() {
        return Ok(());
    }

    if let Some(entry) = manifest.capsules.get(name) {
        if entry.cid.trim().is_empty() || !dest.join("capsule.json").is_file() {
            return Ok(());
        }

        fs::write(
            dest.join(CACHED_CID_FILE),
            format!("{}\n", entry.cid.trim()),
        )?;
        if !entry.sha256.trim().is_empty() {
            fs::write(
                dest.join(CACHED_ARTIFACT_SHA_FILE),
                format!("{}\n", entry.sha256.trim()),
            )?;
        }
        return Ok(());
    }

    let Some(platform_info) = platform_info else {
        return Ok(());
    };
    if platform_info.extract_path.is_none() {
        return Ok(());
    }

    if let Some(cid) = platform_info
        .cid
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        fs::write(dest.join(CACHED_CID_FILE), format!("{}\n", cid))?;
    }
    if let Some(checksum) = platform_info
        .checksum
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        fs::write(
            dest.join(CACHED_ARTIFACT_SHA_FILE),
            format!("{}\n", checksum),
        )?;
    }
    Ok(())
}

fn component_install_state(
    data_dir: &Path,
    component: &Component,
    platform_info: Option<&PlatformInfo>,
) -> InstallState {
    match resolve_install_path(component, platform_info) {
        Some(path) => {
            let candidate = data_dir.join(path);
            if !candidate.exists() {
                return InstallState::Missing;
            }
            if candidate.is_dir() {
                if let Some(platform_info) = platform_info {
                    if platform_info.extract_path.is_some() {
                        if let Some(expected_cid) = platform_info
                            .cid
                            .as_deref()
                            .filter(|value| !value.is_empty())
                        {
                            let cached_cid = fs::read_to_string(candidate.join(CACHED_CID_FILE))
                                .ok()
                                .map(|value| value.trim().to_string())
                                .unwrap_or_default();
                            if cached_cid != expected_cid {
                                return InstallState::Stale(
                                    "extracted bundle CID metadata missing or stale".to_string(),
                                );
                            }
                        }

                        if let Some(expected_sha) = platform_info
                            .checksum
                            .as_deref()
                            .filter(|value| !value.is_empty())
                        {
                            let cached_sha =
                                fs::read_to_string(candidate.join(CACHED_ARTIFACT_SHA_FILE))
                                    .ok()
                                    .map(|value| value.trim().to_string())
                                    .unwrap_or_default();
                            if cached_sha != expected_sha {
                                return InstallState::Stale(
                                    "extracted bundle checksum metadata missing or stale"
                                        .to_string(),
                                );
                            }
                        }
                    }
                }
                return InstallState::Installed;
            }

            let Some(platform_info) = platform_info else {
                return InstallState::Installed;
            };

            if platform_info.strategy.as_deref() == Some("source-build")
                || platform_info.extract_path.is_some()
            {
                return InstallState::Installed;
            }

            if platform_info.strategy.as_deref() == Some("local-copy") {
                if let Some(source) = platform_info.source.as_ref().map(PathBuf::from) {
                    if source.is_file() {
                        let source_len = fs::metadata(&source).ok().map(|m| m.len());
                        let candidate_len = fs::metadata(&candidate).ok().map(|m| m.len());
                        if source_len != candidate_len {
                            return InstallState::Stale(format!(
                                "size mismatch against local source {}",
                                source.display()
                            ));
                        }
                    }
                }
            }

            if let Some(expected_size) = platform_info.size.filter(|size| *size > 0) {
                match fs::metadata(&candidate) {
                    Ok(meta) if meta.len() == expected_size => {}
                    Ok(meta) => {
                        return InstallState::Stale(format!(
                            "size mismatch (have {} bytes, expected {})",
                            meta.len(),
                            expected_size
                        ));
                    }
                    Err(err) => {
                        return InstallState::Stale(format!("metadata read failed: {}", err));
                    }
                }
            }

            if let Some(expected) = platform_info
                .checksum
                .as_deref()
                .filter(|checksum| !checksum.is_empty())
            {
                match file_matches_checksum(&candidate, expected) {
                    Ok(true) => InstallState::Installed,
                    Ok(false) => InstallState::Stale("checksum mismatch".to_string()),
                    Err(err) => {
                        InstallState::Stale(format!("checksum verification failed: {}", err))
                    }
                }
            } else {
                InstallState::Installed
            }
        }
        None => InstallState::Missing,
    }
}

fn file_matches_checksum(path: &Path, expected: &str) -> anyhow::Result<bool> {
    let mut file = fs::File::open(path)?;
    let mut buf = [0u8; 8192];

    if let Some(expected_sha256) = expected.strip_prefix("sha256:") {
        let mut hasher = sha2::Sha256::new();
        loop {
            let read = file.read(&mut buf)?;
            if read == 0 {
                break;
            }
            hasher.update(&buf[..read]);
        }
        return Ok(hex::encode(hasher.finalize()) == expected_sha256.to_lowercase());
    }

    if let Some(expected_sha512) = expected.strip_prefix("sha512:") {
        let mut hasher = sha2::Sha512::new();
        loop {
            let read = file.read(&mut buf)?;
            if read == 0 {
                break;
            }
            hasher.update(&buf[..read]);
        }
        return Ok(hex::encode(hasher.finalize()) == expected_sha512.to_lowercase());
    }

    anyhow::bail!(
        "unknown checksum format for {}: expected sha256:... or sha512:...",
        path.display()
    );
}

/// Resolve install_path: platform-specific overrides component-level.
fn resolve_install_path<'a>(
    component: &'a Component,
    platform_info: Option<&'a PlatformInfo>,
) -> Option<&'a str> {
    platform_info
        .and_then(|p| p.install_path.as_deref())
        .or(component.install_path.as_deref())
}

fn resolve_platform_info<'a>(component: &'a Component, platform: &str) -> Option<&'a PlatformInfo> {
    component
        .platforms
        .get(platform)
        .or_else(|| platform_aliases(platform).find_map(|alias| component.platforms.get(alias)))
        .or_else(|| component.platforms.get("*"))
}

fn resolve_platform_info_mut<'a>(
    component: &'a mut Component,
    platform: &str,
) -> Option<&'a mut PlatformInfo> {
    if component.platforms.contains_key(platform) {
        return component.platforms.get_mut(platform);
    }

    for alias in platform_aliases(platform) {
        if component.platforms.contains_key(alias) {
            return component.platforms.get_mut(alias);
        }
    }

    component.platforms.get_mut("*")
}

fn platform_aliases(platform: &str) -> impl Iterator<Item = &'static str> {
    let aliases: &'static [&'static str] = match platform {
        "x86_64-linux" => &["linux-amd64"],
        "aarch64-linux" => &["linux-arm64"],
        "linux-amd64" => &["x86_64-linux"],
        "linux-arm64" => &["aarch64-linux"],
        _ => &[],
    };
    aliases.iter().copied()
}

fn compute_sha256_checksum(path: &Path) -> anyhow::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut buf = [0u8; 8192];
    let mut hasher = sha2::Sha256::new();
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

fn stamp_installed_file_metadata(
    manifest: &mut ComponentsManifest,
    data_dir: &Path,
    platform: &str,
) -> anyhow::Result<Vec<String>> {
    let mut stamped = Vec::new();

    for (name, component) in manifest.external.iter_mut() {
        let component_install_path = component.install_path.clone();
        let Some(platform_info) = resolve_platform_info_mut(component, platform) else {
            continue;
        };
        if platform_info.strategy.as_deref() == Some("source-build") {
            continue;
        }

        let install_path = platform_info
            .install_path
            .clone()
            .or(component_install_path);
        let Some(install_path) = install_path else {
            continue;
        };

        let installed_path = data_dir.join(install_path);
        if !installed_path.exists() || installed_path.is_dir() {
            continue;
        }

        let force_runtime_checksum = platform_info.strategy.as_deref() == Some("local-copy");
        let needs_checksum = force_runtime_checksum
            || platform_info
                .checksum
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .is_empty();
        let needs_size = force_runtime_checksum || platform_info.size.unwrap_or(0) == 0;
        if !needs_checksum && !needs_size {
            continue;
        }

        let metadata = fs::metadata(&installed_path)?;
        let mut touched = false;
        if needs_size {
            platform_info.size = Some(metadata.len());
            touched = true;
        }
        if needs_checksum {
            platform_info.checksum = Some(compute_sha256_checksum(&installed_path)?);
            touched = true;
        }
        if touched {
            stamped.push(name.clone());
        }
    }

    stamped.sort();
    stamped.dedup();
    Ok(stamped)
}

pub fn write_installed_manifest(
    data_dir: &Path,
    manifest: &ComponentsManifest,
    platform: &str,
) -> anyhow::Result<Vec<String>> {
    let mut installed_manifest = manifest.clone();
    let stamped = stamp_installed_file_metadata(&mut installed_manifest, data_dir, platform)?;
    fs::create_dir_all(data_dir)?;
    let manifest_bytes = serde_json::to_vec_pretty(&installed_manifest)?;
    atomic_write_file(&data_dir.join("components.json"), &manifest_bytes)?;
    Ok(stamped)
}

pub fn write_installed_manifest_bytes(
    data_dir: &Path,
    manifest_bytes: &[u8],
    platform: &str,
) -> anyhow::Result<Vec<String>> {
    let manifest: ComponentsManifest = serde_json::from_slice(manifest_bytes)?;
    write_installed_manifest(data_dir, &manifest, platform)
}

// ── List mode ───────────────────────────────────────────────────────

fn list_components(manifest: &ComponentsManifest, data_dir: &Path, platform: &str) {
    println!("Platform: {}", platform);
    println!();
    println!("Components:");

    let mut names: Vec<_> = manifest.external.keys().collect();
    names.sort();

    for name in &names {
        let comp = &manifest.external[*name];
        let platform_info = resolve_platform_info(comp, platform);
        let install_state =
            component_install_state_for_name(manifest, data_dir, name, comp, platform_info);
        let available = platform_info.is_some();
        let source_build =
            platform_info.and_then(|p| p.strategy.as_deref()) == Some("source-build");

        let status = if matches!(install_state, InstallState::Installed) {
            "[installed]"
        } else if matches!(install_state, InstallState::Stale(_)) {
            "[stale]"
        } else if source_build {
            "[source-build]"
        } else if available {
            "[available]"
        } else {
            "[n/a]"
        };

        let version = comp.version.as_deref().unwrap_or("");
        let size = comp
            .size_mb
            .map(|s| format!(" (~{} MB)", s))
            .unwrap_or_default();
        let desc = comp.description.as_deref().unwrap_or("");
        println!("  {:14} {:20} {}{}", status, name, version, size);
        if !desc.is_empty() {
            println!("  {:14} {:20} {}", "", "", desc);
        }
    }

    println!();
    print_profile_section("Recommended profiles:", manifest, &["pc2", "demo", "irc"]);
    print_profile_section(
        "Advanced profiles:",
        manifest,
        &["minimal", "public-gateway", "agent-local-ai", "full"],
    );

    let listed = [
        "pc2",
        "demo",
        "irc",
        "minimal",
        "public-gateway",
        "agent-local-ai",
        "full",
    ];
    let mut other_profiles: Vec<_> = manifest
        .profiles
        .keys()
        .filter(|name| !listed.contains(&name.as_str()))
        .collect();
    other_profiles.sort();
    if !other_profiles.is_empty() {
        println!("Other profiles:");
        for name in other_profiles {
            print_profile_line(name, &manifest.profiles[name]);
        }
    }
}

fn print_profile_section(title: &str, manifest: &ComponentsManifest, names: &[&str]) {
    let present: Vec<_> = names
        .iter()
        .copied()
        .filter(|name| manifest.profiles.contains_key(*name))
        .collect();
    if present.is_empty() {
        return;
    }
    println!("{}", title);
    for name in present {
        print_profile_line(name, &manifest.profiles[name]);
    }
}

fn print_profile_line(name: &str, profile: &Profile) {
    let desc = profile.description.as_deref().unwrap_or("");
    let comps = if profile.components.is_empty() {
        "(none)".to_string()
    } else {
        profile.components.join(", ")
    };
    println!("  {:20} {} [{}]", name, desc, comps);
}

// ── Component resolution ────────────────────────────────────────────

fn resolve_components(
    manifest: &ComponentsManifest,
    profile: Option<&str>,
    with: &[String],
    without: &[String],
) -> anyhow::Result<Vec<String>> {
    let profile_name = profile
        .map(normalize_profile_name)
        .unwrap_or_default()
        .to_string();

    let mut components: Vec<String> = if profile_name.is_empty() {
        Vec::new()
    } else {
        let profile = manifest
            .profiles
            .get(&profile_name)
            .ok_or_else(|| anyhow::anyhow!("Unknown profile: {}", profile_name))?;
        profile.components.clone()
    };

    // Add --with components
    for name in with {
        if !manifest.external.contains_key(name) {
            anyhow::bail!("Unknown component: {}", name);
        }
        if !components.contains(name) {
            components.push(name.clone());
        }
    }

    // Remove --without components (validate names first)
    for name in without {
        if !manifest.external.contains_key(name) {
            anyhow::bail!("Unknown component in --without: {}", name);
        }
    }
    components.retain(|c| !without.contains(c));

    Ok(components)
}

fn normalize_profile_name(name: &str) -> &str {
    match name {
        "chat" => DEFAULT_SETUP_PROFILE,
        "focus-chat" | "immersive-chat" => "irc",
        other => other,
    }
}

// ── Elastos fetch-path resolution ──────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
struct ElastosFetchPath {
    transport_base: String,
    description: String,
}

/// Build explicit trusted-source fetch paths for component downloads that still
/// require CID transport after Carrier bootstrap.
fn build_gateway_list(data_dir: &Path) -> Vec<ElastosFetchPath> {
    trusted_gateway_overrides(data_dir)
}

fn trusted_gateway_overrides(data_dir: &Path) -> Vec<ElastosFetchPath> {
    let Ok(config) = crate::sources::load_trusted_sources(data_dir) else {
        return Vec::new();
    };
    let Some(source) = config.default_source() else {
        return Vec::new();
    };

    crate::sources::normalize_gateways(&source.gateways)
        .into_iter()
        .map(|gateway| ElastosFetchPath {
            description: format!("trusted source fetch path ({})", gateway),
            transport_base: gateway,
        })
        .collect()
}

fn resolve_cid_display_url(cid: &str) -> String {
    // Display-only identity string for CID-backed components.
    // Actual downloads use the configured Elastos fetch paths above.
    format!("elastos://{}", cid)
}

fn resolve_component_download_url(platform_info: &PlatformInfo) -> Option<String> {
    platform_info
        .cid
        .as_ref()
        .filter(|cid| !cid.is_empty())
        .map(|cid| resolve_cid_display_url(cid))
        .or_else(|| {
            platform_info
                .release_path
                .as_ref()
                .filter(|path| !path.is_empty())
                .map(|path| format!("elastos://artifact/{}", path))
        })
        .or_else(|| platform_info.url.clone())
}

pub async fn refresh_installed_components_for_update(
    data_dir: &Path,
    old_components: Option<&[u8]>,
    new_components: &[u8],
    platform: &str,
) -> anyhow::Result<Vec<String>> {
    let new_manifest: ComponentsManifest = serde_json::from_slice(new_components)?;
    let Some(old_bytes) = old_components else {
        return Ok(Vec::new());
    };
    let old_manifest: ComponentsManifest = serde_json::from_slice(old_bytes)?;

    let gateways = build_gateway_list(data_dir);
    let mut refreshed = Vec::new();

    for (name, new_component) in &new_manifest.external {
        let old_component = old_manifest.external.get(name);
        if component_signature(old_component, platform)
            == component_signature(Some(new_component), platform)
        {
            continue;
        }

        let new_platform_info = match resolve_platform_info(new_component, platform) {
            Some(info) => info,
            None => continue,
        };

        if matches!(
            component_install_state_for_name(
                &new_manifest,
                data_dir,
                name,
                new_component,
                Some(new_platform_info)
            ),
            InstallState::Missing
        ) {
            continue;
        }

        if new_platform_info.strategy.as_deref() == Some("source-build") {
            continue;
        }

        let install_path = match resolve_install_path(new_component, Some(new_platform_info)) {
            Some(path) => path,
            None => continue,
        };
        let dest = data_dir.join(install_path);

        if new_platform_info.strategy.as_deref() == Some("local-copy") {
            let source = match new_platform_info.source.as_ref() {
                Some(s) => PathBuf::from(s),
                None => continue,
            };
            if !source.is_file() {
                continue;
            }
            atomic_copy_file(&source, &dest)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o644));
            }
            refreshed.push(name.clone());
            continue;
        }

        let resolved_url = match resolve_component_download_url(new_platform_info) {
            Some(url) => url,
            None => continue,
        };

        download_component(
            data_dir,
            name,
            &resolved_url,
            new_platform_info,
            &dest,
            &gateways,
        )
        .await?;
        maybe_write_component_cache_metadata(&new_manifest, Some(new_platform_info), name, &dest)?;
        refreshed.push(name.clone());
    }

    refreshed.sort();
    write_installed_manifest_bytes(data_dir, new_components, platform)?;
    Ok(refreshed)
}

fn component_signature(component: Option<&Component>, platform: &str) -> Option<String> {
    let component = component?;
    let platform_info = resolve_platform_info(component, platform)?;
    Some(format!(
        "version={:?}|component_install={:?}|platform_install={:?}|url={:?}|cid={:?}|release_path={:?}|checksum={:?}|extract={:?}|strategy={:?}|source={:?}",
        component.version,
        component.install_path,
        platform_info.install_path,
        platform_info.url,
        platform_info.cid,
        platform_info.release_path,
        platform_info.checksum,
        platform_info.extract_path,
        platform_info.strategy,
        platform_info.source
    ))
}

// ── Download and install ────────────────────────────────────────────

/// Public entry point for supervisor to download an external component.
pub async fn run_download(
    name: &str,
    url: &str,
    platform_info: &PlatformInfo,
    dest: &Path,
) -> anyhow::Result<()> {
    let data_dir = data_dir().unwrap_or_else(|_| PathBuf::from("/tmp/elastos"));
    let gateways = build_gateway_list(&data_dir);
    download_component(&data_dir, name, url, platform_info, dest, &gateways).await
}

pub(crate) async fn fetch_first_party_component_via_carrier(
    data_dir: &Path,
    release_path: &str,
) -> anyhow::Result<Vec<u8>> {
    let source = crate::sources::load_trusted_sources(data_dir)?
        .default_source()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("No trusted source configured"))?;
    crate::carrier::fetch_file_from_trusted_source(&source, release_path, 15, 30).await
}

pub(crate) async fn install_first_party_component_via_carrier(
    data_dir: &Path,
    name: &str,
    platform_info: &PlatformInfo,
    dest: &Path,
) -> anyhow::Result<()> {
    let release_path = platform_info.release_path.as_deref().ok_or_else(|| {
        anyhow::anyhow!("missing release_path for first-party component '{}'", name)
    })?;
    let bytes = fetch_first_party_component_via_carrier(data_dir, release_path).await?;
    verify_checksum(name, &bytes, platform_info)?;

    let is_model = dest.extension().map(|e| e == "gguf").unwrap_or(false);
    let is_tarball = release_path.ends_with(".tar.gz")
        || release_path.ends_with(".tgz")
        || platform_info.extract_path.is_some();

    if is_tarball {
        extract_from_tarball(&bytes, dest, platform_info)?;
    } else {
        atomic_write_file(dest, &bytes)?;
    }

    #[cfg(unix)]
    if !is_model && dest.is_file() {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dest, fs::Permissions::from_mode(0o755));
    }

    Ok(())
}

async fn download_component(
    data_dir: &Path,
    name: &str,
    url: &str,
    platform_info: &PlatformInfo,
    dest: &Path,
    ipfs_gateways: &[ElastosFetchPath],
) -> anyhow::Result<()> {
    // Ensure parent dir exists
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    let is_model = dest.extension().map(|e| e == "gguf").unwrap_or(false);
    let is_tarball =
        url.ends_with(".tar.gz") || url.ends_with(".tgz") || platform_info.extract_path.is_some();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()?;

    // First-party release artifacts come from the trusted source over Carrier.
    // Do not silently fall back to an HTTP gateway here: setup must fail closed
    // if the stamped Carrier bootstrap is missing or broken.
    let response = if let Some(release_path) = platform_info.release_path.as_deref() {
        let elastos_url = platform_info
            .cid
            .as_deref()
            .filter(|cid| !cid.is_empty())
            .map(resolve_cid_display_url)
            .unwrap_or_else(|| format!("elastos://artifact/{}", release_path));
        println!("  Resolving {} from {}...", name, elastos_url);
        println!(
            "  Trying {} via trusted source over Carrier...",
            elastos_url
        );
        match install_first_party_component_via_carrier(data_dir, name, platform_info, dest).await {
            Ok(()) => {
                println!("  Installed: {}", dest.display());
                return Ok(());
            }
            Err(err) => {
                anyhow::bail!(
                    "Trusted source Carrier fetch failed for {} ({}): {}",
                    name,
                    elastos_url,
                    err
                );
            }
        }
    } else if let Some(cid) = &platform_info.cid {
        let elastos_url = resolve_cid_display_url(cid);
        println!("  Resolving {} from {}...", name, elastos_url);
        {
            if ipfs_gateways.is_empty() {
                anyhow::bail!(
                    "No configured fetch path for {} ({}). Configure a trusted source with a publisher gateway.",
                    name,
                    elastos_url
                );
            }
            let mut last_err = String::new();
            let mut resp = None;
            for gw in ipfs_gateways {
                let gw_url = format!("{}/ipfs/{}", gw.transport_base.trim_end_matches('/'), cid);
                println!("  Trying {} via {}...", elastos_url, gw.description);
                match client.get(&gw_url).send().await {
                    Ok(r) if r.status().is_success() => {
                        resp = Some(r);
                        break;
                    }
                    Ok(r) => {
                        last_err = format!("HTTP {}", r.status());
                    }
                    Err(e) => {
                        last_err = e.to_string();
                    }
                }
            }
            resp.ok_or_else(|| {
                anyhow::anyhow!(
                    "All configured Elastos fetch paths failed for {} ({}): {}",
                    name,
                    elastos_url,
                    last_err
                )
            })?
        }
    } else {
        println!("  Downloading {}...", url);
        let r = client.get(url).send().await?;
        if !r.status().is_success() {
            anyhow::bail!("Download failed: HTTP {}", r.status());
        }
        r
    };

    let content_length = response.content_length();

    if is_model {
        // Stream large model files to disk with progress
        download_streaming(name, response, dest, platform_info, content_length).await?;
    } else {
        // Buffer smaller binaries in memory for checksum
        let bytes = response.bytes().await?;

        verify_checksum(name, &bytes, platform_info)?;

        if is_tarball {
            extract_from_tarball(&bytes, dest, platform_info)?;
        } else {
            atomic_write_file(dest, &bytes)?;
        }
    }

    // chmod +x for binaries
    #[cfg(unix)]
    if !is_model && dest.is_file() {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dest, fs::Permissions::from_mode(0o755));
    }

    println!("  Installed: {}", dest.display());
    Ok(())
}

async fn download_streaming(
    name: &str,
    response: reqwest::Response,
    dest: &Path,
    platform_info: &PlatformInfo,
    content_length: Option<u64>,
) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;

    let tmp_path = dest.with_extension("tmp");
    let mut file = tokio::fs::File::create(&tmp_path).await?;

    // Set up the right hasher based on expected checksum format
    let use_sha512 = platform_info
        .checksum
        .as_ref()
        .map(|c| c.starts_with("sha512:"))
        .unwrap_or(false);
    let mut hasher_256 = sha2::Sha256::new();
    let mut hasher_512 = sha2::Sha512::new();

    let mut downloaded: u64 = 0;
    let mut last_progress: u64 = 0;

    let mut response = response;
    while let Some(chunk) = response.chunk().await? {
        if use_sha512 {
            hasher_512.update(&chunk);
        } else {
            hasher_256.update(&chunk);
        }
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;

        // Print progress every 50 MB
        if downloaded - last_progress >= 50 * 1024 * 1024 {
            if let Some(total) = content_length {
                let pct = (downloaded as f64 / total as f64 * 100.0) as u32;
                eprint!(
                    "\r  {} — {} / {} MB ({}%)",
                    name,
                    downloaded / 1024 / 1024,
                    total / 1024 / 1024,
                    pct
                );
            } else {
                eprint!("\r  {} — {} MB downloaded", name, downloaded / 1024 / 1024);
            }
            last_progress = downloaded;
        }
    }
    file.flush().await?;
    drop(file);

    if last_progress > 0 {
        eprintln!(); // newline after progress
    }

    // Verify checksum
    if let Some(expected) = platform_info
        .checksum
        .as_deref()
        .filter(|checksum| !checksum.is_empty())
    {
        let (actual, algo) = if expected.starts_with("sha512:") {
            (
                format!("sha512:{}", hex::encode(hasher_512.finalize())),
                "sha512",
            )
        } else if expected.starts_with("sha256:") {
            (
                format!("sha256:{}", hex::encode(hasher_256.finalize())),
                "sha256",
            )
        } else {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            anyhow::bail!("Unknown checksum format for {}: {}", name, expected);
        };

        if actual != *expected {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            anyhow::bail!(
                "Checksum mismatch for {}! Expected: {}, Got: {}",
                name,
                expected,
                actual
            );
        }
        println!("  Checksum verified ({})", algo);
    }

    tokio::fs::rename(&tmp_path, dest).await?;
    Ok(())
}

fn verify_checksum(name: &str, data: &[u8], platform_info: &PlatformInfo) -> anyhow::Result<()> {
    let expected = match platform_info
        .checksum
        .as_deref()
        .filter(|checksum| !checksum.is_empty())
    {
        Some(c) => c,
        None => return Ok(()),
    };

    if expected.starts_with("sha512:") {
        let actual = format!("sha512:{}", hex::encode(sha2::Sha512::digest(data)));
        if actual != *expected {
            anyhow::bail!(
                "Checksum mismatch for {}! Expected: {}, Got: {}",
                name,
                expected,
                actual
            );
        }
        println!("  Checksum verified (sha512)");
    } else if expected.starts_with("sha256:") {
        let actual = format!("sha256:{}", hex::encode(sha2::Sha256::digest(data)));
        if actual != *expected {
            anyhow::bail!(
                "Checksum mismatch for {}! Expected: {}, Got: {}",
                name,
                expected,
                actual
            );
        }
        println!("  Checksum verified (sha256)");
    } else {
        anyhow::bail!(
            "Unknown checksum format for {}: {}. Expected sha256:... or sha512:...",
            name,
            expected
        );
    }

    Ok(())
}

fn extract_from_tarball(
    data: &[u8],
    dest: &Path,
    platform_info: &PlatformInfo,
) -> anyhow::Result<()> {
    let extract_path = platform_info
        .extract_path
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Tarball component missing extract_path"))?;

    // Reject extract paths that could escape the temp directory
    if extract_path.contains("..") || std::path::Path::new(extract_path).is_absolute() {
        anyhow::bail!(
            "extract_path must be relative and not contain '..': {}",
            extract_path
        );
    }

    let tmp_dir = tempfile::tempdir()?;
    let tar_path = tmp_dir.path().join("archive.tar.gz");
    fs::write(&tar_path, data)?;

    let output = Command::new("tar")
        .args([
            "xzf",
            &tar_path.to_string_lossy(),
            "-C",
            &tmp_dir.path().to_string_lossy(),
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run tar: {}", e))?;

    if !output.status.success() {
        anyhow::bail!(
            "tar extraction failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let extracted = tmp_dir.path().join(extract_path);
    if extracted.is_dir() {
        atomic_copy_dir(&extracted, dest)?;
        return Ok(());
    }

    if extracted.is_file() {
        atomic_copy_file(&extracted, dest)?;
        return Ok(());
    }

    anyhow::bail!(
        "{} not found in tarball (expected at {})",
        extract_path,
        extracted.display()
    )
}

fn atomic_write_file(dest: &Path, data: &[u8]) -> anyhow::Result<()> {
    let parent = dest
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Destination has no parent: {}", dest.display()))?;
    fs::create_dir_all(parent)?;

    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        dest.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("elastos"),
        std::process::id()
    ));

    fs::write(&tmp, data)?;
    fs::rename(&tmp, dest)?;
    Ok(())
}

fn atomic_copy_file(src: &Path, dest: &Path) -> anyhow::Result<()> {
    let parent = dest
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Destination has no parent: {}", dest.display()))?;
    fs::create_dir_all(parent)?;

    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        dest.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("elastos"),
        std::process::id()
    ));

    fs::copy(src, &tmp)?;
    fs::rename(&tmp, dest)?;
    Ok(())
}

fn atomic_copy_dir(src: &Path, dest: &Path) -> anyhow::Result<()> {
    let parent = dest
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Destination has no parent: {}", dest.display()))?;
    fs::create_dir_all(parent)?;

    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        dest.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("elastos"),
        std::process::id()
    ));

    if tmp.exists() {
        fs::remove_dir_all(&tmp)?;
    }
    copy_dir_recursive(src, &tmp)?;
    if dest.exists() {
        fs::remove_dir_all(dest)?;
    }
    fs::rename(&tmp, dest)?;
    Ok(())
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let target = dest.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::{save_trusted_sources, TrustedSource, TrustedSourcesConfig};

    #[test]
    fn test_detect_platform() {
        let p = detect_platform();
        assert!(
            p.contains("linux") || p.contains("unknown"),
            "Platform should contain os: {}",
            p
        );
        assert!(
            p.contains("amd64") || p.contains("arm64") || p.contains("unknown"),
            "Platform should contain arch: {}",
            p
        );
    }

    #[test]
    fn test_load_manifest_from_installed_data_dir() {
        let temp = tempfile::tempdir().unwrap();
        let xdg_data_home = temp.path().join("xdg-data");
        let data_dir = xdg_data_home.join("elastos");
        fs::create_dir_all(&data_dir).unwrap();
        fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../components.json"),
            data_dir.join("components.json"),
        )
        .unwrap();

        std::env::set_var("XDG_DATA_HOME", &xdg_data_home);
        let manifest = load_manifest().unwrap();
        std::env::remove_var("XDG_DATA_HOME");

        assert!(manifest.external.contains_key("kubo"));
        assert!(manifest.profiles.contains_key("pc2"));
        assert!(manifest.profiles.contains_key("irc"));
        assert!(manifest.profiles.contains_key("full"));
    }

    #[test]
    fn test_normalize_irc_profile_aliases() {
        assert_eq!(normalize_profile_name("focus-chat"), "irc");
        assert_eq!(normalize_profile_name("immersive-chat"), "irc");
        assert_eq!(normalize_profile_name("irc"), "irc");
    }

    #[test]
    fn test_resolve_platform_info_accepts_release_platform_alias() {
        let comp: Component = serde_json::from_value(serde_json::json!({
            "install_path": "bin/example",
            "platforms": {
                "linux-arm64": {
                    "cid": "QmAlias",
                    "checksum": "sha256:deadbeef"
                }
            }
        }))
        .unwrap();

        let info = resolve_platform_info(&comp, "aarch64-linux").unwrap();
        assert_eq!(info.cid.as_deref(), Some("QmAlias"));
    }

    #[test]
    fn test_resolve_components_with_profile() {
        let json = r#"{
            "schema": "elastos.components/v1",
            "external": {
                "a": { "install_path": "bin/a", "platforms": {} },
                "b": { "install_path": "bin/b", "platforms": {} },
                "c": { "install_path": "bin/c", "platforms": {} }
            },
            "profiles": {
                "small": { "components": ["a"] },
                "big": { "components": ["a", "b", "c"] }
            }
        }"#;
        let manifest: ComponentsManifest = serde_json::from_str(json).unwrap();

        let result = resolve_components(&manifest, Some("small"), &[], &[]).unwrap();
        assert_eq!(result, vec!["a"]);

        let result = resolve_components(&manifest, Some("big"), &[], &["c".to_string()]).unwrap();
        assert_eq!(result, vec!["a", "b"]);

        let result = resolve_components(&manifest, Some("small"), &["b".to_string()], &[]).unwrap();
        assert_eq!(result, vec!["a", "b"]);
    }

    #[test]
    fn test_resolve_unknown_profile() {
        let json = r#"{
            "schema": "elastos.components/v1",
            "external": {},
            "profiles": { "x": { "components": [] } }
        }"#;
        let manifest: ComponentsManifest = serde_json::from_str(json).unwrap();
        let err = resolve_components(&manifest, Some("nope"), &[], &[]).unwrap_err();
        assert!(err.to_string().contains("Unknown profile"));
    }

    #[test]
    fn test_resolve_unknown_component() {
        let json = r#"{
            "schema": "elastos.components/v1",
            "external": {},
            "profiles": {}
        }"#;
        let manifest: ComponentsManifest = serde_json::from_str(json).unwrap();
        let err = resolve_components(&manifest, None, &["nope".to_string()], &[]).unwrap_err();
        assert!(err.to_string().contains("Unknown component"));
    }

    #[test]
    fn test_resolve_unknown_without() {
        let json = r#"{
            "schema": "elastos.components/v1",
            "external": { "a": { "install_path": "bin/a", "platforms": {} } },
            "profiles": { "p": { "components": ["a"] } }
        }"#;
        let manifest: ComponentsManifest = serde_json::from_str(json).unwrap();
        let err = resolve_components(&manifest, Some("p"), &[], &["typo".to_string()]).unwrap_err();
        assert!(err.to_string().contains("Unknown component in --without"));
    }

    #[test]
    fn test_component_install_state() {
        let tmp = tempfile::tempdir().unwrap();
        let comp = Component {
            version: None,
            install_path: Some("bin/test".to_string()),
            size_mb: None,
            description: None,
            platforms: HashMap::new(),
        };

        assert_eq!(
            component_install_state(tmp.path(), &comp, None),
            InstallState::Missing
        );

        let dest = tmp.path().join("bin/test");
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(&dest, b"binary").unwrap();

        assert_eq!(
            component_install_state(tmp.path(), &comp, None),
            InstallState::Installed
        );
    }

    #[test]
    fn test_component_install_state_detects_stale_checksum() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("bin/site-provider");
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(&dest, b"old-binary").unwrap();

        let mut platforms = HashMap::new();
        platforms.insert(
            "x86_64-linux".to_string(),
            PlatformInfo {
                url: None,
                cid: Some("QmSiteProvider".to_string()),
                release_path: Some("site-provider-linux-amd64".to_string()),
                checksum: Some(
                    "sha256:3314eb4927d668bd72f3b62d2802054cf67713b8e952b97969055f0a7d957697"
                        .to_string(),
                ),
                extract_path: None,
                install_path: Some("bin/site-provider".to_string()),
                strategy: None,
                source: None,
                note: None,
                size: Some(10),
            },
        );
        let comp = Component {
            version: Some("0.20.0-rc30".to_string()),
            install_path: Some("bin/site-provider".to_string()),
            size_mb: None,
            description: None,
            platforms,
        };

        assert_eq!(
            component_install_state(
                tmp.path(),
                &comp,
                resolve_platform_info(&comp, "x86_64-linux")
            ),
            InstallState::Stale("checksum mismatch".to_string())
        );
    }

    #[test]
    fn test_component_install_state_detects_stale_extracted_bundle_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let install_root = tmp.path().join("capsules/pc2");
        fs::create_dir_all(&install_root).unwrap();
        fs::write(install_root.join("capsule.json"), b"{\"name\":\"pc2\"}").unwrap();

        let mut platforms = HashMap::new();
        platforms.insert(
            "linux-amd64".to_string(),
            PlatformInfo {
                url: None,
                cid: Some("QmNewPc2".to_string()),
                release_path: Some("pc2-linux-amd64.tar.gz".to_string()),
                checksum: Some("sha256:new-pc2-archive".to_string()),
                extract_path: Some("pc2".to_string()),
                install_path: Some("capsules/pc2".to_string()),
                strategy: None,
                source: None,
                note: None,
                size: Some(1234),
            },
        );
        let component = Component {
            version: Some("0.1.0".to_string()),
            install_path: Some("capsules/pc2".to_string()),
            size_mb: None,
            description: None,
            platforms,
        };
        let manifest: ComponentsManifest = serde_json::from_value(serde_json::json!({
            "external": {
                "pc2": {
                    "version": "0.1.0",
                    "install_path": "capsules/pc2",
                    "platforms": {
                        "linux-amd64": {
                            "cid": "QmNewPc2",
                            "release_path": "pc2-linux-amd64.tar.gz",
                            "checksum": "sha256:new-pc2-archive",
                            "extract_path": "pc2",
                            "install_path": "capsules/pc2"
                        }
                    }
                }
            },
            "capsules": {},
            "profiles": {}
        }))
        .unwrap();

        assert_eq!(
            component_install_state_for_name(
                &manifest,
                tmp.path(),
                "pc2",
                &component,
                resolve_platform_info(&component, "linux-amd64")
            ),
            InstallState::Stale("extracted bundle CID metadata missing or stale".to_string())
        );
    }

    #[test]
    fn test_build_gateway_list_without_sources_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let gateways = build_gateway_list(tmp.path());
        assert!(gateways.is_empty());
    }

    #[test]
    fn test_build_gateway_list_prefers_trusted_source_gateways() {
        let tmp = tempfile::tempdir().unwrap();
        let config = TrustedSourcesConfig {
            schema: "elastos.trusted-sources/v1".to_string(),
            default_source: "seed".to_string(),
            sources: vec![TrustedSource {
                name: "seed".to_string(),
                publisher_dids: vec!["did:key:z6Mktest".to_string()],
                channel: "jetson-test".to_string(),
                discovery_uri: String::new(),
                connect_ticket: String::new(),
                gateways: vec![
                    "https://elastos.elacitylabs.com".to_string(),
                    "https://publisher-backup.example".to_string(),
                ],
                install_path: String::new(),
                installed_version: String::new(),
                head_cid: String::new(),
                publisher_node_id: String::new(),
                ipns_name: String::new(),
            }],
        };
        save_trusted_sources(tmp.path(), &config).unwrap();

        let gateways = build_gateway_list(tmp.path());
        assert_eq!(
            gateways,
            vec![
                ElastosFetchPath {
                    description: "trusted source fetch path (https://elastos.elacitylabs.com)"
                        .to_string(),
                    transport_base: "https://elastos.elacitylabs.com".to_string(),
                },
                ElastosFetchPath {
                    description: "trusted source fetch path (https://publisher-backup.example)"
                        .to_string(),
                    transport_base: "https://publisher-backup.example".to_string(),
                }
            ]
        );
    }

    #[test]
    fn test_trusted_gateway_overrides_reads_saved_source_gateways() {
        let tmp = tempfile::tempdir().unwrap();
        let config = TrustedSourcesConfig {
            schema: "elastos.trusted-sources/v1".to_string(),
            default_source: "seed".to_string(),
            sources: vec![TrustedSource {
                name: "seed".to_string(),
                publisher_dids: vec!["did:key:z6Mktest".to_string()],
                channel: "jetson-test".to_string(),
                discovery_uri: String::new(),
                connect_ticket: String::new(),
                gateways: vec![
                    "https://elastos.elacitylabs.com/".to_string(),
                    " https://elastos.elacitylabs.com ".to_string(),
                    "https://backup.example".to_string(),
                ],
                install_path: String::new(),
                installed_version: String::new(),
                head_cid: String::new(),
                publisher_node_id: String::new(),
                ipns_name: String::new(),
            }],
        };
        save_trusted_sources(tmp.path(), &config).unwrap();

        let gateways = trusted_gateway_overrides(tmp.path());
        assert_eq!(
            gateways,
            vec![
                ElastosFetchPath {
                    description: "trusted source fetch path (https://elastos.elacitylabs.com)"
                        .to_string(),
                    transport_base: "https://elastos.elacitylabs.com".to_string(),
                },
                ElastosFetchPath {
                    description: "trusted source fetch path (https://backup.example)".to_string(),
                    transport_base: "https://backup.example".to_string(),
                }
            ]
        );
    }

    #[test]
    fn test_verify_installed_component_binary_rejects_dev_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dev-shell");
        fs::write(&path, b"dev-shell").unwrap();

        let err = verify_installed_component_binary(tmp.path(), "shell", &path)
            .unwrap_err()
            .to_string();
        assert!(err.contains("must resolve from an installed runtime path"));
    }

    #[test]
    fn test_verify_installed_component_binary_requires_checksum() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let bin_dir = data_dir.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let install_path = bin_dir.join("shell");
        fs::write(&install_path, b"shell-binary").unwrap();
        fs::write(
            data_dir.join("components.json"),
            r#"{
  "external": {
    "shell": {
      "install_path": "bin/shell",
      "platforms": {
        "linux-amd64": {
          "checksum": "",
          "url": "https://example.invalid/shell"
        }
      }
    }
  },
  "capsules": {},
  "profiles": {}
}"#,
        )
        .unwrap();

        let err = verify_installed_component_binary(data_dir, "shell", &install_path)
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing checksum"));
    }

    #[test]
    fn test_verify_installed_component_binary_verifies_checksum() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let bin_dir = data_dir.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let install_path = bin_dir.join("shell");
        let bytes = b"shell-binary";
        fs::write(&install_path, bytes).unwrap();
        let checksum = format!("sha256:{}", hex::encode(sha2::Sha256::digest(bytes)));
        fs::write(
            data_dir.join("components.json"),
            format!(
                r#"{{
  "external": {{
    "shell": {{
      "install_path": "bin/shell",
      "platforms": {{
        "linux-amd64": {{
          "checksum": "{}",
          "url": "https://example.invalid/shell"
        }}
      }}
    }}
  }},
  "capsules": {{}},
  "profiles": {{}}
}}"#,
                checksum
            ),
        )
        .unwrap();

        let result = verify_installed_component_binary(data_dir, "shell", &install_path).unwrap();
        assert_eq!(result, checksum);
    }

    #[test]
    fn test_write_installed_manifest_stamps_local_copy_checksum() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let bin_dir = data_dir.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();

        let source_path = data_dir.join("Image");
        let install_path = bin_dir.join("vmlinux");
        fs::write(&source_path, b"arm64-kernel").unwrap();
        fs::write(&install_path, b"arm64-kernel").unwrap();

        let manifest: ComponentsManifest = serde_json::from_value(serde_json::json!({
            "external": {
                "vmlinux": {
                    "install_path": "bin/vmlinux",
                    "platforms": {
                        "linux-amd64": {
                            "strategy": "local-copy",
                            "source": source_path.to_string_lossy(),
                            "install_path": "bin/vmlinux"
                        }
                    }
                }
            },
            "capsules": {},
            "profiles": {}
        }))
        .unwrap();

        let stamped = write_installed_manifest(data_dir, &manifest, "linux-amd64").unwrap();
        assert_eq!(stamped, vec!["vmlinux".to_string()]);

        let result = verify_installed_component_binary(data_dir, "vmlinux", &install_path).unwrap();
        assert!(result.starts_with("sha256:"));
    }

    #[test]
    fn test_component_install_state_detects_local_copy_checksum_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let source_path = data_dir.join("Image");
        let install_path = data_dir.join("bin/vmlinux");
        fs::create_dir_all(install_path.parent().unwrap()).unwrap();
        fs::write(&source_path, b"same-size-data").unwrap();
        fs::write(&install_path, b"same-size-date").unwrap();

        let mut platforms = HashMap::new();
        platforms.insert(
            "linux-amd64".to_string(),
            PlatformInfo {
                url: None,
                cid: None,
                release_path: None,
                checksum: Some(format!(
                    "sha256:{}",
                    hex::encode(sha2::Sha256::digest(b"same-size-data"))
                )),
                extract_path: None,
                install_path: Some("bin/vmlinux".to_string()),
                strategy: Some("local-copy".to_string()),
                source: Some(source_path.to_string_lossy().to_string()),
                note: None,
                size: Some(b"same-size-data".len() as u64),
            },
        );
        let comp = Component {
            version: None,
            install_path: Some("bin/vmlinux".to_string()),
            size_mb: None,
            description: None,
            platforms,
        };

        assert_eq!(
            component_install_state(data_dir, &comp, resolve_platform_info(&comp, "linux-amd64")),
            InstallState::Stale("checksum mismatch".to_string())
        );
    }

    #[test]
    fn test_resolve_component_download_url_prefers_cid_over_baked_url() {
        let info = PlatformInfo {
            url: Some("https://old.example/ipfs/QmOld".to_string()),
            cid: Some("QmCanonical".to_string()),
            release_path: Some("shell-linux-amd64".to_string()),
            checksum: None,
            extract_path: None,
            install_path: None,
            strategy: None,
            source: None,
            note: None,
            size: None,
        };

        assert_eq!(
            resolve_component_download_url(&info).as_deref(),
            Some("elastos://QmCanonical")
        );
    }

    #[tokio::test]
    async fn test_refresh_installed_components_for_update_refreshes_changed_local_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let old_source = data_dir.join("old-localhost-provider");
        let new_source = data_dir.join("new-localhost-provider");
        fs::write(&old_source, b"old-binary").unwrap();
        fs::write(&new_source, b"new-binary").unwrap();

        let install_path = data_dir.join("bin/localhost-provider");
        fs::create_dir_all(install_path.parent().unwrap()).unwrap();
        fs::write(&install_path, b"old-binary").unwrap();

        let old_manifest = serde_json::json!({
            "external": {
                "localhost-provider": {
                    "version": "0.1.0",
                    "install_path": "bin/localhost-provider",
                    "platforms": {
                        "x86_64-linux": {
                            "strategy": "local-copy",
                            "source": old_source.to_string_lossy(),
                            "install_path": "bin/localhost-provider"
                        }
                    }
                }
            },
            "capsules": {},
            "profiles": {}
        });
        let new_manifest = serde_json::json!({
            "external": {
                "localhost-provider": {
                    "version": "0.2.0",
                    "install_path": "bin/localhost-provider",
                    "platforms": {
                        "x86_64-linux": {
                            "strategy": "local-copy",
                            "source": new_source.to_string_lossy(),
                            "install_path": "bin/localhost-provider"
                        }
                    }
                }
            },
            "capsules": {},
            "profiles": {}
        });

        let refreshed = refresh_installed_components_for_update(
            data_dir,
            Some(&serde_json::to_vec(&old_manifest).unwrap()),
            &serde_json::to_vec(&new_manifest).unwrap(),
            "x86_64-linux",
        )
        .await
        .unwrap();

        assert_eq!(refreshed, vec!["localhost-provider".to_string()]);
        assert_eq!(fs::read(&install_path).unwrap(), b"new-binary");
    }

    #[tokio::test]
    async fn test_refresh_installed_components_for_update_refreshes_stale_alias_platform() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let old_source = data_dir.join("old-localhost-provider");
        let new_source = data_dir.join("new-localhost-provider");
        let install_path = data_dir.join("bin/localhost-provider");
        fs::write(&old_source, b"old-binary").unwrap();
        fs::write(&new_source, b"new-binary-with-more-bytes").unwrap();
        fs::create_dir_all(install_path.parent().unwrap()).unwrap();
        fs::write(&install_path, b"old-binary").unwrap();

        let old_manifest = serde_json::json!({
            "external": {
                "localhost-provider": {
                    "version": "0.1.0",
                    "install_path": "bin/localhost-provider",
                    "platforms": {
                        "linux-arm64": {
                            "strategy": "local-copy",
                            "source": old_source.to_string_lossy(),
                            "install_path": "bin/localhost-provider"
                        }
                    }
                }
            },
            "capsules": {},
            "profiles": {}
        });
        let new_manifest = serde_json::json!({
            "external": {
                "localhost-provider": {
                    "version": "0.2.0",
                    "install_path": "bin/localhost-provider",
                    "platforms": {
                        "linux-arm64": {
                            "strategy": "local-copy",
                            "source": new_source.to_string_lossy(),
                            "install_path": "bin/localhost-provider"
                        }
                    }
                }
            },
            "capsules": {},
            "profiles": {}
        });
        let refreshed = refresh_installed_components_for_update(
            data_dir,
            Some(&serde_json::to_vec(&old_manifest).unwrap()),
            &serde_json::to_vec(&new_manifest).unwrap(),
            "aarch64-linux",
        )
        .await
        .unwrap();

        assert_eq!(refreshed, vec!["localhost-provider".to_string()]);
        assert_eq!(
            fs::read(&install_path).unwrap(),
            b"new-binary-with-more-bytes"
        );
    }
}
