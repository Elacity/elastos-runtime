//! `elastos setup` — Default component provisioning
//!
//! Downloads and installs external components into `~/.local/share/elastos/`.
//! Assistant acquires its signed engine on demand; model directories use the
//! bounded preparation path, not this archive installer.

use crate::api::capsule_inventory::MAX_MODEL_CATALOG_BYTES;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::HashMap;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;

mod browser_vm_image;
#[cfg(unix)]
mod local_model_engine_receipt;

const DEFAULT_SETUP_PROFILE: &str = "home";
const CACHED_CID_FILE: &str = ".elastos-cid";
const CACHED_ARTIFACT_SHA_FILE: &str = ".elastos-artifact-sha256";
const PROVIDER_ICON_SIZES: [u16; 4] = [32, 64, 128, 256];

// ── Manifest types ──────────────────────────────────────────────────

#[derive(Deserialize, Serialize, Clone)]
pub struct ComponentsManifest {
    /// Setup-materialized artifacts: external tools, provider binaries, and
    /// first-party app bundles installed into the runtime data directory.
    pub external: HashMap<String, Component>,

    /// Capsule registry (CID-based entries). Defaults empty when a manifest has no capsules.
    /// Consumed by supervisor in M2+ (ensure_capsule, launch_capsule).
    #[serde(default)]
    pub capsules: HashMap<String, CapsuleEntry>,

    pub profiles: HashMap<String, Profile>,

    /// Operator-pinned signed model catalog. Absence keeps installed inventory
    /// unchanged; catalog entries cannot supply their own trust configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_catalog: Option<ModelCatalogConfig>,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ModelCatalogConfig {
    pub head_cid: String,
    pub publisher_dids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_use: Option<ModelLocalUseConfig>,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ModelLocalUseConfig {
    #[serde(deserialize_with = "deserialize_model_local_use_budget")]
    pub max_cache_bytes: u64,
    #[serde(deserialize_with = "deserialize_model_local_use_budget")]
    pub max_model_memory_bytes: u64,
}

fn deserialize_model_local_use_budget<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value == 0 || value > i64::MAX as u64 {
        return Err(serde::de::Error::custom(
            "model local-use budget is out of range",
        ));
    }
    Ok(value)
}

/// A setup-materialized component.
///
/// Published artifacts use signed release paths over Carrier. CID-only operator
/// entries require configured trusted-source gateways. Local development uses
/// the explicit source-build and local-copy strategies.
#[derive(Deserialize, Serialize, Clone)]
pub struct Component {
    pub version: Option<String>,
    #[serde(default)]
    pub install_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default)]
    pub size_mb: Option<u64>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_runtime: Option<ProviderRuntime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capsule_metadata: Option<ComponentCapsuleMetadata>,
    pub platforms: HashMap<String, PlatformInfo>,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct ComponentCapsuleMetadata {
    #[serde(default)]
    pub install_path: Option<String>,
    pub platforms: HashMap<String, PlatformInfo>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderRuntime {
    pub role: ProviderRuntimeRole,
    pub substrate: ProviderRuntimeSubstrate,
    pub runtime_abi: ProviderRuntimeAbi,
    pub execution: ProviderRuntimeExecution,
    pub provides: String,
    #[serde(default)]
    pub runtime_only: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderRuntimeRole {
    Provider,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderRuntimeSubstrate {
    Native,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub enum ProviderRuntimeAbi {
    #[serde(rename = "elastos.provider-stdio/v1")]
    ProviderStdioV1,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderRuntimeExecution {
    NativeProvider,
}

/// A capsule registry entry (CID-based, resolved via IPFS gateways).
/// Consumed by supervisor in M2+ (ensure_capsule, launch_capsule).
#[derive(Deserialize, Serialize, Clone)]
pub struct CapsuleEntry {
    pub cid: String,
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default)]
    pub platforms: Vec<String>,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct PlatformInfo {
    /// Legacy artifact hint; setup never fetches this URL.
    pub url: Option<String>,
    /// IPFS CID for content-addressed downloads (used instead of url).
    pub cid: Option<String>,
    #[serde(default)]
    pub release_path: Option<String>,
    pub checksum: Option<String>,
    pub extract_path: Option<String>,
    #[serde(default)]
    pub install_path: Option<String>,
    /// Executable path inside a directory-valued extracted bundle.
    #[serde(default)]
    pub binary_path: Option<String>,
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

pub fn validate_provider_runtime<'a>(
    name: &str,
    component: &'a Component,
) -> anyhow::Result<&'a ProviderRuntime> {
    let runtime = component.provider_runtime.as_ref().ok_or_else(|| {
        anyhow::anyhow!("component '{}' is missing provider runtime metadata", name)
    })?;
    let provides = runtime.provides.trim();
    if provides.is_empty() {
        anyhow::bail!(
            "component '{}' provider runtime provides must not be empty",
            name
        );
    }
    if provides != runtime.provides {
        anyhow::bail!(
            "component '{}' provider runtime provides must not contain surrounding whitespace",
            name
        );
    }
    if runtime.runtime_only {
        if provides.starts_with('-')
            || provides.ends_with('-')
            || !provides
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            anyhow::bail!(
                "component '{}' Runtime-only provider target is invalid",
                name
            );
        }
    } else if !(provides.starts_with("elastos://") || provides.starts_with("localhost://")) {
        anyhow::bail!(
            "component '{}' provider runtime provides must use elastos:// or localhost://",
            name
        );
    }
    Ok(runtime)
}

// ── Entry point ─────────────────────────────────────────────────────

pub async fn run(
    profile: Option<String>,
    with: Vec<String>,
    without: Vec<String>,
    list: bool,
    prerequisites_only: bool,
    media_tools_dir: Option<PathBuf>,
) -> anyhow::Result<()> {
    let data_dir = data_dir()?;
    run_with_data_dir(
        data_dir,
        profile,
        with,
        without,
        list,
        prerequisites_only,
        media_tools_dir,
    )
    .await
}

async fn run_with_data_dir(
    data_dir: PathBuf,
    profile: Option<String>,
    with: Vec<String>,
    without: Vec<String>,
    list: bool,
    prerequisites_only: bool,
    media_tools_dir: Option<PathBuf>,
) -> anyhow::Result<()> {
    if media_tools_dir.is_some() && !prerequisites_only {
        anyhow::bail!("--media-tools-dir requires --prerequisites-only");
    }
    let _writer = if list {
        None
    } else {
        crate::install_transaction::acquire_installed_writer(&data_dir)?
    };
    let signed_setup = if list {
        None
    } else {
        admit_installed_setup_metadata(&data_dir, _writer.as_ref()).await?
    };
    let (manifest_path, manifest, signed_setup) = match signed_setup {
        Some((manifest, metadata)) => (data_dir.join("components.json"), manifest, Some(metadata)),
        None => {
            let (path, manifest) = load_manifest_with_path()?;
            (path, manifest, None)
        }
    };
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
        println!(
            "Using default setup profile: {} (managed Home).",
            DEFAULT_SETUP_PROFILE
        );
        println!("Use `--profile demo` for site/share/browser demo surfaces.");
        println!("Use `--profile blockchain` for typed chain/wallet/DRM provider development.");
        println!("Use `--profile operator` for explicit serve/node/run/agent flows.");
        println!();
    } else if let Some(selected_profile) = selected_profile {
        match selected_profile {
            "home" => {
                println!("Selected profile: home (managed Home)");
                println!();
            }
            "demo" => {
                println!("Selected profile: demo (Home + site/share/browser demo surfaces)");
                println!();
            }
            "blockchain" => {
                println!(
                    "Selected profile: blockchain (typed chain/wallet/DRM provider development)"
                );
                println!();
            }
            "operator" => {
                println!("Selected profile: operator (explicit serve/node/run/agent runtime)");
                println!();
            }
            _ => {}
        }
    }

    let components = resolve_components(&manifest, selected_profile, &with, &without)?;

    if components.iter().any(|name| name == browser_vm_image::NAME) {
        browser_vm_image::component_info(&manifest, &platform)?;
    }

    if components.is_empty() {
        println!("No components selected.");
        println!("Use --with <component> to add components, or --list to see available profiles/components.");
        return Ok(());
    }

    if let Some(metadata) = &signed_setup {
        metadata.install(&data_dir)?;
    }

    prepare_selected_component_prerequisites(
        &data_dir,
        &manifest,
        &platform,
        &components,
        &ipfs_gateways,
        media_tools_dir.as_deref(),
    )
    .await?;
    if prerequisites_only {
        println!("Selected component prerequisites are ready.");
        return Ok(());
    }

    println!("Components to install:");
    for name in &components {
        let comp = &manifest.external[name];
        let platform_info = resolve_platform_info(comp, &platform);
        if name == "llama-server" {
            verify_arm64_model_host(&platform)?;
        }
        let status = match effective_component_install_state_for_name(
            &manifest,
            &data_dir,
            name,
            comp,
            platform_info,
            &platform,
        ) {
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

        let binary_install_state =
            component_install_state_for_name(&manifest, &data_dir, name, comp, platform_info);
        let metadata_install_state =
            capsule_metadata_install_state_for_name(&data_dir, name, comp, &platform);
        match effective_component_install_state_for_name(
            &manifest,
            &data_dir,
            name,
            comp,
            platform_info,
            &platform,
        ) {
            InstallState::Installed => {
                if let Some(platform_info) = platform_info {
                    ensure_bundle_executable_link(&data_dir, name, platform_info)?;
                }
                println!("[skip] {} — already installed", name);
                skipped_count += 1;
                continue;
            }
            InstallState::Stale(reason) => {
                println!("[refresh] {} — {}", name, reason);
            }
            InstallState::Missing => {}
        }

        let mut changed = false;
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

        if platform_info.strategy.as_deref() == Some("local-copy")
            && !matches!(binary_install_state, InstallState::Installed)
        {
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
            set_local_copy_permissions(&source, &dest);
            write_cache_metadata(&manifest, Some(platform_info), &platform, name, &dest)?;
            println!("  Installed: {}", dest.display());
            changed = true;
        } else if !matches!(binary_install_state, InstallState::Installed) {
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
                FirstPartyCarrierContext::Setup,
            )
            .await?;
            write_cache_metadata(&manifest, Some(platform_info), &platform, name, &dest)?;
            changed = true;
        }

        if matches!(
            metadata_install_state,
            Some(InstallState::Missing | InstallState::Stale(_))
        ) {
            ensure_component_capsule_metadata(
                &data_dir,
                name,
                comp,
                &platform,
                &ipfs_gateways,
                FirstPartyCarrierContext::Setup,
            )
            .await?;
            changed = true;
        }

        if changed {
            installed_count += 1;
        } else {
            skipped_count += 1;
        }
    }

    let stamped = if signed_setup.is_some() {
        // Signed component bytes include fields unknown to this Runtime. Their
        // release descriptor remains authoritative after setup.
        Vec::new()
    } else {
        let stamped = write_installed_manifest(&data_dir, &manifest, &platform)?;
        install_signed_model_catalog(&data_dir, &manifest, &manifest_path)?;
        stamped
    };

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

const COMPONENTS_MANIFEST_ENV: &str = "ELASTOS_COMPONENTS_MANIFEST";

struct SignedSetupMetadata {
    components: Vec<u8>,
    catalog: Option<Vec<u8>>,
}

impl SignedSetupMetadata {
    fn install(&self, data_dir: &Path) -> anyhow::Result<()> {
        // Admission of both inputs precedes the first metadata or component write.
        if let Some(bytes) = &self.catalog {
            atomic_write_file(&data_dir.join(MODEL_CATALOG_FILE), bytes)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(
                    data_dir.join(MODEL_CATALOG_FILE),
                    fs::Permissions::from_mode(0o600),
                )?;
            }
        }
        atomic_write_file(&data_dir.join("components.json"), &self.components)
    }
}

async fn admit_installed_setup_metadata(
    data_dir: &Path,
    guard: Option<&crate::install_transaction::InstallationGuard>,
) -> anyhow::Result<Option<(ComponentsManifest, SignedSetupMetadata)>> {
    let sources = crate::sources::load_trusted_sources(data_dir)?;
    let source = sources.default_source();
    let has_consumed = match fs::symlink_metadata(data_dir.join("installation")) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    if !has_consumed && source.is_none_or(|source| source.install_path.is_empty()) {
        return Ok(None);
    }
    let source = source.ok_or_else(missing_trusted_source_error)?;
    let guard =
        guard.ok_or_else(|| anyhow::anyhow!("installed setup requires its installation writer"))?;
    let binary = fs::canonicalize(&source.install_path)?;
    let admitted = crate::installed_release::read_for_setup(data_dir, &binary, source, guard)?;
    let release: serde_json::Value = serde_json::from_slice(&admitted.release)?;
    let descriptor =
        &release["payload"]["platforms"][crate::update::detect_release_platform()]["components"];
    let size = descriptor["size"]
        .as_u64()
        .filter(|size| (1..=4 * 1024 * 1024).contains(size))
        .ok_or_else(|| anyhow::anyhow!("signed components size is missing or exceeds its bound"))?;
    let path = format!(
        "components-{}.json",
        crate::update::detect_release_platform()
    );
    let client = crate::carrier::CarrierClient::connect_trusted_source(source, 15).await?;
    let result = async {
        let mut components = Vec::with_capacity(size as usize);
        client
            .fetch_file_to(&path, &mut components, size, &mut |_, _| Ok(()))
            .await?;
        crate::installed_release::admit_descriptor(
            descriptor,
            &hex::encode(sha2::Sha256::digest(&components)),
            components.len() as u64,
        )?;
        let manifest: ComponentsManifest = serde_json::from_slice(&components)?;
        let catalog = if let Some(trust) = &manifest.model_catalog {
            let bytes = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                client.fetch_file_bounded(MODEL_CATALOG_FILE, MAX_MODEL_CATALOG_BYTES),
            )
            .await
            .map_err(|_| anyhow::anyhow!("model catalogue Carrier fetch timed out"))??;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs();
            crate::api::capsule_inventory::verify_model_catalog(trust, &bytes, now)?;
            Some(bytes)
        } else {
            None
        };
        Ok::<_, anyhow::Error>((
            manifest,
            SignedSetupMetadata {
                components,
                catalog,
            },
        ))
    }
    .await;
    client.close().await;
    let metadata = result?;
    // The same private pair and writer still own the destination after transport.
    let current = crate::installed_release::read_for_setup(data_dir, &binary, source, guard)?;
    anyhow::ensure!(
        current.head == admitted.head && current.release == admitted.release,
        "installed release inputs changed during setup"
    );
    Ok(Some(metadata))
}

#[cfg(test)]
fn load_manifest() -> anyhow::Result<ComponentsManifest> {
    Ok(load_manifest_with_path()?.1)
}

fn load_manifest_with_path() -> anyhow::Result<(PathBuf, ComponentsManifest)> {
    if let Some(path) = explicit_components_manifest_path() {
        let manifest = load_manifest_from_path(&path)?;
        return Ok((path, manifest));
    }

    let exe_path = std::env::current_exe().ok();
    let manifest_paths = [
        // Source checkout layout: <repo>/elastos/target/{debug,release}/elastos.
        // Source-run commands should test the checkout, not a stale installed manifest.
        exe_path.as_deref().and_then(source_checkout_manifest_path),
        // Exe-relative (release tarball)
        exe_path
            .as_deref()
            .and_then(|p| p.parent().map(|d| d.join("components.json"))),
        // Installed layout
        dirs::data_dir().map(|d| d.join("elastos/components.json")),
    ];

    for path in manifest_paths.iter().flatten() {
        if let Ok(content) = fs::read_to_string(path) {
            return parse_manifest_at(path, &content).map(|manifest| (path.clone(), manifest));
        }
    }

    anyhow::bail!("{}", missing_manifest_message())
}

fn explicit_components_manifest_path() -> Option<PathBuf> {
    std::env::var_os(COMPONENTS_MANIFEST_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn load_manifest_from_path(path: &Path) -> anyhow::Result<ComponentsManifest> {
    let content = fs::read_to_string(path).map_err(|e| {
        anyhow::anyhow!(
            "Failed to read components.json at {}: {}",
            path.display(),
            e
        )
    })?;
    parse_manifest_at(path, &content)
}

fn parse_manifest_at(path: &Path, content: &str) -> anyhow::Result<ComponentsManifest> {
    serde_json::from_str(content)
        .map_err(|e| anyhow::anyhow!("Invalid components.json at {}: {}", path.display(), e))
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

fn source_checkout_manifest_path(exe_path: &Path) -> Option<PathBuf> {
    let exe_dir = exe_path.parent()?;
    let parent = exe_dir.parent()?;
    let grandparent = parent.parent()?;
    let great_grandparent = grandparent.parent()?;

    if parent.file_name()?.to_str()? == "target" && grandparent.file_name()?.to_str()? == "elastos"
    {
        return Some(great_grandparent.join("components.json"));
    }

    if exe_dir.file_name()?.to_str()? == "deps"
        && grandparent.file_name()?.to_str()? == "target"
        && great_grandparent.file_name()?.to_str()? == "elastos"
    {
        return Some(great_grandparent.parent()?.join("components.json"));
    }

    None
}

fn missing_manifest_message() -> String {
    "components.json not found. Searched:\n  \
     $ELASTOS_COMPONENTS_MANIFEST when set\n  \
     ~/.local/share/elastos/components.json\n  \
     <exe-dir>/components.json\n  \
     <source-checkout>/components.json\n\n\
     Source-built binaries are not self-contained installs.\n\
     Use the published installer, run the binary from the repo checkout,\n\
     set ELASTOS_COMPONENTS_MANIFEST to a generated manifest,\n\
     or place components.json next to the binary or in ~/.local/share/elastos/."
        .to_string()
}

fn missing_trusted_source_error() -> anyhow::Error {
    anyhow::anyhow!(
        "No trusted source configured.\n\
         `elastos setup` installs first-party artifacts from a trusted source over Carrier.\n\
         For a published install, run the stamped installer first.\n\
         For a source checkout, create your own trusted source or add one with `elastos source add ...`."
    )
}

fn set_local_copy_permissions(source: &Path, dest: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = match fs::metadata(source) {
            Ok(metadata) if metadata.permissions().mode() & 0o111 != 0 => 0o755,
            Ok(_) => 0o644,
            Err(_) => 0o644,
        };
        let _ = fs::set_permissions(dest, fs::Permissions::from_mode(mode));
    }
}

// ── Platform detection ──────────────────────────────────────────────

pub fn detect_platform() -> String {
    let os = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "darwin"
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
    let Some(install_root) = installed_component_root_for_path(data_dir, name, path) else {
        anyhow::bail!(
            "{} must resolve from an installed runtime path, got dev/override path {}",
            name,
            path.display()
        );
    };

    let manifest_path = install_root.join("components.json");
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

fn installed_component_root_for_path(data_dir: &Path, name: &str, path: &Path) -> Option<PathBuf> {
    let installed_bin = data_dir.join("bin").join(name);
    if path == installed_bin {
        return Some(data_dir.to_path_buf());
    }

    let installed_capsule = data_dir.join("capsules").join(name).join(name);
    if path == installed_capsule {
        return Some(data_dir.to_path_buf());
    }

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let exe_root = exe_dir.join("../share/elastos");
            if path == exe_root.join("bin").join(name) {
                return Some(exe_root);
            }
        }
    }

    if path.file_name().and_then(|value| value.to_str()) != Some(name) {
        return None;
    }

    let parent = path.parent()?;
    if parent.file_name().and_then(|value| value.to_str()) == Some("bin") {
        return parent.parent().map(Path::to_path_buf);
    }

    let capsule_dir = parent;
    if capsule_dir.file_name().and_then(|value| value.to_str()) != Some(name) {
        return None;
    }
    let capsules_root = capsule_dir.parent()?;
    if capsules_root.file_name().and_then(|value| value.to_str()) != Some("capsules") {
        return None;
    }
    capsules_root.parent().map(Path::to_path_buf)
}

// ── Component status ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum InstallState {
    Missing,
    Installed,
    Stale(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CapsuleComponentEnsure {
    pub name: String,
    pub status: String,
    pub detail: Option<String>,
}

/// Prepare only the image dependency of an explicitly selected local Engine.
/// Host helper admission and page allocation remain with the Engine adapter.
pub async fn ensure_browser_vm_image_for_local_engine(data_dir: &Path) -> anyhow::Result<()> {
    let check_dir = data_dir.to_path_buf();
    let pending = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<PlatformInfo>> {
        let platform = detect_platform();
        let manifest_path = check_dir.join("components.json");
        if !manifest_path.exists() {
            browser_vm_image::verify_legacy_or_missing_release(&check_dir, &platform)?;
            return Ok(None);
        }
        let manifest = load_manifest_from_path(&manifest_path)?;
        if !manifest.external.contains_key(browser_vm_image::NAME) {
            browser_vm_image::verify_legacy_or_missing_release(&check_dir, &platform)?;
            return Ok(None);
        }
        let info = browser_vm_image::component_info(&manifest, &platform)?;
        Ok(
            browser_vm_image::verify_installed(&check_dir, info, &platform)
                .is_err()
                .then(|| info.clone()),
        )
    })
    .await??;
    let Some(info) = pending else {
        return Ok(());
    };
    let dest = data_dir.join(browser_vm_image::INSTALL_PATH);
    download_component(
        data_dir,
        browser_vm_image::NAME,
        &resolve_component_download_url(&info).expect("validated Browser image release path"),
        &info,
        &dest,
        &build_gateway_list(data_dir),
        FirstPartyCarrierContext::Runtime,
    )
    .await
}

pub(crate) async fn ensure_capsule_component_for_home_launch(
    data_dir: &Path,
    name: &str,
) -> anyhow::Result<CapsuleComponentEnsure> {
    let name = name.trim();
    if name.is_empty() {
        anyhow::bail!("capsule component name is required");
    }

    let manifest_path = data_dir.join("components.json");
    let manifest_bytes = match fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(_) => {
            return Ok(CapsuleComponentEnsure {
                name: name.to_string(),
                status: "skipped".to_string(),
                detail: Some(
                    "installed components manifest unavailable; using local capsule tree"
                        .to_string(),
                ),
            })
        }
    };
    let manifest: ComponentsManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|err| anyhow::anyhow!("invalid {}: {}", manifest_path.display(), err))?;

    let Some(component) = manifest.external.get(name) else {
        return Ok(CapsuleComponentEnsure {
            name: name.to_string(),
            status: "skipped".to_string(),
            detail: Some("not a setup-managed capsule component".to_string()),
        });
    };
    let platform = detect_platform();
    let platform_info = resolve_platform_info(component, &platform).ok_or_else(|| {
        anyhow::anyhow!("capsule component '{name}' is not available for {platform}")
    })?;
    let install_path = resolve_install_path(component, Some(platform_info))
        .ok_or_else(|| anyhow::anyhow!("capsule component '{name}' is missing install_path"))?;
    validate_capsule_component_install_path(name, install_path)?;

    let install_state =
        component_install_state_for_name(&manifest, data_dir, name, component, Some(platform_info));
    if matches!(install_state, InstallState::Installed) {
        return Ok(CapsuleComponentEnsure {
            name: name.to_string(),
            status: "installed".to_string(),
            detail: None,
        });
    }

    validate_capsule_package_identity(name, manifest.capsules.get(name), platform_info)?;
    if platform_info.strategy.as_deref() == Some("source-build") {
        anyhow::bail!(
            "capsule component '{name}' requires a source build and cannot be JIT materialized"
        );
    }
    if platform_info.strategy.as_deref() == Some("local-copy") {
        anyhow::bail!("capsule component '{name}' uses local-copy and cannot be JIT materialized");
    }

    let resolved_url = resolve_component_download_url(platform_info).ok_or_else(|| {
        anyhow::anyhow!("capsule component '{name}' has no Carrier/content release path")
    })?;
    let dest = data_dir.join(install_path);
    let gateways = build_gateway_list(data_dir);
    download_component(
        data_dir,
        name,
        &resolved_url,
        platform_info,
        &dest,
        &gateways,
        FirstPartyCarrierContext::Runtime,
    )
    .await?;
    write_cache_metadata(&manifest, Some(platform_info), &platform, name, &dest)?;

    let installed_manifest = dest.join("capsule.json");
    let manifest_bytes = fs::read(&installed_manifest).map_err(|err| {
        anyhow::anyhow!("materialized capsule '{name}' is missing capsule.json: {err}")
    })?;
    let manifest: elastos_common::CapsuleManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|err| {
            anyhow::anyhow!("materialized capsule '{name}' has invalid capsule.json: {err}")
        })?;
    manifest.validate().map_err(|err| {
        anyhow::anyhow!("materialized capsule '{name}' failed manifest validation: {err}")
    })?;
    if manifest.name != name {
        anyhow::bail!(
            "materialized capsule name mismatch: requested '{}', package declared '{}'",
            name,
            manifest.name
        );
    }

    Ok(CapsuleComponentEnsure {
        name: name.to_string(),
        status: "materialized".to_string(),
        detail: match install_state {
            InstallState::Missing => Some("installed package was missing".to_string()),
            InstallState::Stale(reason) => Some(format!("installed package was stale: {reason}")),
            InstallState::Installed => None,
        },
    })
}

async fn ensure_component_capsule_metadata(
    data_dir: &Path,
    name: &str,
    component: &Component,
    platform: &str,
    ipfs_gateways: &[ElastosFetchPath],
    carrier_context: FirstPartyCarrierContext,
) -> anyhow::Result<()> {
    let Some(metadata) = component.capsule_metadata.as_ref() else {
        return Ok(());
    };
    let platform_info = resolve_component_capsule_metadata_platform_info(metadata, platform)
        .ok_or_else(|| {
            anyhow::anyhow!("provider capsule metadata '{name}' is not available for {platform}")
        })?;
    let install_path =
        resolve_component_capsule_metadata_install_path(metadata, Some(platform_info)).ok_or_else(
            || anyhow::anyhow!("provider capsule metadata '{name}' is missing install_path"),
        )?;
    validate_capsule_component_install_path(name, install_path)?;
    if platform_info.strategy.as_deref() == Some("source-build") {
        anyhow::bail!(
            "provider capsule metadata '{name}' requires a source build and cannot be setup-managed"
        );
    }
    if platform_info.strategy.as_deref() == Some("local-copy") {
        anyhow::bail!(
            "provider capsule metadata '{name}' uses local-copy and cannot be setup-managed"
        );
    }

    let resolved_url = resolve_component_download_url(platform_info).ok_or_else(|| {
        anyhow::anyhow!("provider capsule metadata '{name}' has no Carrier/content release path")
    })?;
    let dest = data_dir.join(install_path);
    println!("[install] {} capsule metadata ...", name);
    download_component(
        data_dir,
        name,
        &resolved_url,
        platform_info,
        &dest,
        ipfs_gateways,
        carrier_context,
    )
    .await?;
    write_platform_cache_metadata(platform_info, &dest)?;
    if let Some(reason) = installed_component_capsule_metadata_stale_reason(name, component, &dest)
    {
        anyhow::bail!("installed capsule metadata '{name}' failed validation: {reason}");
    }
    Ok(())
}

fn validate_capsule_package_identity(
    name: &str,
    entry: Option<&CapsuleEntry>,
    platform_info: &PlatformInfo,
) -> anyhow::Result<()> {
    let registry_cid = entry.map(|entry| entry.cid.trim()).unwrap_or_default();
    let platform_cid = platform_info
        .cid
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    let cid = if platform_cid.is_empty() {
        registry_cid
    } else {
        platform_cid
    };
    if cid.is_empty() {
        anyhow::bail!("capsule component '{name}' is missing package CID");
    }
    if cid::Cid::try_from(cid).is_err() {
        anyhow::bail!("capsule component '{name}' has invalid package CID");
    }
    if !registry_cid.is_empty() && !platform_cid.is_empty() && platform_cid != registry_cid {
        anyhow::bail!(
            "capsule component '{name}' platform package CID does not match registry CID"
        );
    }
    let platform_checksum = platform_info
        .checksum
        .as_deref()
        .map(str::trim)
        .filter(|checksum| !checksum.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("capsule component '{name}' is missing platform archive checksum")
        })?;
    let Some(platform_sha256) = platform_checksum.strip_prefix("sha256:") else {
        anyhow::bail!("capsule component '{name}' platform archive checksum must be sha256");
    };
    if let Some(entry) = entry {
        if entry.sha256.trim().is_empty() {
            anyhow::bail!("capsule component '{name}' is missing package archive sha256");
        }
        let registry_sha256 = entry
            .sha256
            .trim()
            .strip_prefix("sha256:")
            .unwrap_or_else(|| entry.sha256.trim());
        if registry_sha256 != platform_sha256 {
            anyhow::bail!(
                "capsule component '{name}' package sha256 does not match platform archive checksum"
            );
        }
    }
    Ok(())
}

pub(crate) fn capsule_component_has_release_identity(data_dir: &Path, name: &str) -> bool {
    let Ok(manifest_bytes) = fs::read(data_dir.join("components.json")) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_slice::<ComponentsManifest>(&manifest_bytes) else {
        return false;
    };
    let Some(component) = manifest.external.get(name) else {
        return false;
    };
    let platform = detect_platform();
    let Some(platform_info) = resolve_platform_info(component, &platform) else {
        return false;
    };
    package_identity_is_present(manifest.capsules.get(name), platform_info)
}

fn package_identity_is_present(entry: Option<&CapsuleEntry>, platform_info: &PlatformInfo) -> bool {
    let cid_present = platform_info
        .cid
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
        || entry
            .map(|entry| !entry.cid.trim().is_empty())
            .unwrap_or(false);
    let checksum_present = platform_info
        .checksum
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
        || entry
            .map(|entry| !entry.sha256.trim().is_empty())
            .unwrap_or(false);
    cid_present && checksum_present
}

fn validate_capsule_component_install_path(name: &str, install_path: &str) -> anyhow::Result<()> {
    let path = Path::new(install_path);
    if path.is_absolute() {
        anyhow::bail!("capsule component '{name}' install_path must be relative");
    }
    if path.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    }) {
        anyhow::bail!("capsule component '{name}' install_path must not escape data_dir");
    }
    let mut components = path.components();
    if components.next() != Some(std::path::Component::Normal("capsules".as_ref())) {
        anyhow::bail!("capsule component '{name}' install_path must be under capsules/");
    }
    if path.file_name().and_then(|value| value.to_str()) != Some(name) {
        anyhow::bail!("capsule component '{name}' install_path must end with the capsule name");
    }
    Ok(())
}

fn effective_component_install_state_for_name(
    manifest: &ComponentsManifest,
    data_dir: &Path,
    name: &str,
    component: &Component,
    platform_info: Option<&PlatformInfo>,
    platform: &str,
) -> InstallState {
    let state =
        component_install_state_for_name(manifest, data_dir, name, component, platform_info);
    if !matches!(state, InstallState::Installed) {
        return state;
    }
    match capsule_metadata_install_state_for_name(data_dir, name, component, platform) {
        Some(InstallState::Installed) | None => InstallState::Installed,
        Some(InstallState::Missing) => InstallState::Stale(
            "provider capsule metadata missing from installed bundle".to_string(),
        ),
        Some(InstallState::Stale(reason)) => InstallState::Stale(reason),
    }
}

fn component_install_state_for_name(
    manifest: &ComponentsManifest,
    data_dir: &Path,
    name: &str,
    component: &Component,
    platform_info: Option<&PlatformInfo>,
) -> InstallState {
    if name == browser_vm_image::NAME {
        if !data_dir.join(browser_vm_image::INSTALL_PATH).exists() {
            return InstallState::Missing;
        }
        return match browser_vm_image::component_info(manifest, &detect_platform())
            .and_then(|info| browser_vm_image::verify_installed(data_dir, info, &detect_platform()))
        {
            Ok(()) => InstallState::Installed,
            Err(err) => InstallState::Stale(format!("Browser image set requires repair: {err}")),
        };
    }
    #[cfg(unix)]
    if name == "llama-server"
        && platform_info
            .and_then(|info| info.binary_path.as_ref())
            .is_some()
    {
        let Some(path) = resolve_install_path(component, platform_info) else {
            return InstallState::Missing;
        };
        let bundle = data_dir.join(path);
        if !bundle.exists() {
            return InstallState::Missing;
        }
        let result = local_model_engine_receipt_args(component, platform_info.unwrap()).and_then(
            |(version, checksum, binary)| {
                local_model_engine_receipt::verify(
                    &bundle,
                    version,
                    &detect_platform(),
                    checksum,
                    binary,
                )
            },
        );
        return match result {
            Ok(()) => InstallState::Installed,
            Err(err) => {
                InstallState::Stale(format!("llama-server bundle verification failed: {err}"))
            }
        };
    }

    let base = component_install_state(data_dir, component, platform_info);
    if !matches!(base, InstallState::Installed) {
        return base;
    }

    let Some(path) = resolve_install_path(component, platform_info) else {
        return base;
    };
    let install_root = data_dir.join(path);
    if !install_root.is_dir() {
        return base;
    }
    // Ordinary tool bundles retain archive identity without capsule metadata.
    if !manifest.capsules.contains_key(name) && !Path::new(path).starts_with("capsules") {
        return base;
    }
    if !install_root.join("capsule.json").is_file() {
        return InstallState::Stale("capsule metadata missing from installed bundle".to_string());
    }
    if let Some(reason) = installed_capsule_bundle_stale_reason(name, &install_root) {
        return InstallState::Stale(reason);
    }

    let Some(entry) = manifest.capsules.get(name) else {
        return base;
    };

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

fn capsule_metadata_install_state_for_name(
    data_dir: &Path,
    name: &str,
    component: &Component,
    platform: &str,
) -> Option<InstallState> {
    let metadata = component.capsule_metadata.as_ref()?;
    let Some(platform_info) = resolve_component_capsule_metadata_platform_info(metadata, platform)
    else {
        return Some(InstallState::Stale(format!(
            "provider capsule metadata is not available for {platform}"
        )));
    };
    let install_path =
        resolve_component_capsule_metadata_install_path(metadata, Some(platform_info))
            .unwrap_or_default();
    if install_path.is_empty() {
        return Some(InstallState::Stale(
            "provider capsule metadata install_path is missing".to_string(),
        ));
    }
    if let Err(err) = validate_capsule_component_install_path(name, install_path) {
        return Some(InstallState::Stale(format!(
            "provider capsule metadata path is invalid: {err}"
        )));
    }

    let install_root = data_dir.join(install_path);
    if !install_root.exists() {
        return Some(InstallState::Missing);
    }
    if !install_root.is_dir() {
        return Some(InstallState::Stale(
            "provider capsule metadata install path is not a directory".to_string(),
        ));
    }
    if let Some(reason) =
        extracted_bundle_cache_stale_reason(&install_root, platform_info).or_else(|| {
            installed_component_capsule_metadata_stale_reason(name, component, &install_root)
        })
    {
        return Some(InstallState::Stale(reason));
    }
    Some(InstallState::Installed)
}

fn installed_capsule_bundle_stale_reason(name: &str, install_root: &Path) -> Option<String> {
    let manifest_bytes = fs::read(install_root.join("capsule.json")).ok()?;
    let manifest: elastos_common::CapsuleManifest = match serde_json::from_slice(&manifest_bytes) {
        Ok(manifest) => manifest,
        Err(err) => return Some(format!("capsule metadata is invalid: {err}")),
    };
    if let Err(err) = manifest.validate() {
        return Some(format!("capsule metadata failed validation: {err}"));
    }
    if manifest.name != name {
        return Some(format!(
            "capsule metadata name mismatch: expected '{}', found '{}'",
            name, manifest.name
        ));
    }
    if matches!(
        manifest.capsule_type,
        elastos_common::CapsuleType::Wasm | elastos_common::CapsuleType::Data
    ) && !install_root.join(&manifest.entrypoint).is_file()
    {
        return Some(format!(
            "capsule entrypoint missing from installed bundle: {}",
            manifest.entrypoint
        ));
    }
    None
}

fn installed_component_capsule_metadata_stale_reason(
    name: &str,
    component: &Component,
    install_root: &Path,
) -> Option<String> {
    let manifest_bytes = match fs::read(install_root.join("capsule.json")) {
        Ok(bytes) => bytes,
        Err(err) => return Some(format!("capsule metadata is unreadable: {err}")),
    };
    let manifest: elastos_common::CapsuleManifest = match serde_json::from_slice(&manifest_bytes) {
        Ok(manifest) => manifest,
        Err(err) => return Some(format!("capsule metadata is invalid: {err}")),
    };
    if let Err(err) = manifest.validate() {
        return Some(format!("capsule metadata failed validation: {err}"));
    }
    if manifest.name != name {
        return Some(format!(
            "capsule metadata name mismatch: expected '{}', found '{}'",
            name, manifest.name
        ));
    }
    if component.provider_runtime.is_none() {
        // Content metadata describes bytes, not a provider or an execution grant.
        if manifest.role != elastos_common::CapsuleRole::Content
            || manifest.capsule_type != elastos_common::CapsuleType::Data
            || manifest.runtime_abi.is_some()
            || manifest.execution.is_some()
            || manifest.bus_contract.is_some()
            || manifest.wit_world_sha256.is_some()
            || manifest.viewer.is_some()
            || manifest.microvm.is_some()
            || manifest.providers.is_some()
            || manifest.authority.is_some()
            || manifest.provides.is_some()
            || !manifest.requires.is_empty()
            || !manifest.capabilities.is_empty()
            || !manifest.interfaces.is_empty()
            || manifest.permissions.host_process
            || manifest.permissions.guest_network
            || !manifest.permissions.storage.is_empty()
            || !manifest.permissions.messaging.is_empty()
        {
            return Some("capsule metadata must be passive content/data".to_string());
        }
        if let Err(err) = elastos_common::validate_model_content_path(&manifest.entrypoint) {
            return Some(format!("passive capsule entrypoint is invalid: {err}"));
        }
        let mut entrypoint = install_root.to_path_buf();
        for part in Path::new(&manifest.entrypoint).components() {
            entrypoint.push(part);
            match fs::symlink_metadata(&entrypoint) {
                Ok(metadata) if !metadata.file_type().is_symlink() => {}
                _ => return Some("passive capsule entrypoint is missing or aliased".to_string()),
            }
        }
        if !entrypoint.is_file() {
            return Some("passive capsule entrypoint is not a regular file".to_string());
        }
        return None;
    }
    if manifest.role != elastos_common::CapsuleRole::Provider {
        return Some("capsule metadata role must be provider".to_string());
    }
    let Some(icon_dir) = manifest
        .icon
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Some("provider capsule metadata icon path is missing".to_string());
    };
    if Path::new(icon_dir).components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    }) {
        return Some("provider capsule metadata icon path escapes the capsule".to_string());
    }
    for size in PROVIDER_ICON_SIZES {
        let icon_path = install_root.join(icon_dir).join(format!("icon-{size}.png"));
        if !icon_path.is_file() {
            return Some(format!(
                "provider capsule metadata icon asset is missing: {}",
                icon_path
                    .strip_prefix(install_root)
                    .unwrap_or(&icon_path)
                    .display()
            ));
        }
    }
    None
}

fn write_cache_metadata(
    manifest: &ComponentsManifest,
    platform_info: Option<&PlatformInfo>,
    platform: &str,
    name: &str,
    dest: &Path,
) -> anyhow::Result<()> {
    if !dest.is_dir() {
        return Ok(());
    }

    #[cfg(unix)]
    if name == "llama-server"
        && platform_info
            .and_then(|info| info.binary_path.as_ref())
            .is_some()
    {
        let component = manifest.external.get(name).ok_or_else(|| {
            anyhow::anyhow!("llama-server is missing from the component manifest")
        })?;
        let (version, checksum, binary) =
            local_model_engine_receipt_args(component, platform_info.unwrap())?;
        if platform == "linux-arm64" && platform_info.unwrap().release_path.is_some() {
            probe_arm64_model_engine(&dest.join(binary))?;
        }
        local_model_engine_receipt::write(dest, version, platform, checksum, binary)?;
        let install_path = resolve_install_path(component, platform_info)
            .ok_or_else(|| anyhow::anyhow!("local model engine install path is unavailable"))?;
        protect_local_model_engine_install_parents(dest, install_path)?;
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
    write_platform_cache_metadata(platform_info, dest)
}

// Linux AArch64 HWCAP: FP16 scalar, FP16 SIMD and dot product.
#[cfg(any(test, all(target_os = "linux", target_arch = "aarch64")))]
const ARM64_MODEL_HWCAP: u64 = (1 << 9) | (1 << 10) | (1 << 20);

#[cfg(any(test, all(target_os = "linux", target_arch = "aarch64")))]
fn arm64_model_cpu_features_available(hwcap: u64) -> bool {
    hwcap & ARM64_MODEL_HWCAP == ARM64_MODEL_HWCAP
}

#[cfg(any(test, all(target_os = "linux", target_arch = "aarch64")))]
fn arm64_model_elf_compatible(header: &[u8]) -> bool {
    header.len() >= 20
        && header.starts_with(b"\x7fELF\x02\x01\x01")
        && matches!(u16::from_le_bytes([header[16], header[17]]), 2 | 3)
        && u16::from_le_bytes([header[18], header[19]]) == 183
}

pub(crate) fn verify_arm64_model_host(platform: &str) -> anyhow::Result<()> {
    if platform != "linux-arm64" {
        return Ok(());
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        let available = unsafe { libc::getauxval(libc::AT_HWCAP) } as u64;
        anyhow::ensure!(
            arm64_model_cpu_features_available(available),
            "ARM64 llama-server requires dot product and FP16 CPU features"
        );
    }
    Ok(())
}

fn probe_arm64_model_engine(path: &Path) -> anyhow::Result<()> {
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        use std::process::Stdio;
        use std::time::{Duration, Instant};

        verify_arm64_model_host("linux-arm64")?;
        let check_elf = |file: &Path| -> anyhow::Result<()> {
            let mut header = [0_u8; 20];
            fs::File::open(file)?.read_exact(&mut header)?;
            anyhow::ensure!(
                arm64_model_elf_compatible(&header),
                "ARM64 model engine bundle contains an incompatible ELF: {}",
                file.display()
            );
            Ok(())
        };
        check_elf(path)?;
        let mut libraries = 0;
        for entry in fs::read_dir(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("engine path has no parent"))?,
        )? {
            let entry = entry?;
            if !entry.file_name().to_string_lossy().contains(".so") || !entry.file_type()?.is_file()
            {
                continue;
            }
            check_elf(&entry.path())?;
            libraries += 1;
        }
        anyhow::ensure!(libraries > 0, "ARM64 model engine libraries are missing");
        let mut command = Command::new(path);
        command
            .arg("--version")
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = crate::install_transaction::retry_text_file_busy(|| command.spawn())
            .map_err(|error| anyhow::anyhow!("ARM64 model engine cannot start: {error}"))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait()? {
                anyhow::ensure!(
                    status.success(),
                    "ARM64 model engine or host libraries are incompatible"
                );
                return Ok(());
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("ARM64 model engine compatibility probe timed out");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    #[cfg(not(all(target_os = "linux", target_arch = "aarch64")))]
    {
        let _ = path;
        Ok(())
    }
}

fn local_model_engine_receipt_args<'a>(
    component: &'a Component,
    platform_info: &'a PlatformInfo,
) -> anyhow::Result<(&'a str, &'a str, &'a str)> {
    Ok((
        component
            .version
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("llama-server bundle version is missing"))?,
        platform_info
            .checksum
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("llama-server bundle checksum is missing"))?,
        platform_info
            .binary_path
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("llama-server bundle binary path is missing"))?,
    ))
}

/// A read-only confinement boundary, separate from engine verification and
/// offer admission. It can precede installation of the exact pinned bundle.
#[cfg(unix)]
pub(crate) fn local_model_engine_confinement_bundle(
    data_dir: &Path,
    manifest: &ComponentsManifest,
) -> anyhow::Result<Option<PathBuf>> {
    let Some(component) = manifest.external.get("llama-server") else {
        return Ok(None);
    };
    let platform = detect_platform();
    let Some(info) = component.platforms.get(&platform) else {
        return Ok(None);
    };
    let (version, checksum, _) = local_model_engine_receipt_args(component, info)?;
    anyhow::ensure!(
        Path::new(version).components().count() == 1
            && Path::new(version)
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
            && checksum.strip_prefix("sha256:").is_some_and(
                |value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
            ),
        "local model engine pin is invalid"
    );
    let expected = format!("libexec/llama.cpp/{version}/{platform}");
    anyhow::ensure!(
        resolve_install_path(component, Some(info)) == Some(expected.as_str()),
        "local model engine confinement path is invalid"
    );
    Ok(Some(data_dir.canonicalize()?.join(expected)))
}

#[cfg(unix)]
#[derive(PartialEq)]
pub(crate) struct LocalModelEngineIdentity {
    pub path: PathBuf,
    pub sha256: String,
    pub receipt_sha256: String,
}

/// Clear group/other write on install_path directories under the data dir.
/// Call this from the owned install/repair path only. Verification stays read-only.
#[cfg(unix)]
fn protect_local_model_engine_install_parents(
    bundle: &Path,
    install_path: &str,
) -> anyhow::Result<()> {
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
    use std::os::unix::io::AsRawFd;
    let relative = Path::new(install_path);
    anyhow::ensure!(
        !install_path.is_empty()
            && install_path.len() <= 4096
            && relative
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_))),
        "local model engine install path is invalid"
    );
    let mut data_dir = if bundle.is_absolute() {
        bundle.to_path_buf()
    } else {
        std::env::current_dir()?.join(bundle)
    };
    anyhow::ensure!(
        data_dir.ends_with(relative),
        "model engine parent is not protected"
    );
    for _ in relative.components() {
        data_dir = data_dir
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow::anyhow!("model engine parent is not protected"))?;
    }
    let data_dir = fs::canonicalize(data_dir)?;
    let mut path = data_dir.clone();
    for part in relative.components() {
        path.push(part.as_os_str());
        anyhow::ensure!(
            path.starts_with(&data_dir),
            "model engine parent is not protected"
        );
        let dir = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&path)
            .map_err(|_| anyhow::anyhow!("model engine parent is not protected"))?;
        let metadata = dir.metadata()?;
        anyhow::ensure!(
            metadata.is_dir() && metadata.uid() == unsafe { libc::geteuid() },
            "model engine parent is not protected"
        );
        let mode = metadata.mode();
        if mode & 0o022 != 0 {
            let rc =
                unsafe { libc::fchmod(dir.as_raw_fd(), (mode as libc::mode_t & 0o7777) & !0o022) };
            anyhow::ensure!(rc == 0, "model engine parent is not protected");
        }
        let after = dir.metadata()?;
        anyhow::ensure!(
            after.is_dir()
                && after.uid() == unsafe { libc::geteuid() }
                && after.mode() & 0o022 == 0,
            "model engine parent is not protected"
        );
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn local_model_engine_receipt_identity(
    data_dir: &Path,
    manifest: &ComponentsManifest,
) -> anyhow::Result<LocalModelEngineIdentity> {
    use std::os::unix::fs::MetadataExt as _;
    let component = manifest
        .external
        .get("llama-server")
        .ok_or_else(|| anyhow::anyhow!("local model engine is unavailable"))?;
    let platform = detect_platform();
    let info = component
        .platforms
        .get(&platform)
        .ok_or_else(|| anyhow::anyhow!("local model engine platform is unavailable"))?;
    let install_path = resolve_install_path(component, Some(info))
        .ok_or_else(|| anyhow::anyhow!("local model engine install path is unavailable"))?;
    let relative = Path::new(install_path);
    anyhow::ensure!(
        !install_path.is_empty()
            && install_path.len() <= 4096
            && relative
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_))),
        "local model engine install path is invalid"
    );
    let mut bundle = data_dir.canonicalize()?;
    for part in relative.components() {
        bundle.push(part.as_os_str());
        let metadata = fs::symlink_metadata(&bundle)?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink()
                && metadata.is_dir()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o022 == 0,
            "model engine parent is not protected"
        );
    }
    anyhow::ensure!(
        bundle.canonicalize()? == bundle,
        "local model engine bundle is aliased"
    );
    let (version, archive, binary) = local_model_engine_receipt_args(component, info)?;
    let (sha256, receipt_sha256) =
        local_model_engine_receipt::identity(&bundle, version, &platform, archive, binary)?;
    let path = bundle.join(binary);
    anyhow::ensure!(
        path.canonicalize()? == path,
        "local model engine executable is aliased"
    );
    Ok(LocalModelEngineIdentity {
        path,
        sha256,
        receipt_sha256,
    })
}

/// Acquire only a missing engine. A present, invalid bundle requires repair;
/// it must never be silently replaced or admitted for execution.
#[cfg(unix)]
pub(crate) async fn ensure_local_model_engine(
    data_dir: &Path,
    manifest_bytes: &[u8],
) -> anyhow::Result<LocalModelEngineIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    let manifest: ComponentsManifest = serde_json::from_slice(manifest_bytes)?;
    let component = manifest
        .external
        .get("llama-server")
        .ok_or_else(|| anyhow::anyhow!("local model engine is unavailable"))?;
    let platform = detect_platform();
    let info = component
        .platforms
        .get(&platform)
        .ok_or_else(|| anyhow::anyhow!("local model engine platform is unavailable"))?;
    let install_path = resolve_install_path(component, Some(info))
        .ok_or_else(|| anyhow::anyhow!("local model engine install path is unavailable"))?;
    // An installed engine is verified against its receipt and used without
    // the installation writer, so a running update does not block model offers.
    match fs::symlink_metadata(data_dir.join(install_path)) {
        Ok(_) => return verified_local_model_engine(data_dir, &manifest),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let writer = crate::install_transaction::acquire_installed_writer(data_dir)?;
    let sources = crate::sources::load_trusted_sources(data_dir)?;
    let source = sources.default_source();
    let has_consumed = match fs::symlink_metadata(data_dir.join("installation")) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    let installed = if has_consumed || source.is_some_and(|s| !s.install_path.is_empty()) {
        let source = source.ok_or_else(missing_trusted_source_error)?;
        let guard = writer.as_ref().ok_or_else(|| {
            anyhow::anyhow!("engine acquisition requires its installation writer")
        })?;
        let binary = fs::canonicalize(&source.install_path)?;
        let admitted = crate::installed_release::read_for_setup(data_dir, &binary, source, guard)?;
        let release: serde_json::Value = serde_json::from_slice(&admitted.release)?;
        crate::installed_release::admit_descriptor(
            &release["payload"]["platforms"][crate::update::detect_release_platform()]
                ["components"],
            &hex::encode(sha2::Sha256::digest(manifest_bytes)),
            manifest_bytes.len() as u64,
        )?;
        Some((source, binary, admitted))
    } else {
        None
    };
    let release_path = info
        .release_path
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "llama-server must come from the signed release; URL-only engines are refused"
            )
        })?;
    // Source-checkout Homes build the engine during setup; only signed releases
    // carry a prebuilt bundle that Assistant can fetch on demand.
    anyhow::ensure!(
        !matches!(info.strategy.as_deref(), Some("source-build") | Some("local-copy")),
        "This source-checkout Home has no local AI engine. Rerun scripts/setup-source-home.sh with SETUP_SOURCE_HOME_INSTALL_LLAMA_SERVER=1 to build it."
    );
    anyhow::ensure!(
        matches!(info.strategy.as_deref(), None | Some("prebuilt")) && info.extract_path.is_some(),
        "on-demand llama-server requires a signed prebuilt bundle"
    );
    let (source, binary, admitted) = installed.ok_or_else(|| {
        anyhow::anyhow!("engine acquisition requires an installed signed release")
    })?;
    let guard = writer.as_ref().unwrap();
    let bundle = local_model_engine_confinement_bundle(data_dir, &manifest)?
        .ok_or_else(|| anyhow::anyhow!("local model engine is unavailable"))?;
    verify_arm64_model_host(&platform)?;
    let bytes = fetch_first_party_component_via_carrier(
        data_dir,
        release_path,
        FirstPartyCarrierContext::Runtime,
    )
    .await?;
    verify_checksum("llama-server", &bytes, info)?;
    if let Some(size) = info.size {
        anyhow::ensure!(
            bytes.len() as u64 == size,
            "llama-server bundle size differs"
        );
    }
    let current = crate::installed_release::read_for_setup(data_dir, &binary, source, guard)?;
    anyhow::ensure!(
        current.head == admitted.head
            && current.release == admitted.release
            && crate::api::capsule_inventory::read_model_catalog_file(
                data_dir,
                "components.json",
                4 * 1024 * 1024
            )? == manifest_bytes,
        "installed release inputs changed during engine acquisition"
    );
    // Check each parent before the archive installer can create descendants.
    let mut parent = data_dir.canonicalize()?;
    for part in Path::new(install_path).parent().unwrap().components() {
        parent.push(part.as_os_str());
        match fs::create_dir(&parent) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let metadata = fs::symlink_metadata(&parent)?;
        anyhow::ensure!(
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o022 == 0,
            "model engine parent is not protected"
        );
    }
    extract_from_tarball(&bytes, &bundle, info)?;
    write_cache_metadata(&manifest, Some(info), &platform, "llama-server", &bundle)?;
    verified_local_model_engine(data_dir, &manifest)
}

#[cfg(unix)]
pub(crate) fn verified_local_model_engine(
    data_dir: &Path,
    manifest: &ComponentsManifest,
) -> anyhow::Result<LocalModelEngineIdentity> {
    let identity = local_model_engine_receipt_identity(data_dir, manifest)?;
    let component = manifest
        .external
        .get("llama-server")
        .ok_or_else(|| anyhow::anyhow!("local model engine is unavailable"))?;
    let platform = detect_platform();
    let info = component
        .platforms
        .get(&platform)
        .ok_or_else(|| anyhow::anyhow!("local model engine platform is unavailable"))?;
    let (version, archive, binary) = local_model_engine_receipt_args(component, info)?;
    let install_path = resolve_install_path(component, Some(info))
        .ok_or_else(|| anyhow::anyhow!("local model engine install path is unavailable"))?;
    let relative = Path::new(install_path);
    anyhow::ensure!(
        !install_path.is_empty()
            && install_path.len() <= 4096
            && relative
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_))),
        "local model engine install path is invalid"
    );
    let bundle = data_dir.canonicalize()?.join(relative);
    anyhow::ensure!(
        identity.path == bundle.join(binary)
            && bundle.canonicalize()? == bundle
            && identity.path.canonicalize()? == identity.path,
        "local model engine install path is unavailable"
    );
    local_model_engine_receipt::verify(&bundle, version, &platform, archive, binary)?;
    verify_arm64_model_host(&platform)?;
    anyhow::ensure!(
        local_model_engine_receipt_identity(data_dir, manifest)? == identity
            && compute_sha256_checksum(&identity.path)? == identity.sha256,
        "model engine changed during verification"
    );
    Ok(identity)
}

fn extracted_bundle_cache_stale_reason(
    install_root: &Path,
    platform_info: &PlatformInfo,
) -> Option<String> {
    // Only an extracted bundle can be stale.
    platform_info.extract_path.as_ref()?;

    if let Some(expected_cid) = platform_info
        .cid
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        let cached_cid = fs::read_to_string(install_root.join(CACHED_CID_FILE))
            .ok()
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        if cached_cid != expected_cid {
            return Some("extracted bundle CID metadata missing or stale".to_string());
        }
    }
    if let Some(expected_checksum) = platform_info
        .checksum
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        let cached_sha = fs::read_to_string(install_root.join(CACHED_ARTIFACT_SHA_FILE))
            .ok()
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        // Registry capsule installs record the bare hex digest; platform checksums carry the
        // `sha256:` prefix. Both name the same verified archive.
        let bare = |value: &str| value.strip_prefix("sha256:").unwrap_or(value).to_string();
        if bare(&cached_sha) != bare(expected_checksum) {
            return Some("extracted bundle checksum metadata missing or stale".to_string());
        }
    }
    None
}

fn write_platform_cache_metadata(platform_info: &PlatformInfo, dest: &Path) -> anyhow::Result<()> {
    if !dest.is_dir() || platform_info.extract_path.is_none() {
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
                if let Some(reason) = platform_info
                    .and_then(|info| extracted_bundle_cache_stale_reason(&candidate, info))
                {
                    return InstallState::Stale(reason);
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
pub(crate) fn resolve_install_path<'a>(
    component: &'a Component,
    platform_info: Option<&'a PlatformInfo>,
) -> Option<&'a str> {
    resolve_install_path_parts(component.install_path.as_deref(), platform_info)
}

pub(crate) fn resolve_platform_info<'a>(
    component: &'a Component,
    platform: &str,
) -> Option<&'a PlatformInfo> {
    resolve_platform_info_from_map(&component.platforms, platform)
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

fn resolve_install_path_parts<'a>(
    component_install_path: Option<&'a str>,
    platform_info: Option<&'a PlatformInfo>,
) -> Option<&'a str> {
    platform_info
        .and_then(|p| p.install_path.as_deref())
        .or(component_install_path)
}

fn resolve_platform_info_from_map<'a>(
    platforms: &'a HashMap<String, PlatformInfo>,
    platform: &str,
) -> Option<&'a PlatformInfo> {
    platforms
        .get(platform)
        .or_else(|| platform_aliases(platform).find_map(|alias| platforms.get(alias)))
        .or_else(|| platforms.get("*"))
}

fn resolve_component_capsule_metadata_install_path<'a>(
    component: &'a ComponentCapsuleMetadata,
    platform_info: Option<&'a PlatformInfo>,
) -> Option<&'a str> {
    resolve_install_path_parts(component.install_path.as_deref(), platform_info)
}

fn resolve_component_capsule_metadata_platform_info<'a>(
    component: &'a ComponentCapsuleMetadata,
    platform: &str,
) -> Option<&'a PlatformInfo> {
    resolve_platform_info_from_map(&component.platforms, platform)
}

fn platform_aliases(platform: &str) -> impl Iterator<Item = &'static str> {
    let aliases: &'static [&'static str] = match platform {
        "x86_64-linux" => &["linux-amd64"],
        "aarch64-linux" => &["linux-arm64"],
        "linux-amd64" => &["x86_64-linux"],
        "linux-arm64" => &["aarch64-linux"],
        "aarch64-darwin" => &["darwin-arm64"],
        "darwin-arm64" => &["aarch64-darwin"],
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

const MODEL_CATALOG_FILE: &str = "model-catalog.json";

pub(crate) fn catalog_head_cid(bytes: &[u8]) -> anyhow::Result<String> {
    let hash = cid::multihash::Multihash::<64>::wrap(0x12, &sha2::Sha256::digest(bytes))
        .map_err(|err| anyhow::anyhow!("model catalog digest is not a SHA-256 multihash: {err}"))?;
    Ok(cid::Cid::new_v1(0x55, hash).to_string())
}

fn verify_catalog_head(head_cid: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let actual = catalog_head_cid(bytes)?;
    if actual != head_cid {
        anyhow::bail!(
            "model catalog head {actual} does not match the pinned raw SHA-256 CIDv1 {head_cid}"
        );
    }
    Ok(())
}

fn resolve_model_catalog_source(manifest_path: &Path, dest: &Path) -> anyhow::Result<PathBuf> {
    let sibling = manifest_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("components manifest has no parent directory"))?
        .join(MODEL_CATALOG_FILE);
    if sibling.is_file() {
        return Ok(sibling);
    }
    if dest.is_file() {
        return Ok(dest.to_path_buf());
    }
    anyhow::bail!(
        "model catalog pin is present but {MODEL_CATALOG_FILE} is missing beside {} and in the data directory",
        manifest_path.display()
    );
}

pub(crate) fn install_signed_model_catalog(
    data_dir: &Path,
    manifest: &ComponentsManifest,
    manifest_path: &Path,
) -> anyhow::Result<()> {
    let Some(trust) = &manifest.model_catalog else {
        return Ok(());
    };
    if trust.head_cid.is_empty() || trust.head_cid.len() > 128 {
        anyhow::bail!("model catalog trust head is invalid");
    }
    let dest = data_dir.join(MODEL_CATALOG_FILE);
    let source = resolve_model_catalog_source(manifest_path, &dest)?;
    let bytes = fs::read(&source)?;
    if bytes.len() > MAX_MODEL_CATALOG_BYTES {
        anyhow::bail!(
            "model catalog exceeds its {}-byte bound",
            MAX_MODEL_CATALOG_BYTES
        );
    }
    verify_catalog_head(&trust.head_cid, &bytes)?;
    if source != dest {
        atomic_write_file(&dest, &bytes)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o600))?;
    }
    println!("Installed signed model catalog: {}", dest.display());
    Ok(())
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
    println!("Quick start:");
    println!("  elastos setup                # default home profile");
    println!("  elastos setup --profile demo # Home + site/share/browser demo surfaces");
    println!("  elastos setup --profile blockchain # typed chain/wallet/DRM provider development");
    println!("  elastos setup --profile operator # explicit serve/node/run/agent runtime");
    println!();
    print_profile_section(
        "Recommended profiles:",
        manifest,
        &["home", "demo", "blockchain", "operator"],
    );
    print_profile_section(
        "Advanced profiles:",
        manifest,
        &["public-gateway", "agent-local-ai", "full"],
    );

    let listed = [
        "home",
        "demo",
        "blockchain",
        "operator",
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
    name
}

async fn prepare_selected_component_prerequisites(
    data_dir: &Path,
    manifest: &ComponentsManifest,
    platform: &str,
    components: &[String],
    ipfs_gateways: &[ElastosFetchPath],
    supplied_tools: Option<&Path>,
) -> anyhow::Result<()> {
    if components.iter().any(|name| name == "media-provider") {
        let managed_tools;
        let tools = if let Some(tools) = supplied_tools {
            tools
        } else {
            let name = "media-tools";
            let component = manifest.external.get(name).ok_or_else(|| {
                anyhow::anyhow!("Home media-tools component is missing; run the installer again")
            })?;
            let info = resolve_platform_info(component, platform).ok_or_else(|| {
                anyhow::anyhow!("Home media-tools are unavailable for {platform}")
            })?;
            if resolve_install_path(component, Some(info)) != Some("tools/media-tools")
                || info.extract_path.as_deref() != Some("media-tools")
                || info.strategy.is_some()
                || info.release_path.as_deref().is_none_or(str::is_empty)
            {
                anyhow::bail!("Home media-tools require a managed release archive");
            }
            required_release_artifact_checksum(name, info)?;
            #[cfg(unix)]
            crate::protected_content_runtime::ensure_media_provider_directory(
                data_dir,
                "Runtime data root",
            )?;
            #[cfg(unix)]
            crate::protected_content_runtime::ensure_media_provider_directory(
                &data_dir.join("tools"),
                "Runtime managed tools parent",
            )?;
            let dest = data_dir.join("tools/media-tools");
            if !matches!(
                component_install_state(data_dir, component, Some(info)),
                InstallState::Installed
            ) {
                let url = resolve_component_download_url(info)
                    .ok_or_else(|| anyhow::anyhow!("Home media-tools release path is missing"))?;
                download_component(
                    data_dir,
                    name,
                    &url,
                    info,
                    &dest,
                    ipfs_gateways,
                    FirstPartyCarrierContext::Setup,
                )
                .await?;
                write_cache_metadata(manifest, Some(info), platform, name, &dest)?;
            }
            managed_tools = dest.join("bin");
            &managed_tools
        };
        crate::protected_content_runtime::prepare_runtime_media_provider_prerequisite(
            data_dir, tools,
        )?;
    } else if supplied_tools.is_some() {
        anyhow::bail!("--media-tools-dir requires media-provider selection");
    }
    Ok(())
}

// ── Elastos fetch-path resolution ──────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
struct ElastosFetchPath {
    transport_base: String,
    description: String,
}

/// Build the explicit gateway list for CID-only operator entries.
/// Release-path downloads use Carrier and never fall back to these gateways.
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
    // Display-only identity string for content-addressed components.
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

fn requires_release_artifact_checksum(platform_info: &PlatformInfo) -> bool {
    if matches!(
        platform_info.strategy.as_deref(),
        Some("source-build" | "local-copy")
    ) {
        return false;
    }
    platform_info
        .release_path
        .as_deref()
        .is_some_and(|value| !value.is_empty())
        || platform_info
            .cid
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        || platform_info
            .url
            .as_deref()
            .is_some_and(|value| !value.is_empty())
}

fn required_release_artifact_checksum<'a>(
    name: &str,
    platform_info: &'a PlatformInfo,
) -> anyhow::Result<Option<&'a str>> {
    let checksum = platform_info
        .checksum
        .as_deref()
        .filter(|checksum| !checksum.is_empty());
    if !requires_release_artifact_checksum(platform_info) {
        return Ok(checksum);
    }
    valid_release_artifact_checksum(name, checksum).map(Some)
}

fn valid_release_artifact_checksum<'a>(
    name: &str,
    checksum: Option<&'a str>,
) -> anyhow::Result<&'a str> {
    let checksum = checksum.filter(|value| !value.is_empty()).ok_or_else(|| {
        anyhow::anyhow!(
            "component '{}' release artifact is missing checksum; expected sha256:... or sha512:...",
            name
        )
    })?;
    if checksum.split_once(':').is_some_and(|(algorithm, hash)| {
        let length = match algorithm {
            "sha256" => 64,
            "sha512" => 128,
            _ => return false,
        };
        hash.len() == length && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        Ok(checksum)
    } else {
        anyhow::bail!(
            "Unknown checksum format for {}: {}. Expected sha256:... or sha512:...",
            name,
            checksum
        );
    }
}

/// Admit all applicable release components before downloads or installation.
/// Setup retains its explicit development strategies outside release admission.
pub(crate) fn admit_release_components(
    manifest: &ComponentsManifest,
    platform: &str,
) -> anyhow::Result<()> {
    for (name, component) in &manifest.external {
        let assets = [
            resolve_platform_info(component, platform),
            component.capsule_metadata.as_ref().and_then(|metadata| {
                resolve_component_capsule_metadata_platform_info(metadata, platform)
            }),
        ];
        for info in assets.into_iter().flatten() {
            anyhow::ensure!(
                matches!(info.strategy.as_deref(), None | Some("prebuilt")),
                "component '{name}' has a development or unsupported release strategy"
            );
            valid_release_artifact_checksum(name, info.checksum.as_deref())?;
        }
    }
    Ok(())
}

/// Refresh support assets after the updater installs the verified manifest bytes.
/// The publisher owns that manifest; setup's local stamping stays separate.
pub async fn refresh_installed_components_for_update(
    data_dir: &Path,
    old_components: Option<&[u8]>,
    new_components: &[u8],
    platform: &str,
) -> anyhow::Result<Vec<String>> {
    refresh_installed_components_for_update_in_context(
        data_dir,
        old_components,
        new_components,
        platform,
        FirstPartyCarrierContext::Setup,
    )
    .await
}

pub(crate) async fn refresh_installed_components_for_update_in_context(
    data_dir: &Path,
    old_components: Option<&[u8]>,
    new_components: &[u8],
    platform: &str,
    carrier_context: FirstPartyCarrierContext,
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
            set_local_copy_permissions(&source, &dest);
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
            carrier_context,
        )
        .await?;
        write_cache_metadata(
            &new_manifest,
            Some(new_platform_info),
            platform,
            name,
            &dest,
        )?;
        refreshed.push(name.clone());
    }

    for (name, new_component) in &new_manifest.external {
        let old_component = old_manifest.external.get(name);
        if !component_capsule_metadata_changed(old_component, new_component, platform)
            || new_component.capsule_metadata.is_none()
            || !matches!(
                component_install_state_for_name(
                    &new_manifest,
                    data_dir,
                    name,
                    new_component,
                    resolve_platform_info(new_component, platform)
                ),
                InstallState::Installed
            )
        {
            continue;
        }
        ensure_component_capsule_metadata(
            data_dir,
            name,
            new_component,
            platform,
            &gateways,
            carrier_context,
        )
        .await?;
        refreshed.push(name.clone());
    }

    refreshed.sort();
    refreshed.dedup();
    Ok(refreshed)
}

/// Prepare installed support in an isolated tree. The release transaction owns
/// publication and rollback; setup owns archive validation and cache metadata.
pub(crate) async fn stage_update_support(
    data_dir: &Path,
    old_bytes: &[u8],
    new_bytes: &[u8],
    platform: &str,
    fetch: &crate::update::FetchFn,
    owner: &mut dyn crate::update::RestartOwner,
) -> anyhow::Result<(tempfile::TempDir, Vec<(PathBuf, PathBuf)>)> {
    for bytes in [old_bytes, new_bytes] {
        let value: serde_json::Value = serde_json::from_slice(bytes)?;
        anyhow::ensure!(
            value["schema"] == "elastos.components/v1",
            "unsupported release components schema"
        );
    }
    let old: ComponentsManifest = serde_json::from_slice(old_bytes)?;
    let new: ComponentsManifest = serde_json::from_slice(new_bytes)?;
    let stage = tempfile::tempdir_in(data_dir)?;
    let mut paths = std::collections::BTreeMap::new();
    for (name, component) in &new.external {
        let Some(info) = resolve_platform_info(component, platform) else {
            continue;
        };
        let old_component = old.external.get(name);
        let required = new
            .profiles
            .get("home")
            .is_some_and(|profile| profile.components.contains(name));
        let installed = !matches!(
            component_install_state_for_name(
                &old,
                data_dir,
                name,
                old_component.unwrap_or(component),
                old_component.and_then(|old| resolve_platform_info(old, platform))
            ),
            InstallState::Missing
        );
        if !installed && !required {
            continue;
        }
        let changed = component_signature(old_component, platform)
            != component_signature(Some(component), platform);
        let mut assets = Vec::new();
        if changed || !installed {
            let path = resolve_install_path(component, Some(info))
                .ok_or_else(|| anyhow::anyhow!("support install path missing"))?;
            if let Some(old_path) = old_component
                .and_then(|old| resolve_install_path(old, resolve_platform_info(old, platform)))
            {
                if path != old_path {
                    paths.insert(PathBuf::from(old_path), PathBuf::new());
                }
            }
            assets.push((PathBuf::from(path), info, false));
        }
        if component_capsule_metadata_changed(old_component, component, platform)
            || changed
            || !installed
        {
            if let Some(metadata) = &component.capsule_metadata {
                let info = resolve_component_capsule_metadata_platform_info(metadata, platform)
                    .ok_or_else(|| anyhow::anyhow!("support metadata unavailable"))?;
                let path = resolve_component_capsule_metadata_install_path(metadata, Some(info))
                    .ok_or_else(|| anyhow::anyhow!("support metadata path missing"))?;
                validate_capsule_component_install_path(name, path)?;
                assets.push((PathBuf::from(path), info, true));
            }
        }
        for (relative, asset, metadata) in assets {
            crate::install_transaction::validate_support_path(&relative)?;
            required_release_artifact_checksum(name, asset)?
                .ok_or_else(|| anyhow::anyhow!("signed support checksum missing"))?;
            let key =
                if let Some(path) = asset.release_path.as_ref().filter(|path| !path.is_empty()) {
                    anyhow::ensure!(
                        !Path::new(path).is_absolute()
                            && Path::new(path)
                                .components()
                                .all(|part| matches!(part, std::path::Component::Normal(_))),
                        "unsafe signed support release path"
                    );
                    format!("release-path:{path}")
                } else {
                    let cid = asset
                        .cid
                        .as_ref()
                        .filter(|cid| !cid.is_empty())
                        .ok_or_else(|| anyhow::anyhow!("signed support source missing"))?;
                    cid::Cid::try_from(cid.as_str())?;
                    cid.clone()
                };
            owner.progress("downloading", &crate::update::download_message(asset.size))?;
            let bytes = fetch(key, Vec::new()).await?;
            owner.progress(
                "verifying",
                "Verifying the update. Home restarts when the update is ready.",
            )?;
            verify_checksum(name, &bytes, asset)?;
            if let Some(size) = asset.size {
                anyhow::ensure!(bytes.len() as u64 == size, "support artifact size mismatch");
            }
            let dest = stage.path().join(&relative);
            if asset.extract_path.is_some() {
                extract_from_tarball(&bytes, &dest, asset)?;
            } else {
                atomic_write_file(&dest, &bytes)?;
                fs::set_permissions(&dest, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
            }
            if metadata {
                write_platform_cache_metadata(asset, &dest)?;
                anyhow::ensure!(
                    installed_component_capsule_metadata_stale_reason(name, component, &dest)
                        .is_none(),
                    "support capsule metadata verification failed"
                );
            } else {
                write_cache_metadata(&new, Some(asset), platform, name, &dest)?;
            }
            paths.insert(relative, dest.clone());
            if !metadata && asset.binary_path.is_some() {
                // Bundle aliases point to the final owned installation, not staging.
                let relative = PathBuf::from("bin").join(name);
                crate::install_transaction::validate_support_path(&relative)?;
                let link = stage.path().join(&relative);
                fs::create_dir_all(link.parent().unwrap())?;
                let binary = Path::new(asset.binary_path.as_deref().unwrap());
                anyhow::ensure!(
                    !binary.is_absolute()
                        && binary
                            .components()
                            .all(|part| matches!(part, std::path::Component::Normal(_))),
                    "unsafe support executable path"
                );
                anyhow::ensure!(dest.join(binary).is_file(), "support executable missing");
                std::os::unix::fs::symlink(
                    data_dir
                        .join(
                            asset
                                .install_path
                                .as_deref()
                                .ok_or_else(|| anyhow::anyhow!("bundle install path missing"))?,
                        )
                        .join(binary),
                    &link,
                )?;
                paths.insert(relative, link);
            }
        }
    }
    for (name, component) in &old.external {
        if !new.external.contains_key(name) {
            if let Some(path) =
                resolve_install_path(component, resolve_platform_info(component, platform))
            {
                paths.insert(PathBuf::from(path), PathBuf::new());
            }
        }
    }
    for name in old.capsules.keys().chain(new.capsules.keys()) {
        let relative = PathBuf::from("capsules").join(name);
        crate::install_transaction::validate_support_path(&relative)?;
        let changed = match (old.capsules.get(name), new.capsules.get(name)) {
            (Some(old), Some(new)) => old.cid != new.cid || old.sha256 != new.sha256,
            _ => true,
        };
        if !changed
            || paths
                .keys()
                .any(|path| path.starts_with(&relative) || relative.starts_with(path))
        {
            continue;
        }
        let Some(entry) = new.capsules.get(name) else {
            paths.insert(relative, PathBuf::new());
            continue;
        };
        // Uninstalled optional apps remain on demand. A cached app must be ready
        // before activation, with the same archive and cache contract as supervisor.
        if !data_dir.join(&relative).exists() {
            continue;
        }
        cid::Cid::try_from(entry.cid.as_str())?;
        anyhow::ensure!(!entry.sha256.is_empty(), "signed capsule checksum missing");
        owner.progress(
            "downloading",
            &crate::update::download_message(Some(entry.size)),
        )?;
        let bytes = fetch(entry.cid.clone(), Vec::new()).await?;
        owner.progress(
            "verifying",
            "Verifying the update. Home restarts when the update is ready.",
        )?;
        anyhow::ensure!(
            hex::encode(sha2::Sha256::digest(&bytes)) == entry.sha256
                && bytes.len() as u64 == entry.size,
            "capsule artifact verification failed"
        );
        let dest = stage.path().join(&relative);
        fs::create_dir_all(&dest)?;
        tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice())).unpack(&dest)?;
        anyhow::ensure!(
            dest.join("capsule.json").is_file(),
            "capsule manifest missing"
        );
        fs::write(dest.join(CACHED_CID_FILE), format!("{}\n", entry.cid))?;
        fs::write(
            dest.join(CACHED_ARTIFACT_SHA_FILE),
            format!("{}\n", entry.sha256),
        )?;
        paths.insert(relative, dest);
    }
    // Journal the first absent parent as one tree. Recovery then removes a new
    // bundle's directories as well as its files, preserving the old support set.
    let mut complete = std::collections::BTreeMap::new();
    for (relative, source) in &paths {
        let mut root = relative.clone();
        if !source.as_os_str().is_empty() {
            while !data_dir.join(root.parent().unwrap()).exists() {
                root = root.parent().unwrap().to_path_buf();
                crate::install_transaction::validate_support_path(&root)?;
            }
        }
        let source = if root == *relative {
            source.clone()
        } else {
            stage.path().join(&root)
        };
        complete.insert(root, source);
    }
    let roots: Vec<_> = complete.keys().cloned().collect();
    complete.retain(|path, _| {
        !roots
            .iter()
            .any(|root| root != path && path.starts_with(root))
    });
    Ok((stage, complete.into_iter().collect()))
}

fn component_signature(component: Option<&Component>, platform: &str) -> Option<String> {
    let component = component?;
    let platform_info = resolve_platform_info(component, platform)?;
    Some(format!(
        "version={:?}|component_install={:?}|platform_install={:?}|url={:?}|cid={:?}|release_path={:?}|checksum={:?}|extract={:?}|binary={:?}|strategy={:?}|source={:?}",
        component.version,
        component.install_path,
        platform_info.install_path,
        platform_info.url,
        platform_info.cid,
        platform_info.release_path,
        platform_info.checksum,
        platform_info.extract_path,
        platform_info.binary_path,
        platform_info.strategy,
        platform_info.source
    ))
}

fn component_capsule_metadata_changed(
    old_component: Option<&Component>,
    new_component: &Component,
    platform: &str,
) -> bool {
    component_capsule_metadata_signature(old_component, platform)
        != component_capsule_metadata_signature(Some(new_component), platform)
}

fn component_capsule_metadata_signature(
    component: Option<&Component>,
    platform: &str,
) -> Option<String> {
    let metadata = component?.capsule_metadata.as_ref()?;
    let platform_info = resolve_component_capsule_metadata_platform_info(metadata, platform)?;
    Some(format!(
        "component_install={:?}|platform_install={:?}|url={:?}|cid={:?}|release_path={:?}|checksum={:?}|extract={:?}|binary={:?}|strategy={:?}|source={:?}|size={:?}",
        metadata.install_path,
        platform_info.install_path,
        platform_info.url,
        platform_info.cid,
        platform_info.release_path,
        platform_info.checksum,
        platform_info.extract_path,
        platform_info.binary_path,
        platform_info.strategy,
        platform_info.source,
        platform_info.size
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
    download_component(
        &data_dir,
        name,
        url,
        platform_info,
        dest,
        &gateways,
        FirstPartyCarrierContext::Runtime,
    )
    .await
}

#[derive(Clone, Copy)]
pub(crate) enum FirstPartyCarrierContext {
    /// Standalone setup owns the operator's configured transport address.
    Setup,
    /// In-process downloads choose a temporary port on the configured IP;
    /// the running Runtime retains its listener and closes it at shutdown.
    Runtime,
}

fn first_party_carrier_bind_addr(
    data_dir: &Path,
    context: FirstPartyCarrierContext,
) -> anyhow::Result<Option<std::net::SocketAddr>> {
    Ok(
        crate::carrier::configured_carrier_bind_addr(data_dir)?.map(|mut address| {
            if matches!(context, FirstPartyCarrierContext::Runtime) {
                address.set_port(0);
            }
            address
        }),
    )
}

pub(crate) async fn fetch_first_party_component_via_carrier(
    data_dir: &Path,
    release_path: &str,
    context: FirstPartyCarrierContext,
) -> anyhow::Result<Vec<u8>> {
    let source = crate::sources::load_trusted_sources(data_dir)?
        .default_source()
        .cloned()
        .ok_or_else(missing_trusted_source_error)?;
    let bind_addr = first_party_carrier_bind_addr(data_dir, context)?;
    crate::carrier::fetch_file_from_trusted_source_bound(&source, release_path, 15, 30, bind_addr)
        .await
}

fn require_component_not_model(name: &str, dest: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        !(name.starts_with("model-") && name != "model-provider")
            && dest.extension().is_none_or(|extension| extension != "gguf"),
        "model capsules require signed directory preparation in Assistant, not archive downloads"
    );
    Ok(())
}

pub(crate) async fn install_first_party_component_via_carrier(
    data_dir: &Path,
    name: &str,
    platform_info: &PlatformInfo,
    dest: &Path,
    context: FirstPartyCarrierContext,
) -> anyhow::Result<()> {
    require_component_not_model(name, dest)?;
    if name == browser_vm_image::NAME {
        return browser_vm_image::install_via_carrier(
            data_dir,
            platform_info,
            dest,
            &detect_platform(),
            context,
        )
        .await;
    }
    let release_path = platform_info.release_path.as_deref().ok_or_else(|| {
        anyhow::anyhow!("missing release_path for first-party component '{}'", name)
    })?;
    let bytes = fetch_first_party_component_via_carrier(data_dir, release_path, context).await?;

    verify_checksum(name, &bytes, platform_info)?;

    let is_model = dest.extension().map(|e| e == "gguf").unwrap_or(false);
    let is_tarball = release_path.ends_with(".tar.gz")
        || release_path.ends_with(".tgz")
        || platform_info.extract_path.is_some();

    require_component_space(dest, &bytes, is_tarball)?;
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
    carrier_context: FirstPartyCarrierContext,
) -> anyhow::Result<()> {
    require_component_not_model(name, dest)?;
    if name == browser_vm_image::NAME {
        browser_vm_image::validate_request(data_dir, platform_info, dest, &detect_platform())?;
    }

    // Release artifacts come from the trusted source over Carrier.
    // A failed Carrier fetch stays a failure, even when a URL is present.
    if let Some(release_path) = platform_info.release_path.as_deref() {
        if release_path.trim().is_empty() {
            anyhow::bail!(
                "component '{}' must come from the signed release: present release_path is blank",
                name
            );
        }
        required_release_artifact_checksum(name, platform_info)?;
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
        match install_first_party_component_via_carrier(
            data_dir,
            name,
            platform_info,
            dest,
            carrier_context,
        )
        .await
        {
            Ok(()) => {
                ensure_bundle_executable_link(data_dir, name, platform_info)?;
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
    }

    let cid = platform_info
        .cid
        .as_deref()
        .filter(|cid| !cid.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "component '{}' must come from the signed release: release_path or cid is required; URL-only downloads are refused",
                name
            )
        })?;
    required_release_artifact_checksum(name, platform_info)?;
    let elastos_url = resolve_cid_display_url(cid);
    if ipfs_gateways.is_empty() {
        anyhow::bail!(
            "No configured fetch path for {} ({}). Configure a trusted source with a publisher gateway.",
            name,
            elastos_url
        );
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()?;
    println!("  Resolving {} from {}...", name, elastos_url);
    let mut last_err = String::new();
    let mut response = None;
    for gw in ipfs_gateways {
        let gw_url = format!("{}/ipfs/{}", gw.transport_base.trim_end_matches('/'), cid);
        println!("  Trying {} via {}...", elastos_url, gw.description);
        match client.get(&gw_url).send().await {
            Ok(r) if r.status().is_success() => {
                response = Some(r);
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
    let response = response.ok_or_else(|| {
        anyhow::anyhow!(
            "All configured Elastos fetch paths failed for {} ({}): {}",
            name,
            elastos_url,
            last_err
        )
    })?;
    let content_length = response.content_length();
    let is_model = dest.extension().map(|e| e == "gguf").unwrap_or(false);
    let is_tarball =
        url.ends_with(".tar.gz") || url.ends_with(".tgz") || platform_info.extract_path.is_some();

    if is_model {
        // Stream large model files to disk with progress
        download_streaming(name, response, dest, platform_info, content_length).await?;
    } else {
        // Buffer smaller binaries in memory for checksum
        let bytes = response.bytes().await?;

        verify_checksum(name, &bytes, platform_info)?;

        require_component_space(dest, &bytes, is_tarball)?;
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

    ensure_bundle_executable_link(data_dir, name, platform_info)?;

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

    let expected_checksum = required_release_artifact_checksum(name, platform_info)?;
    let tmp_path = dest.with_extension("tmp");
    let mut file = tokio::fs::File::create(&tmp_path).await?;

    // Set up the right hasher based on expected checksum format
    let use_sha512 = expected_checksum
        .map(|checksum| checksum.starts_with("sha512:"))
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
    if let Some(expected) = expected_checksum {
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
    let expected = match required_release_artifact_checksum(name, platform_info)? {
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

/// Component installs keep the shared free-space reserve. A tarball is written
/// and unpacked under the system temp directory, then copied to `dest`.
fn require_component_space(dest: &Path, bytes: &[u8], is_tarball: bool) -> anyhow::Result<()> {
    let temp = is_tarball
        .then(|| crate::install_transaction::volume_space(&std::env::temp_dir()))
        .transpose()?;
    component_space_fits(bytes, temp, crate::install_transaction::volume_space(dest)?)
}

/// `temp` and `dest` are (device, available bytes); `temp` is set for a tarball.
fn component_space_fits(
    bytes: &[u8],
    temp: Option<(u64, u128)>,
    dest: (u64, u128),
) -> anyhow::Result<()> {
    let Some(temp) = temp else {
        return Ok(elastos_common::require_free_space(
            dest.1,
            bytes.len() as u128,
        )?);
    };
    let mut unpacked = 0u128;
    for entry in tar::Archive::new(flate2::read::GzDecoder::new(bytes)).entries()? {
        unpacked += u128::from(entry?.size());
    }
    let staged = bytes.len() as u128 + unpacked;
    if temp.0 == dest.0 {
        return Ok(elastos_common::require_free_space(
            dest.1,
            staged + unpacked,
        )?);
    }
    elastos_common::require_free_space(temp.1, staged)?;
    Ok(elastos_common::require_free_space(dest.1, unpacked)?)
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

fn ensure_bundle_executable_link(
    data_dir: &Path,
    name: &str,
    platform_info: &PlatformInfo,
) -> anyhow::Result<()> {
    let Some(binary_path) = platform_info.binary_path.as_deref() else {
        return Ok(());
    };
    let install_path = platform_info.install_path.as_deref().ok_or_else(|| {
        anyhow::anyhow!("component '{name}' bundle executable requires install_path")
    })?;
    let install_path = Path::new(install_path);
    let binary_path = Path::new(binary_path);
    for (label, path) in [("install_path", install_path), ("binary_path", binary_path)] {
        if path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            anyhow::bail!("component '{name}' bundle {label} must be a relative safe path");
        }
    }

    let target = data_dir.join(install_path).join(binary_path);
    let metadata = fs::symlink_metadata(&target).map_err(|err| {
        anyhow::anyhow!(
            "component '{name}' bundle executable is unavailable at {}: {err}",
            target.display()
        )
    })?;
    if !metadata.file_type().is_file() {
        anyhow::bail!("component '{name}' bundle executable must be a regular file");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let bin_dir = data_dir.join("bin");
        fs::create_dir_all(&bin_dir)?;
        let link = bin_dir.join(name);
        if let Ok(existing) = fs::symlink_metadata(&link) {
            if !existing.file_type().is_symlink() {
                anyhow::bail!(
                    "component '{name}' cannot replace the existing non-link executable at {}",
                    link.display()
                );
            }
        }
        let temporary = bin_dir.join(format!(".{name}.link-{}", std::process::id()));
        let _ = fs::remove_file(&temporary);
        symlink(&target, &temporary)?;
        if let Err(err) = fs::rename(&temporary, &link) {
            let _ = fs::remove_file(&temporary);
            return Err(err.into());
        }
        Ok(())
    }

    #[cfg(not(unix))]
    anyhow::bail!("component '{name}' bundle executable links are unsupported on this platform")
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
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o755);
    }
    builder.create(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let target = dest.join(entry.file_name());
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else if metadata.file_type().is_symlink() {
            let link_target = fs::read_link(&path)?;
            if link_target.is_absolute()
                || link_target.components().any(|part| {
                    matches!(
                        part,
                        std::path::Component::ParentDir
                            | std::path::Component::RootDir
                            | std::path::Component::Prefix(_)
                    )
                })
            {
                anyhow::bail!("bundle symlink target is unsafe");
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                symlink(&link_target, &target)?;
            }
            #[cfg(not(unix))]
            anyhow::bail!("bundle symlinks are unsupported on this platform");
        } else if metadata.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&path, &target)?;
        } else {
            anyhow::bail!("bundle contains a special file");
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::sources::{save_trusted_sources, TrustedSource, TrustedSourcesConfig};
    use elastos_common::{CapsuleManifest, CapsuleRole};
    use std::collections::{BTreeMap, BTreeSet};

    // tokio Mutex so the async prerequisite test can hold the guard across
    // its await without blocking the runtime; sync tests use blocking_lock.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[test]
    fn arm64_model_profile_requires_dot_product_and_both_fp16_features() {
        assert!(arm64_model_cpu_features_available(ARM64_MODEL_HWCAP));
        assert!(arm64_model_cpu_features_available(
            ARM64_MODEL_HWCAP | (1 << 0)
        ));
        for bit in [1 << 9, 1 << 10, 1 << 20] {
            assert!(!arm64_model_cpu_features_available(
                ARM64_MODEL_HWCAP & !bit
            ));
        }
    }

    #[test]
    fn arm64_model_bundle_rejects_wrong_elf_machine_and_format() {
        let mut header = [0_u8; 20];
        header[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        header[16..18].copy_from_slice(&2_u16.to_le_bytes());
        header[18..20].copy_from_slice(&183_u16.to_le_bytes());
        assert!(arm64_model_elf_compatible(&header));
        header[18..20].copy_from_slice(&62_u16.to_le_bytes());
        assert!(!arm64_model_elf_compatible(&header));
        header[18..20].copy_from_slice(&183_u16.to_le_bytes());
        header[4] = 1;
        assert!(!arm64_model_elf_compatible(&header));
    }

    #[cfg(unix)]
    #[test]
    fn home_cli_renderer_archive_extraction_preserves_native_bytes_and_executability() {
        use std::os::unix::fs::PermissionsExt;

        let native = fs::read(std::env::current_exe().unwrap()).unwrap();
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut archive = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(native.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, "home-cli/bin/home-cli", native.as_slice())
            .unwrap();
        let bytes = archive.into_inner().unwrap().finish().unwrap();
        let info: PlatformInfo = serde_json::from_value(serde_json::json!({
            "release_path": format!("home-cli-{}.tar.gz", detect_platform()),
            "install_path": "capsules/home-cli",
            "extract_path": "home-cli",
            "checksum": format!("sha256:{}", hex::encode(sha2::Sha256::digest(&bytes)))
        }))
        .unwrap();
        verify_checksum("home-cli", &bytes, &info).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let installed = temp.path().join("capsules/home-cli");
        extract_from_tarball(&bytes, &installed, &info).unwrap();
        let renderer = installed.join("bin/home-cli");
        assert_eq!(fs::read(&renderer).unwrap(), native);
        assert_ne!(
            fs::metadata(&renderer).unwrap().permissions().mode() & 0o100,
            0
        );
        assert!(!temp.path().join("bin/home-cli").exists());
        let mut corrupt = bytes;
        corrupt[0] ^= 1;
        assert!(verify_checksum("home-cli", &corrupt, &info).is_err());
    }

    #[test]
    fn component_writes_keep_the_shared_free_space_reserve_at_the_boundary() {
        let reserve = u128::from(elastos_common::FREE_SPACE_RESERVE_BYTES);
        let fits = |bytes: &[u8], temp, dest_available| {
            component_space_fits(bytes, temp, (1, dest_available)).is_ok()
        };
        // A plain component writes its own bytes to the destination volume.
        let plain = vec![7u8; 1000];
        assert!(fits(&plain, None, 1000 + reserve));
        assert!(!fits(&plain, None, 1000 + reserve - 1));

        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut archive = tar::Builder::new(encoder);
        for (path, size) in [("tool/bin/tool", 3000_usize), ("tool/share/data", 5000)] {
            let mut header = tar::Header::new_gnu();
            header.set_size(size as u64);
            header.set_mode(0o755);
            header.set_cksum();
            archive
                .append_data(&mut header, path, vec![1u8; size].as_slice())
                .unwrap();
        }
        let tarball = archive.into_inner().unwrap().finish().unwrap();
        let unpacked = 8000;
        let staged = tarball.len() as u128 + unpacked;
        // Separate volumes: the archive and its unpacked tree on temp, the tree on dest.
        let temp_fits = Some((2, staged + reserve));
        assert!(fits(&tarball, temp_fits, unpacked + reserve));
        assert!(!fits(&tarball, temp_fits, unpacked + reserve - 1));
        assert!(!fits(
            &tarball,
            Some((2, staged + reserve - 1)),
            unpacked + reserve
        ));
        // One volume holds the staged archive, its unpacked tree and the copy.
        let shared = staged + unpacked + reserve;
        assert!(fits(&tarball, Some((1, 0)), shared));
        assert!(!fits(&tarball, Some((1, 0)), shared - 1));
    }

    #[cfg(unix)]
    #[test]
    fn extract_from_tarball_preserves_relative_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("llama-b10516");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("lib.0.dylib"), b"dylib\n").unwrap();
        symlink("lib.0.dylib", src.join("lib.dylib")).unwrap();
        let archive = temp.path().join("bundle.tar.gz");
        let status = Command::new("tar")
            .args([
                "czf",
                archive.to_str().unwrap(),
                "-C",
                temp.path().to_str().unwrap(),
                "llama-b10516",
            ])
            .status()
            .unwrap();
        assert!(status.success());
        let bytes = fs::read(&archive).unwrap();
        let info: PlatformInfo = serde_json::from_value(serde_json::json!({
            "release_path": "llama-b10516.tar.gz",
            "install_path": "libexec/llama.cpp/b10516/darwin-arm64",
            "extract_path": "llama-b10516",
            "checksum": format!("sha256:{}", hex::encode(sha2::Sha256::digest(&bytes)))
        }))
        .unwrap();
        let dest = temp.path().join("libexec/llama.cpp/b10516/darwin-arm64");
        extract_from_tarball(&bytes, &dest, &info).unwrap();
        let link = dest.join("lib.dylib");
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "installer must keep the signed dylib symlink"
        );
        assert_eq!(fs::read_link(&link).unwrap(), Path::new("lib.0.dylib"));
        assert_eq!(fs::read(dest.join("lib.0.dylib")).unwrap(), b"dylib\n");
    }

    #[test]
    fn test_detect_platform() {
        let p = detect_platform();
        assert!(
            p.contains("linux") || p.contains("darwin") || p.contains("unknown"),
            "Platform should contain os: {}",
            p
        );
        assert!(
            p.contains("amd64") || p.contains("arm64") || p.contains("unknown"),
            "Platform should contain arch: {}",
            p
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_non_media_selection_has_no_media_prerequisite_effect() {
        let temp = tempfile::tempdir().unwrap();
        let manifest: ComponentsManifest =
            serde_json::from_value(serde_json::json!({"external": {}, "profiles": {}})).unwrap();
        prepare_selected_component_prerequisites(
            temp.path(),
            &manifest,
            &detect_platform(),
            &["shell".to_string()],
            &[],
            None,
        )
        .await
        .unwrap();
        assert!(!temp
            .path()
            .join("protected-content/media-provider/config.json")
            .exists());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn test_missing_managed_media_tools_fail_before_first_component_install_effect() {
        let _guard = ENV_LOCK.lock().await;
        let temp = tempfile::tempdir().unwrap();
        let xdg_data_home = temp.path().join("xdg-data");
        let manifest_path = temp.path().join("components.json");
        let source = temp.path().join("effect-source");
        fs::write(&source, b"install-effect").unwrap();
        let platform = detect_platform();
        fs::write(
            &manifest_path,
            serde_json::to_vec(&serde_json::json!({
                "external": {
                    "effect": {
                        "install_path": "bin/effect",
                        "platforms": {
                            platform.clone(): {
                                "strategy": "local-copy",
                                "source": source,
                                "install_path": "bin/effect"
                            }
                        }
                    },
                    "media-provider": {
                        "install_path": "bin/media-provider",
                        "platforms": {}
                    }
                },
                "profiles": {
                    "media-preflight": {
                        "components": ["effect", "media-provider"]
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        // The data dir is injected rather than resolved from HOME/XDG env so
        // this test can never write into the live user installation and never
        // poisons concurrently running tests through process-global env.
        let data_dir = xdg_data_home.join("elastos");
        let original_manifest = std::env::var_os(COMPONENTS_MANIFEST_ENV);
        let original_path = std::env::var_os("PATH");
        std::env::set_var(COMPONENTS_MANIFEST_ENV, &manifest_path);
        std::env::set_var("PATH", "");

        let result = run_with_data_dir(
            data_dir.clone(),
            Some("media-preflight".to_string()),
            vec![],
            vec![],
            false,
            false,
            None,
        )
        .await;

        match original_manifest {
            Some(value) => std::env::set_var(COMPONENTS_MANIFEST_ENV, value),
            None => std::env::remove_var(COMPONENTS_MANIFEST_ENV),
        }
        match original_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }

        assert!(result.unwrap_err().to_string().contains("media-tools"));
        assert!(!data_dir.join("bin/effect").exists());
    }

    #[tokio::test]
    async fn test_explicit_media_tools_require_prerequisite_only_before_effects() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let error = run_with_data_dir(
            data.clone(),
            None,
            vec![],
            vec![],
            false,
            false,
            Some(temp.path().join("tools")),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("--prerequisites-only"));
        assert!(!data.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_managed_media_tools_create_private_data_before_fetch() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let temp = tempfile::tempdir().unwrap();
        let platform = detect_platform();
        let manifest: ComponentsManifest = serde_json::from_value(serde_json::json!({
            "external": {"media-tools": {
                "install_path": "tools/media-tools", "platforms": {platform.clone(): {
                    "release_path": format!("media-tools-{platform}.tar.gz"),
                    "extract_path": "media-tools", "checksum": format!("sha256:{}", "a".repeat(64))
                }}
            }}, "profiles": {}
        }))
        .unwrap();
        let selected = vec!["media-provider".to_owned()];
        let data = temp.path().join("fresh-data");
        let error = prepare_selected_component_prerequisites(
            &data,
            &manifest,
            &platform,
            &selected,
            &[],
            None,
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("Carrier fetch failed"),
            "{error}"
        );
        assert_eq!(
            fs::metadata(&data).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(!data.join("protected-content").exists());

        let existing = temp.path().join("existing-data");
        fs::create_dir(&existing).unwrap();
        fs::set_permissions(&existing, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(existing.join("retained"), "existing installation").unwrap();
        let error = prepare_selected_component_prerequisites(
            &existing,
            &manifest,
            &platform,
            &selected,
            &[],
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Runtime data root"), "{error}");
        assert_eq!(
            fs::metadata(&existing).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            fs::read(existing.join("retained")).unwrap(),
            b"existing installation"
        );
        assert!(!existing.join("tools").exists());
        let linked = temp.path().join("linked-data");
        symlink(&existing, &linked).unwrap();
        assert!(prepare_selected_component_prerequisites(
            &linked,
            &manifest,
            &platform,
            &selected,
            &[],
            None,
        )
        .await
        .is_err());
        assert!(fs::symlink_metadata(&linked).unwrap().is_symlink());
        assert!(!existing.join("tools").exists());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn test_managed_media_tools_are_imported_before_other_components() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = ENV_LOCK.lock().await;
        let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/media-setup-test-fixtures");
        fs::create_dir_all(&parent).unwrap();
        // The source parent is deliberately safe even when this test runs under umask 0002.
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        let temp = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(parent)
            .unwrap();
        let data = temp.path().join("data");
        let package = temp.path().join("package/media-tools/bin");
        fs::create_dir_all(&package).unwrap();
        for name in ["ffmpeg", "ffprobe"] {
            fs::write(package.join(name), name).unwrap();
            fs::set_permissions(package.join(name), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let archive = temp.path().join("media-tools.tar.gz");
        assert!(std::process::Command::new("tar")
            .args(["czf", archive.to_str().unwrap(), "-C"])
            .arg(temp.path().join("package"))
            .arg("media-tools")
            .status()
            .unwrap()
            .success());
        let archive_bytes = fs::read(&archive).unwrap();
        let checksum = format!("sha256:{:x}", sha2::Sha256::digest(&archive_bytes));
        let effect = temp.path().join("effect");
        fs::write(&effect, "other component").unwrap();
        let platform = detect_platform();
        let manifest_path = temp.path().join("components.json");
        fs::write(&manifest_path, serde_json::to_vec(&serde_json::json!({
            "external": {
                "media-tools": {"install_path": "tools/media-tools", "platforms": {platform.clone(): {
                    "release_path": format!("media-tools-{platform}.tar.gz"), "extract_path": "media-tools", "checksum": checksum}}},
                "media-provider": {"platforms": {}},
                "effect": {"install_path": "bin/effect", "platforms": {platform.clone(): {"strategy": "local-copy", "source": effect}}}
            }, "profiles": {"home": {"components": ["effect", "media-provider", "media-tools"]}}
        })).unwrap()).unwrap();
        let manifest = load_manifest_from_path(&manifest_path).unwrap();
        let selected = vec!["media-provider".to_owned()];
        let error = prepare_selected_component_prerequisites(
            &data,
            &manifest,
            &platform,
            &selected,
            &[],
            None,
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("Carrier fetch failed"),
            "{error}"
        );
        for path in [&data, &data.join("tools")] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        // Resume at the verified archive boundary. Carrier transport has separate fixtures;
        // extraction, installed cache selection and private import are the production code.
        let info = resolve_platform_info(&manifest.external["media-tools"], &platform).unwrap();
        verify_checksum("media-tools", &archive_bytes, info).unwrap();
        let dest = data.join("tools/media-tools");
        extract_from_tarball(&archive_bytes, &dest, info).unwrap();
        write_cache_metadata(&manifest, Some(info), &platform, "media-tools", &dest).unwrap();
        for path in [&dest, &dest.join("bin")] {
            assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o022, 0);
        }
        let original = std::env::var_os(COMPONENTS_MANIFEST_ENV);
        std::env::set_var(COMPONENTS_MANIFEST_ENV, &manifest_path);
        let result =
            run_with_data_dir(data.clone(), None, vec![], vec![], false, false, None).await;
        match original {
            Some(value) => std::env::set_var(COMPONENTS_MANIFEST_ENV, value),
            None => std::env::remove_var(COMPONENTS_MANIFEST_ENV),
        }
        result.unwrap();
        let installed = load_manifest_from_path(&data.join("components.json")).unwrap();
        assert_eq!(
            serde_json::to_value(&installed.external["media-tools"]).unwrap(),
            serde_json::to_value(&manifest.external["media-tools"]).unwrap()
        );
        let state = |manifest: &ComponentsManifest| {
            component_install_state_for_name(
                manifest,
                &data,
                "media-tools",
                &manifest.external["media-tools"],
                Some(info),
            )
        };
        assert_eq!(state(&manifest), InstallState::Installed);
        fs::write(dest.join(CACHED_ARTIFACT_SHA_FILE), "sha256:stale").unwrap();
        assert_eq!(
            state(&manifest),
            InstallState::Stale("extracted bundle checksum metadata missing or stale".into())
        );
        write_cache_metadata(&manifest, Some(info), &platform, "media-tools", &dest).unwrap();
        assert_eq!(state(&manifest), InstallState::Installed);
        let mut registered_capsule = manifest.clone();
        registered_capsule.capsules.insert(
            "media-tools".into(),
            serde_json::from_value(serde_json::json!({"cid": "test", "sha256": "test"})).unwrap(),
        );
        assert_eq!(
            state(&registered_capsule),
            InstallState::Stale("capsule metadata missing from installed bundle".into())
        );
        assert!(data
            .join("protected-content/media-provider/config.json")
            .is_file());
        for name in ["ffmpeg", "ffprobe"] {
            let private = data
                .join("protected-content/media-provider/tools")
                .join(name);
            assert_eq!(fs::read(&private).unwrap(), name.as_bytes());
            assert_eq!(
                fs::metadata(private).unwrap().permissions().mode() & 0o777,
                0o500
            );
        }
        assert_eq!(
            fs::read(data.join("bin/effect")).unwrap(),
            b"other component"
        );
    }

    #[test]
    fn extracted_bundle_checksum_receipt_matches_only_the_same_sha256_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let digest = "a".repeat(64);
        let reason = |expected: String| {
            let info: PlatformInfo = serde_json::from_value(serde_json::json!({
                "extract_path": "bundle", "checksum": expected
            }))
            .unwrap();
            extracted_bundle_cache_stale_reason(tmp.path(), &info)
        };
        let stale = Some("extracted bundle checksum metadata missing or stale".to_string());

        assert_eq!(reason(format!("sha256:{digest}")), stale, "receipt missing");
        // Registry capsule installs record the bare digest of the same archive.
        fs::write(
            tmp.path().join(CACHED_ARTIFACT_SHA_FILE),
            format!("{digest}\n"),
        )
        .unwrap();
        assert_eq!(reason(format!("sha256:{digest}")), None);
        assert_eq!(reason(format!("sha256:{}", "b".repeat(64))), stale);
        assert_eq!(reason(format!("sha512:{digest}")), stale);
        fs::write(
            tmp.path().join(CACHED_ARTIFACT_SHA_FILE),
            format!("sha256:{digest}\n"),
        )
        .unwrap();
        assert_eq!(reason(format!("sha512:{digest}")), stale);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn model_confinement_predeclares_only_current_pinned_engine_bundle() {
        let manifest_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../components.json");
        let mut manifest = load_manifest_from_path(&manifest_path).unwrap();
        let platform = detect_platform();
        // Source inputs are stamped by release packaging; this fixture models
        // the signed platform manifest consumed by an installed Runtime.
        manifest
            .external
            .get_mut("llama-server")
            .unwrap()
            .platforms
            .get_mut(&platform)
            .unwrap()
            .checksum = Some(format!("sha256:{}", "a".repeat(64)));
        let root = tempfile::tempdir().unwrap();
        let bundle = local_model_engine_confinement_bundle(root.path(), &manifest)
            .unwrap()
            .unwrap();
        assert!(!bundle.exists(), "predeclaration must precede installation");
        let version = manifest.external["llama-server"].version.as_ref().unwrap();
        assert_eq!(
            bundle,
            root.path()
                .canonicalize()
                .unwrap()
                .join(format!("libexec/llama.cpp/{version}/{platform}"))
        );
        let info = manifest
            .external
            .get_mut("llama-server")
            .unwrap()
            .platforms
            .get_mut(&platform)
            .unwrap();
        info.install_path = Some("providers/model-provider".into());
        assert!(local_model_engine_confinement_bundle(root.path(), &manifest).is_err());
        manifest.external.remove("llama-server");
        assert!(
            local_model_engine_confinement_bundle(root.path(), &manifest)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn test_current_checkout_manifest_content_matches_expected_profiles() {
        let manifest_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../components.json");
        let manifest = load_manifest_from_path(&manifest_path).unwrap();

        assert!(manifest.external.contains_key("kubo"));
        assert!(manifest.external.contains_key("archive-manager"));
        // Every prebuilt local model engine installs as a receipted bundle. The
        // release executables load their libraries from their own directory, so
        // a single copied executable would neither verify nor run.
        let engine = &manifest.external["llama-server"];
        let version = engine.version.as_deref().unwrap();
        let mut prebuilt = 0;
        for (platform, info) in &engine.platforms {
            if info.release_path.is_none() {
                continue;
            }
            prebuilt += 1;
            assert!(info.url.is_none(), "{platform} must use the signed release");
            assert_eq!(
                info.binary_path.as_deref(),
                Some("llama-server"),
                "{platform}"
            );
            assert_eq!(
                info.extract_path.as_deref(),
                Some(format!("llama-{version}").as_str()),
                "{platform}"
            );
            assert_eq!(
                info.install_path.as_deref(),
                Some(format!("libexec/llama.cpp/{version}/{platform}").as_str()),
                "{platform}"
            );
        }
        assert!(
            prebuilt >= 2,
            "darwin-arm64 and linux-amd64 engines are prebuilt"
        );
        for provider in [
            "net-provider",
            "exit-provider",
            "browser-engine-adapter",
            "object-provider",
            "webspace-provider",
            "content-block-graph-provider",
            "ipfs-provider",
            "wallet-provider",
        ] {
            assert!(
                manifest.external[provider]
                    .platforms
                    .contains_key("darwin-arm64"),
                "{provider} should be source-buildable on Apple Silicon"
            );
        }
        assert!(manifest.profiles.contains_key("home"));
        assert!(manifest.profiles["home"]
            .components
            .iter()
            .any(|component| component == "archive-manager"));
        let selected = resolve_components(&manifest, Some("home"), &[], &[]).unwrap();
        assert!(selected.iter().any(|name| name == "model-provider"));
        // Home lists only installed capsules; each Home profile installs its apps.
        for profile_name in ["home", "agent-local-ai", "public-gateway", "demo", "full"] {
            let selected = resolve_components(&manifest, Some(profile_name), &[], &[]).unwrap();
            for app in ["chat-room", "people", "inbox"] {
                assert!(
                    selected.iter().any(|name| name == app),
                    "{profile_name} must install {app}"
                );
            }
        }
        assert!(
            !selected.iter().any(|name| name == "llama-server"
                || manifest
                    .external
                    .get(name)
                    .is_some_and(|component| component
                        .install_path
                        .as_deref()
                        .is_some_and(|path| path.ends_with(".gguf")))),
            "Home installs the engine and models only on demand"
        );
        let on_demand =
            resolve_components(&manifest, None, &["llama-server".to_string()], &[]).unwrap();
        assert_eq!(on_demand, ["llama-server"]);
        for profile_name in ["home", "demo", "agent-local-ai", "public-gateway", "full"] {
            let profile = manifest
                .profiles
                .get(profile_name)
                .unwrap_or_else(|| panic!("{profile_name} profile is missing"));
            let has_wallet_or_browser_surface = profile.components.iter().any(|component| {
                matches!(
                    component.as_str(),
                    "wallet"
                        | "wallet-metamask"
                        | "wallet-unisat"
                        | "wallet-walletconnect"
                        | "browser"
                        | "inbox"
                )
            });
            if has_wallet_or_browser_surface {
                for provider in ["chain-provider", "wallet-provider"] {
                    assert!(
                        profile.components.iter().any(|component| component == provider),
                        "{profile_name} profile installs Wallet/Browser surfaces without {provider}"
                    );
                }
            }
        }
        let protected_home_dependencies = [
            "chain-provider",
            "wallet-provider",
            "kubo",
            "ipfs-provider",
            "protected-content-protect-provider",
            "media-provider",
            "protected-content-decrypt-provider",
            "library",
            "marketplace",
            "elacity-player",
        ];
        for profile_name in ["home", "demo", "agent-local-ai", "public-gateway", "full"] {
            let profile = &manifest.profiles[profile_name];
            for dependency in protected_home_dependencies {
                assert_eq!(
                    profile
                        .components
                        .iter()
                        .filter(|component| component.as_str() == dependency)
                        .count(),
                    1,
                    "{profile_name} protected-content Home profile must include {dependency} exactly once"
                );
            }
        }
        let private_providers = [
            "protected-content-protect-provider",
            "media-provider",
            "custody-provider",
            "protected-content-decrypt-provider",
        ];
        let provisional_providers = [
            "drm-provider",
            "rights-provider",
            "key-provider",
            "decrypt-provider",
        ];
        for profile_name in ["blockchain", "full"] {
            let profile = &manifest.profiles[profile_name];
            for provider in private_providers.into_iter().chain(provisional_providers) {
                assert_eq!(
                    profile
                        .components
                        .iter()
                        .filter(|component| component.as_str() == provider)
                        .count(),
                    1,
                    "{profile_name} profile must include {provider} exactly once"
                );
            }
        }
        let custody_profiles = manifest
            .profiles
            .iter()
            .filter_map(|(name, profile)| {
                profile
                    .components
                    .iter()
                    .any(|component| component == "custody-provider")
                    .then_some(name.as_str())
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(custody_profiles, BTreeSet::from(["blockchain", "full"]));
        let operator_with_custody = resolve_components(
            &manifest,
            Some("operator"),
            &["chain-provider".to_string(), "custody-provider".to_string()],
            &[],
        )
        .unwrap();
        assert_eq!(
            operator_with_custody,
            [
                "shell",
                "localhost-provider",
                "did-provider",
                "chain-provider",
                "custody-provider",
            ]
        );
        assert!(!manifest.profiles.contains_key("chat"));
        assert!(manifest.profiles.contains_key("blockchain"));
        assert!(manifest.profiles.contains_key("operator"));
        assert!(manifest.profiles.contains_key("full"));
    }

    #[test]
    fn test_load_manifest_uses_explicit_manifest_override() {
        let _guard = ENV_LOCK.blocking_lock();
        let temp = tempfile::tempdir().unwrap();
        let source_manifest =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../components.json");
        let override_manifest = temp.path().join("components.json");
        let mut manifest_json: serde_json::Value =
            serde_json::from_slice(&fs::read(&source_manifest).unwrap()).unwrap();
        manifest_json["external"]
            .as_object_mut()
            .unwrap()
            .remove("kubo");
        fs::write(
            &override_manifest,
            serde_json::to_vec_pretty(&manifest_json).unwrap(),
        )
        .unwrap();

        std::env::set_var(COMPONENTS_MANIFEST_ENV, &override_manifest);
        let manifest = load_manifest().unwrap();
        std::env::remove_var(COMPONENTS_MANIFEST_ENV);

        assert!(!manifest.external.contains_key("kubo"));
        assert!(manifest.external.contains_key("archive-manager"));
    }

    fn first_party_provider_manifest_path(root: &Path, name: &str) -> Option<PathBuf> {
        [
            root.join("capsules").join(name).join("capsule.json"),
            root.join("elastos")
                .join("capsules")
                .join(name)
                .join("capsule.json"),
        ]
        .into_iter()
        .find(|path| path.is_file())
    }

    #[test]
    fn provider_runtime_contract_covers_exact_active_provider_set() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../");
        let components: ComponentsManifest =
            serde_json::from_slice(&fs::read(root.join("components.json")).unwrap()).unwrap();
        let expected = BTreeMap::from([
            (
                "browser-engine-adapter".to_string(),
                "elastos://browser-engine/*".to_string(),
            ),
            (
                "chain-provider".to_string(),
                "elastos://chain/*".to_string(),
            ),
            (
                "content-block-graph-provider".to_string(),
                "elastos://block-graph/*".to_string(),
            ),
            ("custody-provider".to_string(), "custody".to_string()),
            ("did-provider".to_string(), "elastos://did/*".to_string()),
            ("exit-provider".to_string(), "elastos://exit/*".to_string()),
            ("ipfs-provider".to_string(), "elastos://ipfs/*".to_string()),
            (
                "localhost-provider".to_string(),
                "localhost://*".to_string(),
            ),
            (
                "model-provider".to_string(),
                "elastos://model/*".to_string(),
            ),
            ("media-provider".to_string(), "media".to_string()),
            ("net-provider".to_string(), "elastos://net/*".to_string()),
            (
                "object-provider".to_string(),
                "elastos://object/*".to_string(),
            ),
            (
                "protected-content-decrypt-provider".to_string(),
                "protected-content-decrypt".to_string(),
            ),
            (
                "protected-content-protect-provider".to_string(),
                "protect".to_string(),
            ),
            (
                "wallet-provider".to_string(),
                "elastos://wallet/meta/status".to_string(),
            ),
            (
                "webspace-provider".to_string(),
                "localhost://WebSpaces/*".to_string(),
            ),
        ]);
        let actual = components
            .external
            .iter()
            .filter_map(|(name, component)| {
                component
                    .provider_runtime
                    .as_ref()
                    .map(|runtime| (name.clone(), runtime.provides.clone()))
            })
            .collect::<BTreeMap<_, _>>();

        assert_eq!(actual, expected);

        for helper in [
            "browser-engine-supervisor",
            "browser-local-exit",
            "browser-native-proxy-engine",
            "browser-stream-bridge",
        ] {
            let component = components.external.get(helper).unwrap();
            assert!(component.provider_runtime.is_none(), "{helper}");
        }

        for (name, provides) in expected {
            let component = components.external.get(&name).unwrap();
            let runtime = validate_provider_runtime(&name, component).unwrap();
            let expected_install_path = format!("bin/{name}");
            assert_eq!(
                component.install_path.as_deref(),
                Some(expected_install_path.as_str())
            );
            assert_eq!(runtime.provides, provides);
            assert_eq!(
                runtime.runtime_only,
                matches!(
                    name.as_str(),
                    "custody-provider"
                        | "media-provider"
                        | "protected-content-decrypt-provider"
                        | "protected-content-protect-provider"
                )
            );
            if runtime.runtime_only {
                assert!(first_party_provider_manifest_path(&root, &name).is_none());
                continue;
            }
            let path = first_party_provider_manifest_path(&root, &name).unwrap();
            let manifest: CapsuleManifest =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert_eq!(manifest.role, CapsuleRole::Provider);
            assert_eq!(
                manifest.provides.as_deref(),
                Some(runtime.provides.as_str())
            );
        }

        for profile in ["agent-local-ai", "full"] {
            assert!(
                components
                    .profiles
                    .get(profile)
                    .unwrap()
                    .components
                    .iter()
                    .any(|value| value == "model-provider"),
                "profile {profile} must install model-provider"
            );
        }
        assert!(
            !components
                .profiles
                .get("public-gateway")
                .unwrap()
                .components
                .iter()
                .any(|value| value == "model-provider"),
            "public-gateway must not install model-provider"
        );
    }

    #[test]
    fn assistant_capsule_is_packaged_with_a_capsule_owned_icon() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../");
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.join("capsules/assistant/capsule.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["schema"], "elastos.capsule/v1");
        assert_eq!(manifest["name"], "assistant");
        assert_eq!(manifest["icon"], "browser/icons");
        assert_eq!(manifest["entrypoint"], "browser/index.html");

        for file in ["icon-32.png", "icon-64.png", "icon-128.png", "icon-256.png"] {
            assert!(
                root.join("capsules/assistant/browser/icons")
                    .join(file)
                    .is_file(),
                "missing Assistant icon asset {file}"
            );
        }

        let components: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("components.json")).unwrap()).unwrap();
        assert_eq!(
            components["external"]["assistant"]["install_path"],
            "capsules/assistant"
        );
        for profile in ["home", "demo", "agent-local-ai", "public-gateway", "full"] {
            assert!(
                components["profiles"][profile]["components"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|value| value == "assistant"),
                "profile {profile} must include the Assistant capsule"
            );
        }
    }

    #[test]
    fn service_provider_capsules_are_packaged_with_capsule_owned_icons() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../");
        for name in [
            "browser-engine-adapter",
            "chain-provider",
            "content-block-graph-provider",
            "did-provider",
            "exit-provider",
            "ipfs-provider",
            "model-provider",
            "net-provider",
            "object-provider",
            "wallet-provider",
            "webspace-provider",
        ] {
            let capsule_dir = root.join("capsules").join(name);
            let manifest: elastos_common::CapsuleManifest =
                serde_json::from_slice(&fs::read(capsule_dir.join("capsule.json")).unwrap())
                    .unwrap();
            manifest
                .validate()
                .unwrap_or_else(|err| panic!("{name} manifest must validate: {err}"));
            assert_eq!(manifest.role, elastos_common::CapsuleRole::Provider);
            assert_eq!(
                manifest.icon.as_deref(),
                Some("icons"),
                "{name} must own its icon"
            );
            for file in ["icon-32.png", "icon-64.png", "icon-128.png", "icon-256.png"] {
                assert!(
                    capsule_dir.join("icons").join(file).is_file(),
                    "missing {name} icon asset {file}"
                );
            }
        }
    }

    #[test]
    fn elacity_player_capsule_is_packaged_with_a_capsule_owned_icon() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../");
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.join("capsules/elacity-player/capsule.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["schema"], "elastos.capsule/v1");
        assert_eq!(manifest["name"], "elacity-player");
        assert_eq!(manifest["icon"], "browser/icons");
        assert_eq!(manifest["entrypoint"], "browser/index.html");

        for file in ["icon-32.png", "icon-64.png", "icon-128.png", "icon-256.png"] {
            assert!(
                root.join("capsules/elacity-player/browser/icons")
                    .join(file)
                    .is_file(),
                "missing Elacity Player icon asset {file}"
            );
        }

        let components: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("components.json")).unwrap()).unwrap();
        assert_eq!(
            components["external"]["elacity-player"]["install_path"],
            "capsules/elacity-player"
        );
        for profile in ["home", "demo", "agent-local-ai", "public-gateway", "full"] {
            assert!(
                components["profiles"][profile]["components"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|value| value == "elacity-player"),
                "profile {profile} must include the Elacity Player capsule"
            );
        }
    }

    #[test]
    fn test_normalize_profile_name_preserves_home_profile() {
        assert_eq!(normalize_profile_name("home"), "home");
    }

    #[test]
    fn test_source_checkout_manifest_path_from_release_binary_layout() {
        let path = PathBuf::from("/tmp/elastos-runtime/elastos/target/release/elastos");
        let manifest = source_checkout_manifest_path(&path).unwrap();
        assert_eq!(
            manifest,
            PathBuf::from("/tmp/elastos-runtime/components.json")
        );
    }

    #[test]
    fn test_source_checkout_manifest_path_from_test_binary_layout() {
        let path = PathBuf::from("/tmp/elastos-runtime/elastos/target/debug/deps/elastos-server");
        let manifest = source_checkout_manifest_path(&path).unwrap();
        assert_eq!(
            manifest,
            PathBuf::from("/tmp/elastos-runtime/components.json")
        );
    }

    #[test]
    fn test_missing_manifest_message_mentions_source_checkout() {
        let message = missing_manifest_message();
        assert!(message.contains("ELASTOS_COMPONENTS_MANIFEST"));
        assert!(message.contains("<source-checkout>/components.json"));
        assert!(message.contains("Source-built binaries are not self-contained installs."));
    }

    #[test]
    fn test_missing_trusted_source_error_mentions_source_add() {
        let err = missing_trusted_source_error();
        let message = err.to_string();
        assert!(message.contains("elastos setup"));
        assert!(message.contains("elastos source add"));
    }

    #[test]
    fn test_resolve_platform_info_accepts_release_platform_alias() {
        let comp: Component = serde_json::from_value(serde_json::json!({
            "install_path": "bin/example",
            "platforms": {
                "linux-arm64": {
                    "cid": "QmAlias",
                    "checksum": "sha256:deadbeef"
                },
                "darwin-arm64": {
                    "cid": "QmDarwinAlias",
                    "checksum": "sha256:feedface"
                }
            }
        }))
        .unwrap();

        let info = resolve_platform_info(&comp, "aarch64-linux").unwrap();
        assert_eq!(info.cid.as_deref(), Some("QmAlias"));

        let info = resolve_platform_info(&comp, "aarch64-darwin").unwrap();
        assert_eq!(info.cid.as_deref(), Some("QmDarwinAlias"));
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
            repository: None,
            size_mb: None,
            description: None,
            provider_runtime: None,
            capsule_metadata: None,
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
                binary_path: None,
                strategy: None,
                source: None,
                note: None,
                size: Some(10),
            },
        );
        let comp = Component {
            version: Some("0.20.0-rc30".to_string()),
            install_path: Some("bin/site-provider".to_string()),
            repository: None,
            size_mb: None,
            description: None,
            provider_runtime: None,
            capsule_metadata: None,
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

    #[cfg(unix)]
    #[test]
    fn directory_bundle_exposes_declared_binary_at_stable_path() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = tmp.path().join("libexec/llama.cpp/b10516/darwin-arm64");
        fs::create_dir_all(&bundle).unwrap();
        let binary = bundle.join("llama-server");
        fs::write(&binary, b"llama-server").unwrap();
        let platform_info = PlatformInfo {
            url: None,
            cid: None,
            release_path: None,
            checksum: None,
            extract_path: Some("llama-b10516".to_string()),
            install_path: Some("libexec/llama.cpp/b10516/darwin-arm64".to_string()),
            binary_path: Some("llama-server".to_string()),
            strategy: None,
            source: None,
            note: None,
            size: None,
        };

        ensure_bundle_executable_link(tmp.path(), "llama-server", &platform_info).unwrap();
        let link = tmp.path().join("bin/llama-server");
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::canonicalize(&link).unwrap(),
            fs::canonicalize(&binary).unwrap()
        );

        ensure_bundle_executable_link(tmp.path(), "llama-server", &platform_info).unwrap();
        assert_eq!(
            fs::canonicalize(&link).unwrap(),
            fs::canonicalize(&binary).unwrap()
        );

        let unsafe_info = PlatformInfo {
            binary_path: Some("../llama-server".to_string()),
            ..platform_info
        };
        assert!(
            ensure_bundle_executable_link(tmp.path(), "llama-server", &unsafe_info)
                .unwrap_err()
                .to_string()
                .contains("relative safe path")
        );
    }

    #[cfg(unix)]
    fn restore_engine_tree_for_cleanup(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                restore_engine_tree_for_cleanup(&entry.path());
            }
        }
    }

    #[cfg(unix)]
    fn unix_engine_identity_fixture(root: &Path) -> (ComponentsManifest, PathBuf, PathBuf) {
        unix_engine_identity_fixture_with_binary(root, "llama-server")
    }

    #[cfg(unix)]
    fn unix_engine_identity_fixture_with_binary(
        root: &Path,
        binary_path: &str,
    ) -> (ComponentsManifest, PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let data = root.join("data");
        let platform = detect_platform();
        let relative = format!("libexec/llama.cpp/fixture-v1/{platform}");
        let bundle = data.join(&relative);
        let executable = bundle.join(binary_path);
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, b"fixture llama-server\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let archive = format!("sha256:{}", "a".repeat(64));
        let manifest = serde_json::from_value(serde_json::json!({
            "external": {
                "llama-server": {
                    "version": "fixture-v1",
                    "platforms": { platform: {
                        "url": "https://fixture.invalid/llama.tar.gz",
                        "checksum": archive,
                        "extract_path": "llama-fixture-v1",
                        "install_path": relative,
                        "binary_path": binary_path
                    }}
                }
            },
            "profiles": {}
        }))
        .unwrap();
        (manifest, data, bundle)
    }

    #[cfg(unix)]
    fn mark_engine_parents_group_writable(data: &Path, bundle: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut path = bundle.parent().unwrap().to_path_buf();
        while path.starts_with(data) && path != data {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o775)).unwrap();
            path = path.parent().unwrap().to_path_buf();
        }
        fs::set_permissions(data.join("libexec"), fs::Permissions::from_mode(0o775)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn local_model_engine_identity_rejects_group_writable_install_parents() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let (manifest, data, bundle) = unix_engine_identity_fixture(tmp.path());
        let component = &manifest.external["llama-server"];
        let platform = detect_platform();
        write_cache_metadata(
            &manifest,
            resolve_platform_info(component, &platform),
            &platform,
            "llama-server",
            &bundle,
        )
        .unwrap();
        restore_engine_tree_for_cleanup(&data);
        mark_engine_parents_group_writable(&data, &bundle);
        fs::set_permissions(&bundle, fs::Permissions::from_mode(0o500)).unwrap();
        fs::set_permissions(
            bundle.join("llama-server"),
            fs::Permissions::from_mode(0o500),
        )
        .unwrap();
        fs::set_permissions(
            bundle.join(".elastos-engine.json"),
            fs::Permissions::from_mode(0o400),
        )
        .unwrap();
        let error = local_model_engine_receipt_identity(&data, &manifest)
            .err()
            .expect("identity must fail")
            .to_string();
        restore_engine_tree_for_cleanup(&data);
        assert!(
            error.contains("model engine parent is not protected"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_cache_metadata_clears_group_writable_parents() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let (manifest, data, bundle) = unix_engine_identity_fixture(tmp.path());
        mark_engine_parents_group_writable(&data, &bundle);
        let component = &manifest.external["llama-server"];
        let platform = detect_platform();
        write_cache_metadata(
            &manifest,
            resolve_platform_info(component, &platform),
            &platform,
            "llama-server",
            &bundle,
        )
        .unwrap();
        for path in [
            data.join("libexec"),
            data.join("libexec/llama.cpp"),
            data.join("libexec/llama.cpp/fixture-v1"),
        ] {
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o022,
                0,
                "{}",
                path.display()
            );
        }
        restore_engine_tree_for_cleanup(&data);
    }

    #[cfg(unix)]
    #[test]
    fn verified_local_model_engine_rejects_group_writable_parents_without_chmod() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let (manifest, data, bundle) = unix_engine_identity_fixture(tmp.path());
        let component = &manifest.external["llama-server"];
        let platform = detect_platform();
        write_cache_metadata(
            &manifest,
            resolve_platform_info(component, &platform),
            &platform,
            "llama-server",
            &bundle,
        )
        .unwrap();
        restore_engine_tree_for_cleanup(&data);
        mark_engine_parents_group_writable(&data, &bundle);
        fs::set_permissions(&bundle, fs::Permissions::from_mode(0o500)).unwrap();
        fs::set_permissions(
            bundle.join("llama-server"),
            fs::Permissions::from_mode(0o500),
        )
        .unwrap();
        fs::set_permissions(
            bundle.join(".elastos-engine.json"),
            fs::Permissions::from_mode(0o400),
        )
        .unwrap();
        let libexec = data.join("libexec");
        let before = fs::metadata(&libexec).unwrap().permissions().mode() & 0o777;
        let error = verified_local_model_engine(&data, &manifest)
            .err()
            .expect("verification must stay read-only")
            .to_string();
        let after = fs::metadata(&libexec).unwrap().permissions().mode() & 0o777;
        restore_engine_tree_for_cleanup(&data);
        assert!(
            error.contains("model engine parent is not protected"),
            "{error}"
        );
        assert_eq!(before, 0o775);
        assert_eq!(after, 0o775);
    }

    #[cfg(unix)]
    #[test]
    fn verified_local_model_engine_accepts_a_nested_binary_path() {
        let tmp = tempfile::tempdir().unwrap();
        let (manifest, data, bundle) =
            unix_engine_identity_fixture_with_binary(tmp.path(), "bin/llama-server");
        let component = &manifest.external["llama-server"];
        let platform = detect_platform();
        write_cache_metadata(
            &manifest,
            resolve_platform_info(component, &platform),
            &platform,
            "llama-server",
            &bundle,
        )
        .unwrap();
        let identity = verified_local_model_engine(&data, &manifest)
            .expect("configured install_path remains the receipt bundle root");
        let expected = bundle.canonicalize().unwrap().join("bin/llama-server");
        restore_engine_tree_for_cleanup(&data);
        assert_eq!(identity.path, expected);
    }

    #[cfg(unix)]
    #[test]
    fn protect_engine_parents_rejects_symlink_escape_without_mutating_outside() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let victim = tmp.path().join("victim");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&victim).unwrap();
        let sentinel = victim.join("sentinel");
        fs::write(&sentinel, b"outside-bytes").unwrap();
        fs::set_permissions(&victim, fs::Permissions::from_mode(0o775)).unwrap();
        symlink(&victim, data.join("libexec")).unwrap();
        let platform = detect_platform();
        let install_path = format!("libexec/llama.cpp/fixture-v1/{platform}");
        let dest = data.join(&install_path);
        let error = protect_local_model_engine_install_parents(&dest, &install_path)
            .expect_err("protect must reject an aliased install parent")
            .to_string();
        let victim_mode = fs::metadata(&victim).unwrap().permissions().mode() & 0o777;
        let sentinel_bytes = fs::read(&sentinel).unwrap();
        assert!(
            error.contains("model engine parent is not protected"),
            "{error}"
        );
        assert_eq!(victim_mode, 0o775);
        assert_eq!(sentinel_bytes, b"outside-bytes");
    }

    #[cfg(target_os = "macos")]
    fn llama_bundle_fixture(root: &Path) -> (ComponentsManifest, PathBuf, PathBuf) {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let data = root.join("data");
        let bundle = data.join("libexec/llama.cpp/fixture-v1/darwin-arm64");
        fs::create_dir_all(&bundle).unwrap();
        fs::write(bundle.join("libfixture.dylib"), b"fixture dylib\n").unwrap();
        fs::write(bundle.join("llama-server"), b"fixture llama-server\n").unwrap();
        fs::set_permissions(
            bundle.join("llama-server"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        symlink("libfixture.dylib", bundle.join("libllama.dylib")).unwrap();
        let model = data.join("models/stable.gguf");
        fs::create_dir_all(model.parent().unwrap()).unwrap();
        fs::write(&model, b"stable model\n").unwrap();
        let manifest = serde_json::from_value(serde_json::json!({
            "external": {
                "llama-server": {
                    "version": "fixture-v1",
                    "platforms": {"darwin-arm64": {
                        "url": "https://fixture.invalid/llama.tar.gz",
                        "checksum": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "extract_path": "llama-fixture-v1",
                        "install_path": "libexec/llama.cpp/fixture-v1/darwin-arm64",
                        "binary_path": "llama-server",
                        "strategy": "prebuilt"
                    }}
                },
                "model-qwen3.5-9b": {
                    "description": "stable fixture",
                    "platforms": {"*": {
                        "url": "https://fixture.invalid/stable.gguf",
                        "checksum": compute_sha256_checksum(&model).unwrap(),
                        "install_path": "models/stable.gguf"
                    }}
                }
            },
            "profiles": {}
        }))
        .unwrap();
        (manifest, data, bundle)
    }

    #[cfg(target_os = "macos")]
    fn write_fetch_fixture_receipt(bundle: &Path) {
        use std::os::unix::fs::PermissionsExt;

        fs::write(
            bundle.join(".elastos-engine.json"),
            serde_json::to_vec(&serde_json::json!({
                "archive_sha256": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "entries": [
                    {"path": "libfixture.dylib", "sha256": compute_sha256_checksum(&bundle.join("libfixture.dylib")).unwrap(), "type": "file"},
                    {"path": "libllama.dylib", "target": "libfixture.dylib", "type": "symlink"},
                    {"path": "llama-server", "sha256": compute_sha256_checksum(&bundle.join("llama-server")).unwrap(), "type": "file"}
                ],
                "platform": "darwin-arm64",
                "schema": "elastos.local-model-engine/v2",
                "version": "fixture-v1"
            }))
            .unwrap(),
        )
        .unwrap();
        for (path, mode) in [
            (bundle.join("libfixture.dylib"), 0o400),
            (bundle.join("llama-server"), 0o500),
            (bundle.join(".elastos-engine.json"), 0o400),
            (bundle.to_path_buf(), 0o500),
        ] {
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn fetch_llama_bundle_has_exact_inventory_for_runtime_setup() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let (manifest, data, bundle) = llama_bundle_fixture(tmp.path());
        write_fetch_fixture_receipt(&bundle);
        let component = &manifest.external["llama-server"];
        let state = component_install_state_for_name(
            &manifest,
            &data,
            "llama-server",
            component,
            resolve_platform_info(component, "darwin-arm64"),
        );
        assert_eq!(state, InstallState::Installed);
        fs::set_permissions(&bundle, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(bundle.join("unexpected.dylib"), b"unexpected\n").unwrap();
        fs::set_permissions(&bundle, fs::Permissions::from_mode(0o500)).unwrap();
        assert!(matches!(
            component_install_state_for_name(
                &manifest,
                &data,
                "llama-server",
                component,
                resolve_platform_info(component, "darwin-arm64"),
            ),
            InstallState::Stale(_)
        ));
        fs::set_permissions(&bundle, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_file(bundle.join("unexpected.dylib")).unwrap();
        let library = bundle.join("libfixture.dylib");
        fs::set_permissions(&library, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&library, b"changed library\n").unwrap();
        fs::set_permissions(&library, fs::Permissions::from_mode(0o400)).unwrap();
        fs::set_permissions(&bundle, fs::Permissions::from_mode(0o500)).unwrap();
        assert!(matches!(
            component_install_state_for_name(
                &manifest,
                &data,
                "llama-server",
                component,
                resolve_platform_info(component, "darwin-arm64"),
            ),
            InstallState::Stale(_)
        ));
        fs::set_permissions(&bundle, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn test_component_install_state_detects_stale_extracted_bundle_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let install_root = tmp.path().join("capsules/home-cli");
        fs::create_dir_all(&install_root).unwrap();
        fs::write(
            install_root.join("capsule.json"),
            b"{\"name\":\"home-cli\"}",
        )
        .unwrap();

        let mut platforms = HashMap::new();
        platforms.insert(
            "linux-amd64".to_string(),
            PlatformInfo {
                url: None,
                cid: Some("QmNewHomeCli".to_string()),
                release_path: Some("home-cli.tar.gz".to_string()),
                checksum: Some("sha256:new-home-cli-archive".to_string()),
                extract_path: Some("home-cli".to_string()),
                install_path: Some("capsules/home-cli".to_string()),
                binary_path: None,
                strategy: None,
                source: None,
                note: None,
                size: Some(1234),
            },
        );
        let component = Component {
            version: Some("0.1.0".to_string()),
            install_path: Some("capsules/home-cli".to_string()),
            repository: None,
            size_mb: None,
            description: None,
            provider_runtime: None,
            capsule_metadata: None,
            platforms,
        };
        let manifest: ComponentsManifest = serde_json::from_value(serde_json::json!({
            "external": {
                "home-cli": {
                    "version": "0.1.0",
                    "install_path": "capsules/home-cli",
                    "platforms": {
                        "linux-amd64": {
                            "cid": "QmNewHomeCli",
                            "release_path": "home-cli.tar.gz",
                            "checksum": "sha256:new-home-cli-archive",
                            "extract_path": "home-cli",
                            "install_path": "capsules/home-cli"
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
                "home-cli",
                &component,
                resolve_platform_info(&component, "linux-amd64")
            ),
            InstallState::Stale("extracted bundle CID metadata missing or stale".to_string())
        );
    }

    fn provider_component_with_capsule_metadata() -> Component {
        let mut platforms = HashMap::new();
        platforms.insert(
            "linux-amd64".to_string(),
            PlatformInfo {
                url: None,
                cid: None,
                release_path: Some("object-provider-linux-amd64".to_string()),
                checksum: None,
                extract_path: None,
                install_path: Some("bin/object-provider".to_string()),
                binary_path: None,
                strategy: None,
                source: None,
                note: None,
                size: None,
            },
        );
        let mut capsule_platforms = HashMap::new();
        capsule_platforms.insert(
            "*".to_string(),
            PlatformInfo {
                url: None,
                cid: Some(
                    "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi".to_string(),
                ),
                release_path: Some("object-provider-capsule-metadata.tar.gz".to_string()),
                checksum: Some(
                    "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                        .to_string(),
                ),
                extract_path: Some("object-provider".to_string()),
                install_path: Some("capsules/object-provider".to_string()),
                binary_path: None,
                strategy: None,
                source: None,
                note: None,
                size: Some(123),
            },
        );
        Component {
            version: None,
            install_path: Some("bin/object-provider".to_string()),
            repository: None,
            size_mb: None,
            description: None,
            provider_runtime: Some(ProviderRuntime {
                role: ProviderRuntimeRole::Provider,
                substrate: ProviderRuntimeSubstrate::Native,
                runtime_abi: ProviderRuntimeAbi::ProviderStdioV1,
                execution: ProviderRuntimeExecution::NativeProvider,
                provides: "elastos://object/*".to_string(),
                runtime_only: false,
            }),
            capsule_metadata: Some(ComponentCapsuleMetadata {
                install_path: Some("capsules/object-provider".to_string()),
                platforms: capsule_platforms,
            }),
            platforms,
        }
    }

    #[test]
    fn passive_capsule_metadata_admits_content_without_provider_authority() {
        let tmp = tempfile::tempdir().unwrap();
        let mut component = provider_component_with_capsule_metadata();
        component.provider_runtime = None;
        let manifest = serde_json::json!({
            "schema": "elastos.capsule/v1", "name": "kubo", "version": "0.40.1",
            "role": "content", "type": "data", "projections": ["content"],
            "entrypoint": "ipfs"
        });
        fs::write(tmp.path().join("ipfs"), b"packaged kubo").unwrap();
        fs::write(
            tmp.path().join("capsule.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert_eq!(
            installed_component_capsule_metadata_stale_reason("kubo", &component, tmp.path()),
            None
        );

        for (field, value) in [
            ("name", serde_json::json!("another-component")),
            ("role", serde_json::json!("provider")),
            ("type", serde_json::json!("wasm")),
            (
                "runtime_abi",
                serde_json::json!("elastos.provider-stdio/v1"),
            ),
            ("execution", serde_json::json!("native-provider")),
            ("capabilities", serde_json::json!(["elastos://object/*"])),
            ("permissions", serde_json::json!({"storage": ["shared"]})),
            ("entrypoint", serde_json::json!("../outside")),
            ("entrypoint", serde_json::json!("missing")),
        ] {
            let mut refused = manifest.clone();
            refused[field] = value;
            fs::write(
                tmp.path().join("capsule.json"),
                serde_json::to_vec(&refused).unwrap(),
            )
            .unwrap();
            assert!(
                installed_component_capsule_metadata_stale_reason("kubo", &component, tmp.path())
                    .is_some(),
                "{field}"
            );
        }
        fs::write(
            tmp.path().join("capsule.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        fs::remove_file(tmp.path().join("ipfs")).unwrap();
        std::os::unix::fs::symlink("capsule.json", tmp.path().join("ipfs")).unwrap();
        assert!(
            installed_component_capsule_metadata_stale_reason("kubo", &component, tmp.path())
                .is_some()
        );
    }

    #[test]
    fn provider_capsule_metadata_keeps_role_and_icon_admission() {
        let tmp = tempfile::tempdir().unwrap();
        let component = provider_component_with_capsule_metadata();
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../capsules/object-provider/capsule.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(source).unwrap()).unwrap();
        fs::write(
            tmp.path().join("capsule.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(installed_component_capsule_metadata_stale_reason(
            "object-provider",
            &component,
            tmp.path()
        )
        .is_some());
        fs::create_dir(tmp.path().join("icons")).unwrap();
        for size in PROVIDER_ICON_SIZES {
            fs::write(tmp.path().join(format!("icons/icon-{size}.png")), b"icon").unwrap();
        }
        assert_eq!(
            installed_component_capsule_metadata_stale_reason(
                "object-provider",
                &component,
                tmp.path()
            ),
            None
        );
        manifest = serde_json::json!({
            "schema": "elastos.capsule/v1", "name": "object-provider", "version": "0.2.0",
            "role": "content", "type": "data", "entrypoint": "payload"
        });
        fs::write(tmp.path().join("payload"), b"passive downgrade").unwrap();
        fs::write(
            tmp.path().join("capsule.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(installed_component_capsule_metadata_stale_reason(
            "object-provider",
            &component,
            tmp.path()
        )
        .is_some());
    }

    #[test]
    fn installed_provider_binary_still_refreshes_missing_capsule_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let component = provider_component_with_capsule_metadata();
        let binary_path = tmp.path().join("bin/object-provider");
        fs::create_dir_all(binary_path.parent().unwrap()).unwrap();
        fs::write(&binary_path, b"object-provider").unwrap();
        let manifest = ComponentsManifest {
            model_catalog: None,
            external: HashMap::new(),
            capsules: HashMap::new(),
            profiles: HashMap::new(),
        };

        assert_eq!(
            component_install_state_for_name(
                &manifest,
                tmp.path(),
                "object-provider",
                &component,
                resolve_platform_info(&component, "linux-amd64")
            ),
            InstallState::Installed
        );
        assert_eq!(
            effective_component_install_state_for_name(
                &manifest,
                tmp.path(),
                "object-provider",
                &component,
                resolve_platform_info(&component, "linux-amd64"),
                "linux-amd64"
            ),
            InstallState::Stale(
                "provider capsule metadata missing from installed bundle".to_string()
            )
        );
    }

    #[test]
    fn provider_capsule_metadata_requires_capsule_manifest_even_with_cache_markers() {
        let tmp = tempfile::tempdir().unwrap();
        let component = provider_component_with_capsule_metadata();
        let install_root = tmp.path().join("capsules/object-provider");
        fs::create_dir_all(&install_root).unwrap();
        fs::write(
            install_root.join(CACHED_CID_FILE),
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi\n",
        )
        .unwrap();
        fs::write(
            install_root.join(CACHED_ARTIFACT_SHA_FILE),
            "sha256:1111111111111111111111111111111111111111111111111111111111111111\n",
        )
        .unwrap();

        match capsule_metadata_install_state_for_name(
            tmp.path(),
            "object-provider",
            &component,
            "linux-amd64",
        ) {
            Some(InstallState::Stale(reason)) => {
                assert!(reason.starts_with("capsule metadata is unreadable:"));
            }
            other => panic!("unexpected provider capsule metadata state: {other:?}"),
        }
    }

    #[tokio::test]
    async fn metadata_only_component_update_does_not_refresh_provider_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let binary_path = tmp.path().join("bin/object-provider");
        fs::create_dir_all(binary_path.parent().unwrap()).unwrap();
        fs::write(&binary_path, b"unchanged-provider-binary").unwrap();

        let old_component = provider_component_with_capsule_metadata();
        let mut new_component = old_component.clone();
        let metadata = new_component.capsule_metadata.as_mut().unwrap();
        let metadata_platform = metadata.platforms.get_mut("*").unwrap();
        metadata_platform.cid = None;
        metadata_platform.release_path = None;
        metadata_platform.checksum = Some(
            "sha256:2222222222222222222222222222222222222222222222222222222222222222".to_string(),
        );

        assert_eq!(
            component_signature(Some(&old_component), "linux-amd64"),
            component_signature(Some(&new_component), "linux-amd64")
        );
        assert!(component_capsule_metadata_changed(
            Some(&old_component),
            &new_component,
            "linux-amd64"
        ));

        let old_manifest = ComponentsManifest {
            model_catalog: None,
            external: HashMap::from([("object-provider".to_string(), old_component)]),
            capsules: HashMap::new(),
            profiles: HashMap::new(),
        };
        let new_manifest = ComponentsManifest {
            model_catalog: None,
            external: HashMap::from([("object-provider".to_string(), new_component)]),
            capsules: HashMap::new(),
            profiles: HashMap::new(),
        };
        let err = refresh_installed_components_for_update(
            tmp.path(),
            Some(&serde_json::to_vec(&old_manifest).unwrap()),
            &serde_json::to_vec(&new_manifest).unwrap(),
            "linux-amd64",
        )
        .await
        .unwrap_err();
        assert!(err
            .to_string()
            .contains("has no Carrier/content release path"));
        assert_eq!(fs::read(binary_path).unwrap(), b"unchanged-provider-binary");
    }

    #[tokio::test]
    async fn metadata_only_component_update_skips_absent_provider_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let new_component = provider_component_with_capsule_metadata();
        let mut old_component = new_component.clone();
        old_component.capsule_metadata = None;
        let old_manifest = ComponentsManifest {
            model_catalog: None,
            external: HashMap::from([("object-provider".to_string(), old_component)]),
            capsules: HashMap::new(),
            profiles: HashMap::new(),
        };
        let new_manifest = ComponentsManifest {
            model_catalog: None,
            external: HashMap::from([("object-provider".to_string(), new_component)]),
            capsules: HashMap::new(),
            profiles: HashMap::new(),
        };

        let refreshed = refresh_installed_components_for_update(
            tmp.path(),
            Some(&serde_json::to_vec(&old_manifest).unwrap()),
            &serde_json::to_vec(&new_manifest).unwrap(),
            "linux-amd64",
        )
        .await
        .unwrap();

        assert!(refreshed.is_empty());
        assert!(!tmp.path().join("capsules/object-provider").exists());
    }

    #[tokio::test]
    async fn ensure_capsule_component_rejects_missing_package_cid_when_materialization_required() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("components.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "external": {
                    "marketplace": {
                        "install_path": "capsules/marketplace",
                        "platforms": {
                            "*": {
                                "release_path": "marketplace.tar.gz",
                                "extract_path": "marketplace",
                                "install_path": "capsules/marketplace",
                                "checksum": "sha256:archive"
                            }
                        }
                    }
                },
                "capsules": {
                    "marketplace": {
                        "cid": "",
                        "sha256": "sha256:archive"
                    }
                },
                "profiles": {}
            }))
            .unwrap(),
        )
        .unwrap();

        let err = ensure_capsule_component_for_home_launch(tmp.path(), "marketplace")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("missing package CID"));
    }

    #[tokio::test]
    async fn ensure_capsule_component_rejects_registry_checksum_mismatch_before_fetch() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("components.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "external": {
                    "marketplace": {
                        "install_path": "capsules/marketplace",
                        "platforms": {
                            "*": {
                                "release_path": "marketplace.tar.gz",
                                "extract_path": "marketplace",
                                "install_path": "capsules/marketplace",
                                "checksum": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                            }
                        }
                    }
                },
                "capsules": {
                    "marketplace": {
                        "cid": "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
                        "sha256": "2222222222222222222222222222222222222222222222222222222222222222"
                    }
                },
                "profiles": {}
            }))
            .unwrap(),
        )
        .unwrap();

        let err = ensure_capsule_component_for_home_launch(tmp.path(), "marketplace")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("does not match"));
    }

    #[tokio::test]
    async fn ensure_capsule_component_rejects_platform_cid_mismatch_before_fetch() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("components.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "external": {
                    "marketplace": {
                        "install_path": "capsules/marketplace",
                        "platforms": {
                            "*": {
                                "cid": "bafybeihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
                                "release_path": "marketplace.tar.gz",
                                "extract_path": "marketplace",
                                "install_path": "capsules/marketplace",
                                "checksum": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                            }
                        }
                    }
                },
                "capsules": {
                    "marketplace": {
                        "cid": "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
                        "sha256": "1111111111111111111111111111111111111111111111111111111111111111"
                    }
                },
                "profiles": {}
            }))
            .unwrap(),
        )
        .unwrap();

        let err = ensure_capsule_component_for_home_launch(tmp.path(), "marketplace")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("platform package CID"));
    }

    #[tokio::test]
    async fn ensure_capsule_component_keeps_installed_source_capsule_without_registry_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let install_root = tmp.path().join("capsules/marketplace");
        fs::create_dir_all(&install_root).unwrap();
        fs::write(
            install_root.join("capsule.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "elastos.capsule/v1",
                "name": "marketplace",
                "version": "0.1.0",
                "description": "Marketplace",
                "author": "elastos",
                "role": "app",
                "type": "wasm",
                "entrypoint": "marketplace.wasm"
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(install_root.join("marketplace.wasm"), b"\0asm").unwrap();
        fs::write(
            tmp.path().join("components.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "external": {
                    "marketplace": {
                        "install_path": "capsules/marketplace",
                        "platforms": {
                            "*": {
                                "release_path": "marketplace.tar.gz",
                                "extract_path": "marketplace",
                                "install_path": "capsules/marketplace"
                            }
                        }
                    }
                },
                "capsules": {
                    "marketplace": {
                        "cid": "",
                        "sha256": ""
                    }
                },
                "profiles": {}
            }))
            .unwrap(),
        )
        .unwrap();

        let ensure = ensure_capsule_component_for_home_launch(tmp.path(), "marketplace")
            .await
            .unwrap();
        assert_eq!(ensure.status, "installed");
    }

    #[test]
    fn component_install_state_detects_installed_capsule_missing_runtime_entrypoint() {
        let tmp = tempfile::tempdir().unwrap();
        let install_root = tmp.path().join("capsules/marketplace");
        fs::create_dir_all(&install_root).unwrap();
        fs::write(
            install_root.join("capsule.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "elastos.capsule/v1",
                "name": "marketplace",
                "version": "0.1.0",
                "description": "Marketplace",
                "author": "elastos",
                "role": "app",
                "type": "wasm",
                "entrypoint": "marketplace.wasm"
            }))
            .unwrap(),
        )
        .unwrap();
        let mut platforms = HashMap::new();
        platforms.insert(
            "*".to_string(),
            PlatformInfo {
                url: None,
                cid: None,
                release_path: Some("marketplace.tar.gz".to_string()),
                checksum: None,
                extract_path: Some("marketplace".to_string()),
                install_path: Some("capsules/marketplace".to_string()),
                binary_path: None,
                strategy: None,
                source: None,
                note: None,
                size: None,
            },
        );
        let component = Component {
            version: None,
            install_path: Some("capsules/marketplace".to_string()),
            repository: None,
            size_mb: None,
            description: None,
            provider_runtime: None,
            capsule_metadata: None,
            platforms,
        };
        let manifest = ComponentsManifest {
            model_catalog: None,
            external: HashMap::new(),
            capsules: HashMap::new(),
            profiles: HashMap::new(),
        };

        assert_eq!(
            component_install_state_for_name(
                &manifest,
                tmp.path(),
                "marketplace",
                &component,
                component.platforms.get("*"),
            ),
            InstallState::Stale(
                "capsule entrypoint missing from installed bundle: marketplace.wasm".to_string()
            )
        );
    }

    #[test]
    fn capsule_component_release_identity_accepts_direct_asset_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("components.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "external": {
                    "marketplace": {
                        "install_path": "capsules/marketplace",
                        "platforms": {
                            "*": {
                                "cid": "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
                                "release_path": "marketplace.tar.gz",
                                "extract_path": "marketplace",
                                "install_path": "capsules/marketplace",
                                "checksum": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                            }
                        }
                    }
                },
                "capsules": {},
                "profiles": {}
            }))
            .unwrap(),
        )
        .unwrap();

        assert!(capsule_component_has_release_identity(
            tmp.path(),
            "marketplace"
        ));
    }

    #[test]
    fn validate_capsule_package_identity_accepts_direct_asset_metadata() {
        let platform_info = PlatformInfo {
            url: None,
            cid: Some("bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi".to_string()),
            release_path: Some("marketplace.tar.gz".to_string()),
            checksum: Some(
                "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                    .to_string(),
            ),
            extract_path: Some("marketplace".to_string()),
            install_path: Some("capsules/marketplace".to_string()),
            binary_path: None,
            strategy: None,
            source: None,
            note: None,
            size: None,
        };

        validate_capsule_package_identity("marketplace", None, &platform_info).unwrap();
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
        let platform = detect_platform();
        let bin_dir = data_dir.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let install_path = bin_dir.join("shell");
        fs::write(&install_path, b"shell-binary").unwrap();
        fs::write(
            data_dir.join("components.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "external": {
                    "shell": {
                        "install_path": "bin/shell",
                        "platforms": {
                            platform: {
                                "checksum": "",
                                "url": "https://example.invalid/shell"
                            }
                        }
                    }
                },
                "capsules": {},
                "profiles": {}
            }))
            .unwrap(),
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
        let platform = detect_platform();
        let bin_dir = data_dir.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let install_path = bin_dir.join("shell");
        let bytes = b"shell-binary";
        fs::write(&install_path, bytes).unwrap();
        let checksum = format!("sha256:{}", hex::encode(sha2::Sha256::digest(bytes)));
        fs::write(
            data_dir.join("components.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "external": {
                    "shell": {
                        "install_path": "bin/shell",
                        "platforms": {
                            platform.clone(): {
                                "checksum": checksum.clone(),
                                "url": "https://example.invalid/shell"
                            }
                        }
                    }
                },
                "capsules": {},
                "profiles": {}
            }))
            .unwrap(),
        )
        .unwrap();

        let result = verify_installed_component_binary(data_dir, "shell", &install_path).unwrap();
        assert_eq!(result, checksum);
    }

    #[test]
    fn test_write_installed_manifest_stamps_local_copy_checksum() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let platform = detect_platform();
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
                        platform.clone(): {
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

        let stamped = write_installed_manifest(data_dir, &manifest, &platform).unwrap();
        assert_eq!(stamped, vec!["vmlinux".to_string()]);

        let result = verify_installed_component_binary(data_dir, "vmlinux", &install_path).unwrap();
        assert!(result.starts_with("sha256:"));
    }

    #[cfg(unix)]
    #[test]
    fn test_local_copy_permissions_preserve_executability() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source-bin");
        let dest = tmp.path().join("dest-bin");
        fs::write(&source, b"binary").unwrap();
        fs::write(&dest, b"binary").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();

        set_local_copy_permissions(&source, &dest);

        let mode = fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
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
                binary_path: None,
                strategy: Some("local-copy".to_string()),
                source: Some(source_path.to_string_lossy().to_string()),
                note: None,
                size: Some(b"same-size-data".len() as u64),
            },
        );
        let comp = Component {
            version: None,
            install_path: Some("bin/vmlinux".to_string()),
            repository: None,
            size_mb: None,
            description: None,
            provider_runtime: None,
            capsule_metadata: None,
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
            binary_path: None,
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

    #[test]
    fn release_component_admission_resolves_aliases_and_checks_complete_hash_syntax() {
        for (info, accepted) in [
            (
                serde_json::json!({"checksum":format!("sha256:{}", "A".repeat(64))}),
                true,
            ),
            (
                serde_json::json!({"checksum":format!("sha512:{}", "b".repeat(128)), "strategy":"prebuilt"}),
                true,
            ),
            (serde_json::json!({"checksum":"sha256:bad"}), false),
            (
                serde_json::json!({"checksum":format!("sha256:{}", "g".repeat(64))}),
                false,
            ),
            (
                serde_json::json!({"checksum":format!("sha256:{}\n", "a".repeat(64))}),
                false,
            ),
            (serde_json::json!({}), false),
            (serde_json::json!({"strategy":"unknown"}), false),
        ] {
            let manifest = serde_json::from_value(serde_json::json!({
                "external":{"fixture":{"platforms":{"aarch64-linux":info}}}, "profiles":{}
            }))
            .unwrap();
            assert_eq!(
                admit_release_components(&manifest, "linux-arm64").is_ok(),
                accepted
            );
            assert!(admit_release_components(&manifest, "darwin-arm64").is_ok());
        }
    }

    #[test]
    fn checkout_manifest_prepared_for_release_admits_every_platform() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../components.json");
        let mut manifest = load_manifest_from_path(&path).unwrap();
        // Release preparation stamps built assets and replaces source-build recipes.
        // Keep other strategies and existing checksums to check the real inventory.
        for component in manifest.external.values_mut() {
            for info in component.platforms.values_mut().chain(
                component
                    .capsule_metadata
                    .iter_mut()
                    .flat_map(|metadata| metadata.platforms.values_mut()),
            ) {
                if info.strategy.as_deref() == Some("source-build") {
                    info.strategy = None;
                }
                if info.checksum.as_deref().is_none_or(str::is_empty) {
                    info.checksum = Some(format!("sha256:{}", "a".repeat(64)));
                }
            }
        }
        for platform in ["darwin-arm64", "linux-amd64", "linux-arm64"] {
            admit_release_components(&manifest, platform)
                .unwrap_or_else(|error| panic!("{platform}: {error:#}"));
        }
        // The signer binds every pinned component to a release file, so no
        // descriptor may name bytes only by CID.
        for (name, component) in &manifest.external {
            for (platform, info) in &component.platforms {
                assert!(
                    info.cid.as_deref().is_none_or(str::is_empty)
                        || info.release_path.as_deref().is_some_and(|p| !p.is_empty()),
                    "{name} {platform}: CID-only descriptor cannot be signed"
                );
            }
        }
    }

    #[test]
    fn verify_checksum_requires_release_artifact_checksum() {
        let info = PlatformInfo {
            url: None,
            cid: None,
            release_path: Some("shell-linux-amd64".to_string()),
            checksum: None,
            extract_path: None,
            install_path: Some("bin/shell".to_string()),
            binary_path: None,
            strategy: None,
            source: None,
            note: None,
            size: None,
        };

        let err = verify_checksum("shell", b"shell-bytes", &info).unwrap_err();
        assert!(err.to_string().contains("missing checksum"));
    }

    #[test]
    fn verify_checksum_accepts_stamped_release_artifact_checksum() {
        let bytes = b"shell-bytes";
        let info = PlatformInfo {
            url: None,
            cid: None,
            release_path: Some("shell-linux-amd64".to_string()),
            checksum: Some(format!(
                "sha256:{}",
                hex::encode(sha2::Sha256::digest(bytes))
            )),
            extract_path: None,
            install_path: Some("bin/shell".to_string()),
            binary_path: None,
            strategy: None,
            source: None,
            note: None,
            size: None,
        };

        verify_checksum("shell", bytes, &info).unwrap();
    }

    #[test]
    fn release_artifact_checksum_requirement_keeps_dev_escape_hatches() {
        for strategy in ["source-build", "local-copy"] {
            let info = PlatformInfo {
                url: None,
                cid: None,
                release_path: Some("shell-linux-amd64".to_string()),
                checksum: None,
                extract_path: None,
                install_path: Some("bin/shell".to_string()),
                binary_path: None,
                strategy: Some(strategy.to_string()),
                source: None,
                note: None,
                size: None,
            };
            assert!(required_release_artifact_checksum("shell", &info)
                .unwrap()
                .is_none());
        }
    }

    #[tokio::test]
    async fn download_component_rejects_unstamped_release_artifact_before_fetch() {
        let tmp = tempfile::tempdir().unwrap();
        let info = PlatformInfo {
            url: None,
            cid: None,
            release_path: Some("shell-linux-amd64".to_string()),
            checksum: None,
            extract_path: None,
            install_path: Some("bin/shell".to_string()),
            binary_path: None,
            strategy: None,
            source: None,
            note: None,
            size: None,
        };

        let err = download_component(
            tmp.path(),
            "shell",
            "elastos://artifact/shell-linux-amd64",
            &info,
            &tmp.path().join("bin/shell"),
            &[],
            FirstPartyCarrierContext::Setup,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("missing checksum"));
    }

    #[tokio::test]
    async fn download_component_rejects_url_only_before_fetch() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        for identity in [None, Some(""), Some(" ")] {
            let tmp = tempfile::tempdir().unwrap();
            let source: TrustedSource = serde_json::from_value(serde_json::json!({
                "name": "fixture",
                "publisher_dids": [elastos_identity::derive_did(&[217; 32]).1]
            }))
            .unwrap();
            let mut sources = TrustedSourcesConfig::empty();
            sources.upsert_source(source);
            save_trusted_sources(tmp.path(), &sources).unwrap();

            let requests = Arc::new(AtomicUsize::new(0));
            let observed = requests.clone();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/artifact", listener.local_addr().unwrap());
            let app = axum::Router::new().route(
                "/artifact",
                axum::routing::get(move || {
                    observed.fetch_add(1, Ordering::SeqCst);
                    async { "upstream fixture" }
                }),
            );
            let serving = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let info: PlatformInfo = serde_json::from_value(serde_json::json!({
                "url": url,
                "release_path": identity,
                "cid": identity,
                "checksum": format!("sha256:{:x}", sha2::Sha256::digest(b"upstream fixture"))
            }))
            .unwrap();
            let dest = tmp.path().join("bin/kubo");
            let result = download_component(
                tmp.path(),
                "kubo",
                &url,
                &info,
                &dest,
                &[],
                FirstPartyCarrierContext::Setup,
            )
            .await;
            serving.abort();
            let _ = serving.await;

            let error = result.unwrap_err().to_string();
            assert!(error.contains("kubo"), "{error}");
            assert!(
                error.contains("must come from the signed release"),
                "{error}"
            );
            assert_eq!(requests.load(Ordering::SeqCst), 0);
            assert!(!dest.parent().unwrap().exists());
        }
    }

    fn signed_setup_fixture_envelope(
        payload: serde_json::Value,
        domain: &str,
        signer: u8,
    ) -> Vec<u8> {
        let key = ed25519_dalek::SigningKey::from_bytes(&[signer; 32]);
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &key,
            domain,
            &serde_json::to_vec(&payload).unwrap(),
        );
        serde_json::to_vec(&serde_json::json!({
            "payload": payload, "signature": signature, "signer_did": signer_did,
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn installed_setup_bootstraps_carrier_metadata_before_writes() {
        signed_setup_carrier_fixture("fresh", false).await;
    }

    #[tokio::test]
    async fn installed_setup_refuses_tampered_components_without_writes() {
        for existing in [false, true] {
            signed_setup_carrier_fixture("component hash", existing).await;
        }
    }

    #[tokio::test]
    async fn installed_setup_refuses_tampered_catalog_without_writes() {
        for existing in [false, true] {
            signed_setup_carrier_fixture("catalog hash", existing).await;
        }
    }

    #[tokio::test]
    async fn installed_setup_refuses_oversized_catalog_before_body_without_writes() {
        for existing in [false, true] {
            signed_setup_carrier_fixture("catalog size", existing).await;
        }
    }

    #[tokio::test]
    async fn installed_setup_refuses_wrong_signers_without_writes() {
        for case in ["catalog signer", "head signer", "release signer"] {
            for existing in [false, true] {
                signed_setup_carrier_fixture(case, existing).await;
            }
        }
    }

    #[tokio::test]
    async fn installed_setup_refuses_pending_or_busy_writer_before_fetch() {
        for case in ["pending journal", "writer busy"] {
            signed_setup_carrier_fixture(case, false).await;
        }
    }

    pub(crate) async fn signed_engine_fixture(
        data: &Path,
        fault: &str,
    ) -> (iroh::Endpoint, tokio::task::JoinHandle<usize>, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        use tokio::io::AsyncBufReadExt;

        let data = data.canonicalize().unwrap();
        let engine = fs::read("/usr/bin/true").unwrap();
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut archive = tar::Builder::new(encoder);
        for name in ["llama-server", "libfixture.so"] {
            let mut header = tar::Header::new_gnu();
            header.set_size(engine.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            archive
                .append_data(
                    &mut header,
                    format!("llama-fixture/{name}"),
                    engine.as_slice(),
                )
                .unwrap();
        }
        let mut bytes = archive.into_inner().unwrap().finish().unwrap();
        let platform = detect_platform();
        let relative = format!("libexec/llama.cpp/fixture/{platform}");
        let bundle = data.join(&relative);
        let mut components: serde_json::Value =
            serde_json::from_slice(&fs::read(data.join("components.json")).unwrap()).unwrap();
        components["external"]["llama-server"] = serde_json::json!({
            "version":"fixture", "platforms":{platform:{
                "release_path": if fault == "url-only" { None } else { Some("llama-fixture.tar.gz") },
                "url":"http://127.0.0.1:1/refused-upstream",
                "checksum":format!("sha256:{:x}", sha2::Sha256::digest(&bytes)),
                "size":bytes.len(), "extract_path":"llama-fixture", "binary_path":"llama-server",
                "install_path":relative
            }}
        });
        let mut components = serde_json::to_vec(&components).unwrap();
        let binary = data.join("elastos");
        fs::write(&binary, b"signed fixture runtime").unwrap();
        let descriptor = |bytes: &[u8]| {
            serde_json::json!({
                "cid":catalog_head_cid(bytes).unwrap(),
                "sha256":hex::encode(sha2::Sha256::digest(bytes)), "size":bytes.len()
            })
        };
        let release = signed_setup_fixture_envelope(
            serde_json::json!({
                "schema":"elastos.release/v1", "version":"0.7.1", "channel":"stable",
                "platforms":{(crate::update::detect_release_platform()):{
                    "binary":descriptor(b"signed fixture runtime"),
                    "components":descriptor(&components)
                }}
            }),
            "elastos.release.v1",
            217,
        );
        let head = signed_setup_fixture_envelope(
            serde_json::json!({
                "schema":"elastos.release.head/v1", "version":"0.7.1", "channel":"stable",
                "latest_release_cid":catalog_head_cid(&release).unwrap(),
                "release_sha256":hex::encode(sha2::Sha256::digest(&release))
            }),
            "elastos.release.head.v1",
            217,
        );
        fs::create_dir(data.join("installation")).unwrap();
        fs::set_permissions(data.join("installation"), fs::Permissions::from_mode(0o700)).unwrap();
        for (name, bytes) in [("release-head.json", &head), ("release.json", &release)] {
            let path = data.join("installation").join(name);
            fs::write(&path, bytes).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let server = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .clear_ip_transports()
            .portmapper_config(iroh::endpoint::PortmapperConfig::Disabled)
            .alpns(vec![b"elastos/carrier/1".to_vec()])
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
            .unwrap()
            .bind()
            .await
            .unwrap();
        let address = crate::carrier::tests::wait_for_direct_endpoint_addr(&server).await;
        let source: TrustedSource = serde_json::from_value(serde_json::json!({
            "name":"fixture",
            "publisher_dids":[crate::crypto::encode_signing_key_did(
                &ed25519_dalek::SigningKey::from_bytes(&[217; 32]),
            )],
            "install_path":binary, "installed_version":"0.7.1",
            "head_cid":catalog_head_cid(&head).unwrap(), "publisher_node_id":server.id().to_string(),
            "connect_ticket":crate::carrier::tests::encode_ticket_for(address)
        }))
        .unwrap();
        let mut sources = TrustedSourcesConfig::empty();
        sources.upsert_source(source);
        save_trusted_sources(&data, &sources).unwrap();
        fs::write(
            data.join("config.toml"),
            "carrier_bind_addr = \"127.0.0.1:0\"\n",
        )
        .unwrap();
        if fault == "manifest" {
            components.push(b'\n');
        }
        fs::write(data.join("components.json"), components).unwrap();
        if fault == "tampered" {
            bytes.push(0);
        }
        if fault == "pending" {
            fs::write(
                data.join(".elastos.update-journal.json"),
                b"retained recovery",
            )
            .unwrap();
        }
        let endpoint = server.clone();
        let serving = tokio::spawn(async move {
            let mut requests = 0;
            while let Some(incoming) = endpoint.accept().await {
                let Ok(connection) = incoming.await else {
                    break;
                };
                while let Ok((mut send, recv)) = connection.accept_bi().await {
                    let mut request = String::new();
                    tokio::io::BufReader::new(recv)
                        .read_line(&mut request)
                        .await
                        .unwrap();
                    let request: serde_json::Value = serde_json::from_str(&request).unwrap();
                    assert_eq!(request["path"], "llama-fixture.tar.gz");
                    assert!(crate::install_transaction::InstallationGuard::acquire(&data).is_err());
                    requests += 1;
                    send.write_all(&(bytes.len() as u64).to_be_bytes())
                        .await
                        .unwrap();
                    send.write_all(&bytes).await.unwrap();
                    send.finish().unwrap();
                }
            }
            requests
        });
        (server, serving, bundle)
    }

    #[tokio::test]
    async fn model_capsules_refuse_legacy_archive_download_before_fetch() {
        let root = tempfile::tempdir().unwrap();
        let info: PlatformInfo = serde_json::from_value(serde_json::json!({
            "release_path":"model-bonsai-8b-q1", "checksum":format!("sha256:{}", "a".repeat(64)),
            "extract_path":"model-bonsai-8b-q1"
        }))
        .unwrap();
        for (name, path) in [
            ("model-bonsai-8b-q1", "capsules/model-bonsai-8b-q1"),
            ("weights", "models/weights.gguf"),
        ] {
            let dest = root.path().join(path);
            let result = download_component(
                root.path(),
                name,
                "http://127.0.0.1:1/refused",
                &info,
                &dest,
                &[],
                FirstPartyCarrierContext::Runtime,
            )
            .await;
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("signed directory preparation"));
            assert!(!dest.exists());
        }
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn on_demand_engine_refuses_unsigned_tampered_or_unbound_inputs_without_activation() {
        for (fault, expected, fetches) in [
            ("url-only", "URL-only engines are refused", 0),
            (
                "manifest",
                "Installed artifact differs from its signed checksum",
                0,
            ),
            ("pending", "A signed update requires recovery", 0),
            ("tampered", "Checksum mismatch for llama-server", 1),
        ] {
            let root = tempfile::tempdir().unwrap();
            fs::write(
                root.path().join("components.json"),
                br#"{"external":{},"profiles":{}}"#,
            )
            .unwrap();
            let (server, serving, bundle) = signed_engine_fixture(root.path(), fault).await;
            let manifest = fs::read(root.path().join("components.json")).unwrap();
            let Err(error) = ensure_local_model_engine(root.path(), &manifest).await else {
                panic!("{fault}: engine acquisition must be refused");
            };
            assert!(
                format!("{error:#}").contains(expected),
                "{fault}: {error:#}"
            );
            assert!(
                !bundle.exists(),
                "{fault}: engine bundle must not be activated"
            );
            server.close().await;
            assert_eq!(serving.await.unwrap(), fetches, "{fault}");
        }
    }

    #[tokio::test]
    async fn on_demand_engine_fetches_signed_bundle_over_carrier_then_reuses_it() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("components.json"),
            br#"{"external":{},"profiles":{}}"#,
        )
        .unwrap();
        let (server, serving, bundle) = signed_engine_fixture(root.path(), "none").await;
        let manifest = fs::read(root.path().join("components.json")).unwrap();
        let first = ensure_local_model_engine(root.path(), &manifest).await;
        assert!(first.is_ok(), "{:#}", first.err().unwrap());
        assert!(
            bundle.join("llama-server").is_file(),
            "engine binary extracted"
        );
        assert!(
            bundle.join("libfixture.so").is_file(),
            "engine library extracted"
        );
        // An installed engine is verified and reused; Carrier is not asked again.
        let second = ensure_local_model_engine(root.path(), &manifest).await;
        assert!(second.is_ok(), "{:#}", second.err().unwrap());
        server.close().await;
        assert_eq!(serving.await.unwrap(), 1);
    }

    fn signed_setup_snapshot(root: &Path) -> BTreeMap<PathBuf, (u32, Option<Vec<u8>>)> {
        use std::os::unix::fs::PermissionsExt;

        let mut snapshot = BTreeMap::new();
        for entry in fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            let bytes = if metadata.is_dir() {
                snapshot.extend(signed_setup_snapshot(&path));
                None
            } else {
                Some(fs::read(&path).unwrap())
            };
            snapshot.insert(path, (metadata.permissions().mode(), bytes));
        }
        snapshot
    }

    async fn signed_setup_carrier_fixture(case: &str, existing_metadata: bool) {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        use tokio::io::AsyncBufReadExt;

        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().canonicalize().unwrap();
        let binary = data.join("elastos");
        fs::write(&binary, b"signed fixture runtime").unwrap();
        let publisher = crate::crypto::encode_signing_key_did(
            &ed25519_dalek::SigningKey::from_bytes(&[217; 32]),
        );
        let catalog = signed_setup_fixture_envelope(
            crate::api::capsule_inventory::tests::model_catalog_fixture(),
            "elastos.model.catalog.v1",
            if case == "catalog signer" { 218 } else { 217 },
        );
        let components = format!(
            "{{\n \"publisher_extension\":true, \"external\":{{\"effect\":{{
            \"install_path\":\"bin/effect\",\"platforms\":{{\"*\":{{
                \"release_path\":\"effect\", \"cid\":\"{}\", \"checksum\":\"sha256:{:x}\"
            }}}}
        }}}},\"profiles\":{{\"home\":{{\"components\":[\"effect\"]}}}},
        \"model_catalog\":{{\"head_cid\":\"{}\",\"publisher_dids\":[\"{}\"]}}\n}}\n",
            catalog_head_cid(b"Carrier component").unwrap(),
            sha2::Sha256::digest(b"Carrier component"),
            catalog_head_cid(&catalog).unwrap(),
            publisher
        )
        .into_bytes();
        let descriptor = |bytes: &[u8]| {
            serde_json::json!({
                "cid": catalog_head_cid(bytes).unwrap(),
                "sha256": hex::encode(sha2::Sha256::digest(bytes)), "size": bytes.len()
            })
        };
        let release = signed_setup_fixture_envelope(
            serde_json::json!({
                "schema":"elastos.release/v1", "version":"0.7.1", "channel":"stable",
                "platforms":{(crate::update::detect_release_platform()):{
                    "binary":descriptor(b"signed fixture runtime"), "components":descriptor(&components)
                }}
            }),
            "elastos.release.v1",
            if case == "release signer" { 218 } else { 217 },
        );
        let head = signed_setup_fixture_envelope(
            serde_json::json!({
                "schema":"elastos.release.head/v1", "version":"0.7.1", "channel":"stable",
                "latest_release_cid":catalog_head_cid(&release).unwrap(),
                "release_sha256":hex::encode(sha2::Sha256::digest(&release))
            }),
            "elastos.release.head.v1",
            if case == "head signer" { 218 } else { 217 },
        );
        fs::create_dir(data.join("installation")).unwrap();
        fs::set_permissions(data.join("installation"), fs::Permissions::from_mode(0o700)).unwrap();
        for (name, bytes) in [("release-head.json", &head), ("release.json", &release)] {
            fs::write(data.join("installation").join(name), bytes).unwrap();
            fs::set_permissions(
                data.join("installation").join(name),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        let server = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .clear_ip_transports()
            .portmapper_config(iroh::endpoint::PortmapperConfig::Disabled)
            .alpns(vec![b"elastos/carrier/1".to_vec()])
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
            .unwrap()
            .bind()
            .await
            .unwrap();
        let address = crate::carrier::tests::wait_for_direct_endpoint_addr(&server).await;
        let source: TrustedSource = serde_json::from_value(serde_json::json!({
            "name":"fixture", "publisher_dids":[publisher], "install_path":binary,
            "installed_version":"0.7.1", "head_cid":catalog_head_cid(&head).unwrap(),
            "publisher_node_id":server.id().to_string(),
            "connect_ticket":crate::carrier::tests::encode_ticket_for(address)
        }))
        .unwrap();
        let mut sources = TrustedSourcesConfig::empty();
        sources.upsert_source(source);
        save_trusted_sources(&data, &sources).unwrap();
        fs::write(
            data.join("config.toml"),
            "carrier_bind_addr = \"127.0.0.1:0\"\n",
        )
        .unwrap();
        if existing_metadata {
            fs::write(data.join("components.json"), b"previous components").unwrap();
            fs::write(data.join(MODEL_CATALOG_FILE), b"previous catalog").unwrap();
        }
        if case == "pending journal" {
            fs::write(
                data.join(".elastos.update-journal.json"),
                b"retained recovery",
            )
            .unwrap();
        }
        // Establish the guard file before snapshotting; refusal must not change
        // anything else, including when neither metadata file exists yet.
        let mut held = Some(crate::install_transaction::InstallationGuard::acquire(&data).unwrap());
        if case != "writer busy" {
            drop(held.take());
        }
        let before = signed_setup_snapshot(&data);
        let mut served_components = components.clone();
        if case == "component hash" {
            served_components[1] = b' ';
        }
        let mut served_catalog = catalog.clone();
        if case == "catalog hash" {
            served_catalog.push(b'\n');
        }
        let files = Arc::new(HashMap::from([
            (
                format!(
                    "components-{}.json",
                    crate::update::detect_release_platform()
                ),
                served_components,
            ),
            (MODEL_CATALOG_FILE.to_owned(), served_catalog),
            ("effect".to_owned(), b"Carrier component".to_vec()),
        ]));
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        let endpoint = server.clone();
        let writer_parent = data.clone();
        let oversized_catalog = case == "catalog size";
        let serving = tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let Ok(connection) = incoming.await else {
                    break;
                };
                while let Ok((mut send, recv)) = connection.accept_bi().await {
                    let mut request = String::new();
                    tokio::io::BufReader::new(recv)
                        .read_line(&mut request)
                        .await
                        .unwrap();
                    let request: serde_json::Value = serde_json::from_str(&request).unwrap();
                    assert!(
                        crate::install_transaction::InstallationGuard::acquire(&writer_parent)
                            .is_err(),
                        "setup must retain the writer across Carrier reads"
                    );
                    if request["path"] != "effect" {
                        assert_eq!(signed_setup_snapshot(&writer_parent), before);
                    }
                    observed.fetch_add(1, Ordering::SeqCst);
                    if oversized_catalog && request["path"] == MODEL_CATALOG_FILE {
                        // Keep the body open and unsent: refusal must use the header alone.
                        send.write_all(&((MAX_MODEL_CATALOG_BYTES as u64) + 1).to_be_bytes())
                            .await
                            .unwrap();
                        let _ = send.stopped().await;
                        continue;
                    }
                    let bytes = &files[request["path"].as_str().unwrap()];
                    send.write_all(&(bytes.len() as u64).to_be_bytes())
                        .await
                        .unwrap();
                    send.write_all(bytes).await.unwrap();
                    send.finish().unwrap();
                }
            }
            before
        });
        let result =
            run_with_data_dir(data.clone(), None, vec![], vec![], false, false, None).await;
        server.close().await;
        let before = serving.await.unwrap();
        drop(held);
        if case == "fresh" {
            result.unwrap();
            assert_eq!(fs::read(data.join("components.json")).unwrap(), components);
            assert_eq!(fs::read(data.join(MODEL_CATALOG_FILE)).unwrap(), catalog);
            assert_eq!(
                fs::read(data.join("bin/effect")).unwrap(),
                b"Carrier component"
            );
            assert_eq!(requests.load(Ordering::SeqCst), 3);
        } else {
            assert!(result.is_err(), "{case}");
            assert_eq!(signed_setup_snapshot(&data), before, "{case}");
            if case == "catalog size" {
                let error = format!("{:#}", result.unwrap_err());
                assert!(
                    error.contains(&format!("exceeds its {MAX_MODEL_CATALOG_BYTES}-byte bound")),
                    "{error}"
                );
            }
            let expected_requests = match case {
                "component hash" => 1,
                "catalog hash" | "catalog signer" | "catalog size" => 2,
                "head signer" | "release signer" | "pending journal" | "writer busy" => 0,
                _ => panic!("unknown refusal case"),
            };
            assert_eq!(requests.load(Ordering::SeqCst), expected_requests, "{case}");
        }
    }

    pub(crate) async fn carrier_component_download_fixture(
        data_dir: &Path,
        release_path: &'static str,
        bytes: Vec<u8>,
    ) -> (iroh::Endpoint, tokio::task::JoinHandle<()>, PlatformInfo) {
        use tokio::io::AsyncBufReadExt;

        let server = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .clear_ip_transports()
            .portmapper_config(iroh::endpoint::PortmapperConfig::Disabled)
            .alpns(vec![b"elastos/carrier/1".to_vec()])
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
            .unwrap()
            .bind()
            .await
            .unwrap();
        let address = crate::carrier::tests::wait_for_direct_endpoint_addr(&server).await;
        let source: TrustedSource = serde_json::from_value(serde_json::json!({
            "name": "fixture",
            "publisher_dids": [elastos_identity::derive_did(&[217; 32]).1],
            "connect_ticket": crate::carrier::tests::encode_ticket_for(address),
            "publisher_node_id": server.id().to_string()
        }))
        .unwrap();
        let mut sources = TrustedSourcesConfig::empty();
        sources.upsert_source(source);
        save_trusted_sources(data_dir, &sources).unwrap();
        fs::write(
            data_dir.join("config.toml"),
            "carrier_bind_addr = \"127.0.0.1:0\"\n",
        )
        .unwrap();

        let digest = sha2::Sha256::digest(&bytes);
        let cid = cid::Cid::new_v1(
            0x55,
            cid::multihash::Multihash::<64>::wrap(0x12, &digest).unwrap(),
        );
        let info: PlatformInfo = serde_json::from_value(serde_json::json!({
            "url": "http://127.0.0.1:9/unused",
            "release_path": release_path,
            "cid": cid.to_string(),
            "checksum": format!("sha256:{digest:x}")
        }))
        .unwrap();
        let endpoint = server.clone();
        let serving = tokio::spawn(async move {
            let connection = endpoint.accept().await.unwrap().await.unwrap();
            let (mut send, recv) = connection.accept_bi().await.unwrap();
            let mut request = String::new();
            tokio::io::BufReader::new(recv)
                .read_line(&mut request)
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&request).unwrap()["path"],
                release_path
            );
            send.write_all(&(bytes.len() as u64).to_be_bytes())
                .await
                .unwrap();
            send.write_all(&bytes).await.unwrap();
            send.finish().unwrap();
            connection.closed().await;
        });
        (server, serving, info)
    }

    #[tokio::test]
    async fn download_component_accepts_release_path_and_cid_over_carrier() {
        let tmp = tempfile::tempdir().unwrap();
        let (server, serving, info) =
            carrier_component_download_fixture(tmp.path(), "artifact", b"carrier fixture".to_vec())
                .await;
        let dest = tmp.path().join("bin/kubo");
        let result = download_component(
            tmp.path(),
            "kubo",
            info.url.as_deref().unwrap(),
            &info,
            &dest,
            &[],
            FirstPartyCarrierContext::Setup,
        )
        .await;
        server.close().await;
        serving.await.unwrap();

        result.unwrap();
        assert_eq!(fs::read(dest).unwrap(), b"carrier fixture");
    }

    #[tokio::test]
    async fn download_component_rejects_cid_checksum_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let (server, serving, mut info) =
            carrier_component_download_fixture(tmp.path(), "artifact", b"carrier fixture".to_vec())
                .await;
        info.checksum = Some(format!(
            "sha256:{:x}",
            sha2::Sha256::digest(b"different component")
        ));
        let dest = tmp.path().join("bin/kubo");
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(&dest, b"installed fixture").unwrap();
        let result = download_component(
            tmp.path(),
            "kubo",
            info.url.as_deref().unwrap(),
            &info,
            &dest,
            &[],
            FirstPartyCarrierContext::Setup,
        )
        .await;
        server.close().await;
        serving.await.unwrap();

        let error = result.unwrap_err().to_string();
        assert!(error.contains("Checksum mismatch for kubo"), "{error}");
        assert_eq!(fs::read(dest).unwrap(), b"installed fixture");
    }

    #[tokio::test]
    async fn download_component_requires_explicit_cid_gateway_and_verifies_checksum() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        let bytes = b"gateway fixture";
        let digest = sha2::Sha256::digest(bytes);
        let cid = cid::Cid::new_v1(
            0x55,
            cid::multihash::Multihash::<64>::wrap(0x12, &digest).unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway = format!("http://{}", listener.local_addr().unwrap());
        let url = format!("{gateway}/unused");
        let app = axum::Router::new().route(
            &format!("/ipfs/{cid}"),
            axum::routing::get(move || {
                observed.fetch_add(1, Ordering::SeqCst);
                async { "gateway fixture" }
            }),
        );
        let serving = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut info: PlatformInfo = serde_json::from_value(serde_json::json!({
            "url": url,
            "cid": cid.to_string(),
            "checksum": format!("sha256:{digest:x}")
        }))
        .unwrap();
        let dest = tmp.path().join("bin/operator-tool");
        let error = download_component(
            tmp.path(),
            "operator-tool",
            &url,
            &info,
            &dest,
            &build_gateway_list(tmp.path()),
            FirstPartyCarrierContext::Setup,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("No configured fetch path"));
        assert_eq!(requests.load(Ordering::SeqCst), 0);
        assert!(!dest.parent().unwrap().exists());

        let source: TrustedSource = serde_json::from_value(serde_json::json!({
            "name": "fixture",
            "publisher_dids": [elastos_identity::derive_did(&[217; 32]).1],
            "gateways": [gateway]
        }))
        .unwrap();
        let mut sources = TrustedSourcesConfig::empty();
        sources.upsert_source(source);
        save_trusted_sources(tmp.path(), &sources).unwrap();
        let gateways = build_gateway_list(tmp.path());
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(&dest, b"installed fixture").unwrap();
        info.checksum = Some(format!(
            "sha256:{:x}",
            sha2::Sha256::digest(b"different component")
        ));
        let error = download_component(
            tmp.path(),
            "operator-tool",
            &url,
            &info,
            &dest,
            &gateways,
            FirstPartyCarrierContext::Setup,
        )
        .await
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Checksum mismatch for operator-tool"),
            "{error}"
        );
        assert_eq!(fs::read(&dest).unwrap(), b"installed fixture");

        info.checksum = Some(format!("sha256:{digest:x}"));
        let result = download_component(
            tmp.path(),
            "operator-tool",
            &url,
            &info,
            &dest,
            &gateways,
            FirstPartyCarrierContext::Setup,
        )
        .await;
        serving.abort();
        let _ = serving.await;

        result.unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert_eq!(fs::read(dest).unwrap(), bytes);
    }

    #[tokio::test]
    async fn download_component_rejects_blank_release_path_with_cid_before_fetch() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new().fallback(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            async { "gateway fixture" }
        });
        let serving = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let source: TrustedSource = serde_json::from_value(serde_json::json!({
            "name": "fixture", "publisher_dids": [elastos_identity::derive_did(&[217; 32]).1],
            "gateways": [gateway]
        }))
        .unwrap();
        let mut sources = TrustedSourcesConfig::empty();
        sources.upsert_source(source);
        save_trusted_sources(tmp.path(), &sources).unwrap();
        let dest = tmp.path().join("bin/kubo");
        for blank in ["", " ", "\t\n"] {
            let info: PlatformInfo = serde_json::from_value(serde_json::json!({
                "release_path": blank,
                "cid": catalog_head_cid(b"gateway fixture").unwrap(),
                "checksum": format!("sha256:{:x}", sha2::Sha256::digest(b"gateway fixture"))
            }))
            .unwrap();
            let error = download_component(
                tmp.path(),
                "kubo",
                "unused",
                &info,
                &dest,
                &build_gateway_list(tmp.path()),
                FirstPartyCarrierContext::Setup,
            )
            .await
            .unwrap_err();
            assert!(
                error.to_string().contains("release_path is blank"),
                "{error}"
            );
            assert!(!dest.parent().unwrap().exists());
        }
        serving.abort();
        let _ = serving.await;
        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn update_refresh_preserves_manifest_bytes_without_asset_changes() {
        let new_bytes = b"{ \n \"profiles\": {}, \"external\":{}, \"capsules\": {}, \"publisher_extension\":true }\n";
        let old_bytes = br#"{"external":{},"capsules":{},"profiles":{}}"#;
        for old in [None, Some(old_bytes.as_slice()), Some(new_bytes.as_slice())] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("components.json");
            fs::write(&path, new_bytes).unwrap();
            let expected_hash = sha2::Sha256::digest(new_bytes);
            let refreshed =
                refresh_installed_components_for_update(tmp.path(), old, new_bytes, "x86_64-linux")
                    .await
                    .unwrap();
            assert!(refreshed.is_empty());
            let installed = fs::read(&path).unwrap();
            assert_eq!(sha2::Sha256::digest(&installed), expected_hash);
            assert_eq!(installed, new_bytes);
        }
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

        // The updater installs the publisher's bytes before refreshing assets.
        let mut new_bytes = b" \n".to_vec();
        new_bytes.extend(serde_json::to_vec(&new_manifest).unwrap());
        new_bytes.extend(b"\n ");
        let manifest_path = data_dir.join("components.json");
        fs::write(&manifest_path, &new_bytes).unwrap();
        let expected_hash = sha2::Sha256::digest(&new_bytes);
        let refreshed = refresh_installed_components_for_update(
            data_dir,
            Some(&serde_json::to_vec(&old_manifest).unwrap()),
            &new_bytes,
            "x86_64-linux",
        )
        .await
        .unwrap();

        assert_eq!(refreshed, vec!["localhost-provider".to_string()]);
        assert_eq!(fs::read(&install_path).unwrap(), b"new-binary");
        let installed = fs::read(&manifest_path).unwrap();
        assert_eq!(sha2::Sha256::digest(&installed), expected_hash);
        assert_eq!(installed, new_bytes);
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

    #[test]
    fn install_signed_model_catalog_copies_sibling_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let source_dir = tmp.path().join("source");
        let data_dir = tmp.path().join("data");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&data_dir).unwrap();
        let catalog = br#"{"payload":{"schema":"elastos.model.catalog/v1"},"signature":"ab","signer_did":"did:key:z"}"#;
        fs::write(source_dir.join(MODEL_CATALOG_FILE), catalog).unwrap();
        let head = catalog_head_cid(catalog).unwrap();
        let manifest: ComponentsManifest = serde_json::from_value(serde_json::json!({
            "external": {},
            "profiles": {},
            "model_catalog": {
                "head_cid": head,
                "publisher_dids": ["did:key:z6Mkabcdefghijklmnopqrstuvwxyz0123456789ABCDE"]
            }
        }))
        .unwrap();
        fs::write(
            source_dir.join("components.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        install_signed_model_catalog(&data_dir, &manifest, &source_dir.join("components.json"))
            .unwrap();
        assert_eq!(
            fs::read(data_dir.join(MODEL_CATALOG_FILE)).unwrap(),
            catalog
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(data_dir.join(MODEL_CATALOG_FILE))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn install_signed_model_catalog_skips_when_unpinned() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let manifest: ComponentsManifest = serde_json::from_value(serde_json::json!({
            "external": {},
            "profiles": {}
        }))
        .unwrap();
        install_signed_model_catalog(data_dir, &manifest, &data_dir.join("components.json"))
            .unwrap();
        assert!(!data_dir.join(MODEL_CATALOG_FILE).exists());
    }

    #[test]
    fn install_signed_model_catalog_rejects_head_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let source_dir = tmp.path().join("source");
        let data_dir = tmp.path().join("data");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(source_dir.join(MODEL_CATALOG_FILE), b"catalog-a").unwrap();
        let manifest: ComponentsManifest = serde_json::from_value(serde_json::json!({
            "external": {},
            "profiles": {},
            "model_catalog": {
                "head_cid": catalog_head_cid(b"catalog-b").unwrap(),
                "publisher_dids": ["did:key:z6Mkabcdefghijklmnopqrstuvwxyz0123456789ABCDE"]
            }
        }))
        .unwrap();
        let err =
            install_signed_model_catalog(&data_dir, &manifest, &source_dir.join("components.json"))
                .unwrap_err();
        assert!(err.to_string().contains("does not match the pinned"));
    }
}
