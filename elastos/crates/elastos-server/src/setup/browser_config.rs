//! Normal Home setup uses the same Engine/Exit configuration as source Homes.
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const GENERATOR: &str = include_str!("../../../../../scripts/browser-source-home-config.mjs");
const CONFIG_FILES: [&str; 4] = [
    "exit-provider.json",
    "browser-local-exit.json",
    "browser-vz-vsock-transport.json",
    "browser-engine-adapter.json",
];
const RETIRED_SELECTION: &[u8] = b"{\"adapters\": []}\n";

/// Opening Browser admits its host support files from the installed signed release.
/// The base Home profile stays small; each helper keeps the normal checksum gate.
pub(super) async fn ensure_host_components(
    data: &Path,
    manifest: &super::ComponentsManifest,
    platform: &str,
    context: super::FirstPartyCarrierContext,
) -> anyhow::Result<()> {
    ensure_viewer_ingress(data)?;
    if !local_vm_selected(data, platform)? {
        return configure(data, platform, false);
    }
    let Some(rows) = host_rows(manifest, platform)? else {
        return Ok(()); // Source Homes and older releases own their existing helpers.
    };
    if rows.iter().any(|row| row.missing(data)) {
        // Downloads take their rows from the release admitted against its capsule
        // records; a Runtime caller holds the writer until they are written.
        let admitted = super::admit_installed_components(data, context)?;
        let admitted_rows = match &admitted {
            Some(admitted) => host_rows(&admitted.manifest, platform)?.unwrap_or_default(),
            None => rows,
        };
        for row in &admitted_rows {
            if row.missing(data) {
                let admitted_row =
                    super::admitted_row(data, admitted.as_ref(), manifest, row.name, row.info)?;
                let dest = data.join(row.relative);
                super::download_component(data, &admitted_row, &dest, context).await?;
                // The receipts let the next open use these bytes without the writer.
                super::write_platform_cache_metadata(admitted_row.info(), &dest)?;
            }
            super::ensure_bundle_executable_link(data, row.name, row.info)?;
        }
    } else {
        for row in &rows {
            super::ensure_bundle_executable_link(data, row.name, row.info)?;
        }
    }
    // Engine and Exit bridges load the owner's selection when Home starts.
    // Setup publishes a first selection before that startup. A running Home
    // can repair helper bytes, but needs setup/restart for its first selection.
    if matches!(context, super::FirstPartyCarrierContext::Runtime) {
        anyhow::ensure!(
            read_engine_selection(
                data,
                std::env::var("ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG")
                    .ok()
                    .as_deref(),
            )?
            .is_some(),
            "Browser helpers are ready. Close Home, run elastos setup, then start Home again."
        );
        return Ok(());
    }
    configure(
        data,
        platform,
        manifest
            .external
            .contains_key(super::browser_vm_image::NAME),
    )
}

/// One Browser host helper row of a release.
#[derive(Clone, Copy)]
struct HostRow<'a> {
    name: &'a str,
    component: &'a super::Component,
    info: &'a super::PlatformInfo,
    relative: &'a str,
}

impl HostRow<'_> {
    fn missing(&self, data: &Path) -> bool {
        !matches!(
            super::component_install_state(data, self.component, Some(self.info)),
            super::InstallState::Installed
        ) || (self.info.extract_path.is_none() && !executable(&data.join(self.relative)))
    }
}

/// The release's Browser host helpers for `platform`, each a prepared signed
/// release artifact. `None` when the release has no `browser-host` profile.
fn host_rows<'a>(
    manifest: &'a super::ComponentsManifest,
    platform: &str,
) -> anyhow::Result<Option<Vec<HostRow<'a>>>> {
    let Some(profile) = manifest.profiles.get("browser-host") else {
        return Ok(None);
    };
    let mut rows = Vec::new();
    for name in &profile.components {
        let component = manifest
            .external
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Browser host component '{name}' is absent"))?;
        let Some(info) = super::resolve_platform_info(component, platform) else {
            continue;
        };
        super::valid_release_artifact_checksum(name, info.checksum.as_deref())?;
        anyhow::ensure!(
            matches!(info.strategy.as_deref(), None | Some("prebuilt")),
            "Browser host component '{name}' requires a prepared release artifact"
        );
        let relative = super::resolve_install_path(component, Some(info)).ok_or_else(|| {
            anyhow::anyhow!("Browser host component '{name}' has no install path")
        })?;
        crate::install_transaction::validate_support_path(Path::new(relative))?;
        anyhow::ensure!(
            info.release_path
                .as_deref()
                .is_some_and(|p| !p.trim().is_empty()),
            "Browser host component '{name}' must come from the signed release"
        );
        rows.push(HostRow {
            name,
            component,
            info,
            relative,
        });
    }
    Ok(Some(rows))
}

pub(super) fn local_vm_selected(data: &Path, platform: &str) -> anyhow::Result<bool> {
    local_vm_selected_with_override(
        data,
        platform,
        std::env::var("ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG")
            .ok()
            .as_deref(),
    )
}

/// Startup only warms an Engine the Home has already configured. Setup may
/// select a default for a fresh Home through `local_vm_selected`.
pub(super) fn local_vm_configured(data: &Path, platform: &str) -> anyhow::Result<bool> {
    let inline = std::env::var("ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG").ok();
    Ok(read_engine_selection(data, inline.as_deref())?.is_some()
        && local_vm_selected_with_override(data, platform, inline.as_deref())?)
}

fn read_engine_selection(
    data: &Path,
    inline: Option<&str>,
) -> anyhow::Result<Option<serde_json::Value>> {
    if let Some(raw) = inline {
        return Ok(Some(serde_json::from_str(raw)?));
    }
    let path = data.join("config/browser-engine-adapter.json");
    match fs::read(&path) {
        Ok(bytes) if bytes == RETIRED_SELECTION => Ok(None),
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound && path.symlink_metadata().is_err() =>
        {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

/// Platforms with a Browser Engine implementation. Each Linux Home also
/// requires its selected native Engine and host capability proof.
pub fn browser_available(platform: &str) -> bool {
    matches!(platform, "darwin-arm64" | "linux-arm64")
}

pub fn home_browser_available(data: &Path, platform: &str) -> bool {
    home_browser_available_with_probe(
        data,
        platform,
        std::env::var("ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG")
            .ok()
            .as_deref(),
        || native_host_available(data).is_ok(),
    )
}

fn home_browser_available_with_probe(
    data: &Path,
    platform: &str,
    inline: Option<&str>,
    native_available: impl FnOnce() -> bool,
) -> bool {
    // Apple Silicon retains its existing local and approved remote Engines.
    if platform == "darwin-arm64" {
        return true;
    }
    if platform != "linux-arm64" {
        return false;
    }
    let Ok(Some(config)) = read_engine_selection(data, inline) else {
        return false;
    };
    let native = data.join("bin/browser-vz-engine-supervisor");
    config["adapters"].as_array().is_some_and(|adapters| {
        adapters.iter().any(|adapter| {
            let supervisor = &adapter["supervisor"];
            let launcher = supervisor["env"]["ELASTOS_BROWSER_VM_CONTROL_LAUNCHER"]
                .as_str()
                .or_else(|| supervisor["program"].as_str());
            adapter["kind"] == "chromium_microvm" && launcher == native.to_str()
        })
    }) && native_available()
}

fn native_host_available(data: &Path) -> anyhow::Result<()> {
    let program = data.join("bin/browser-vz-engine-supervisor");
    super::verify_installed_component_binary(data, "browser-vz-engine-supervisor", &program)?;
    probe_native_host(&program, data, std::time::Duration::from_secs(2))
}

fn probe_native_host(
    program: &Path,
    data: &Path,
    timeout: std::time::Duration,
) -> anyhow::Result<()> {
    use std::time::{Duration, Instant};

    let mut child = Command::new(program)
        .arg("--host-capabilities")
        .env_clear()
        .env("ELASTOS_BROWSER_VM_DATA_DIR", data)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("Browser native host capability check did not complete");
            }
        }
    }
    let output = child.wait_with_output()?;
    anyhow::ensure!(
        output.status.success(),
        "Browser native host capability check failed"
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    anyhow::ensure!(
        result["schema"] == "elastos.browser.vm-host-capabilities/v1"
            && result["available"] == true,
        "Browser requires its admitted native Engine and access to the selected KVM transport"
    );
    Ok(())
}

fn local_vm_selected_with_override(
    data: &Path,
    platform: &str,
    inline: Option<&str>,
) -> anyhow::Result<bool> {
    if !browser_available(platform) {
        return Ok(false);
    }
    let Some(config) = read_engine_selection(data, inline)? else {
        return Ok(true);
    };
    Ok(config["adapters"].as_array().is_some_and(|adapters| {
        adapters.iter().any(|adapter| {
            let supervisor = &adapter["supervisor"];
            let launcher = supervisor["env"]["ELASTOS_BROWSER_VM_CONTROL_LAUNCHER"]
                .as_str()
                .or_else(|| supervisor["program"].as_str())
                .unwrap_or_default();
            adapter["kind"] == "chromium_microvm"
                && !Path::new(launcher)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("browser-vm-remote-vz-launcher"))
        })
    }))
}

fn ensure_viewer_ingress(data: &Path) -> anyhow::Result<()> {
    let path = data.join("config/browser-viewer-ingress.json");
    if path.symlink_metadata().is_ok() {
        return Ok(()); // The Home owner keeps any approved off-device viewer route.
    }
    let config = serde_json::json!({
        "schema": "elastos.browser.viewer-ingress-config/v1",
        "listen_host": "127.0.0.1",
        "advertised_host": "127.0.0.1",
        "port_start": 48100,
        "port_end": 48131,
    });
    fs::create_dir_all(data.join("config"))?;
    super::atomic_write_file(&path, &serde_json::to_vec_pretty(&config)?)?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

fn host_program(data: &Path, name: &str) -> Option<PathBuf> {
    let path = data.join("bin").join(name);
    executable(&path).then_some(path)
}

#[cfg(target_os = "linux")]
fn require_kvm(path: &Path) -> anyhow::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::FileTypeExt;
    let file = fs::OpenOptions::new().read(true).write(true).open(path)
        .map_err(|error| anyhow::anyhow!("Browser virtualization is unavailable to this Home user: {error}. Complete the installer host-access step, or choose another approved Engine"))?;
    anyhow::ensure!(
        file.metadata()?.file_type().is_char_device(),
        "Browser virtualization requires a KVM character device"
    );
    // KVM_GET_API_VERSION requires a zero argument and returns the stable API version.
    let version = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            0xAE00,
            std::ptr::null_mut::<libc::c_void>(),
        )
    };
    anyhow::ensure!(
        version == 12,
        "Browser virtualization requires an accessible KVM API (version 12)"
    );
    Ok(())
}

/// Browser preparation can be repaired later without withholding the Home.
pub(super) async fn prepare_home(
    data: &Path,
    manifest: &super::ComponentsManifest,
    platform: &str,
    browser_selected: bool,
) {
    let result: anyhow::Result<()> = async {
        if !browser_available(platform) {
            return retire_host(data, manifest);
        }
        if browser_selected && manifest.profiles.contains_key("browser-host") {
            println!("Preparing Browser Engine helpers...");
            ensure_host_components(
                data,
                manifest,
                platform,
                super::FirstPartyCarrierContext::Setup,
            )
            .await?;
            if local_vm_selected(data, platform)?
                && manifest
                    .external
                    .contains_key(super::browser_vm_image::NAME)
            {
                #[cfg(target_os = "linux")]
                if platform == "linux-arm64" {
                    require_kvm(Path::new("/dev/kvm"))?;
                    native_host_available(data)?;
                }
                println!("Preparing Browser Engine image...");
                super::ensure_browser_vm_image_in_context(
                    data,
                    super::FirstPartyCarrierContext::Setup,
                )
                .await?;
            }
        }
        configure(
            data,
            platform,
            manifest
                .external
                .contains_key(super::browser_vm_image::NAME),
        )
    }
    .await;
    if let Err(error) = result {
        println!("Browser preparation is incomplete: {error}. Home setup continues. Repair Browser with elastos setup, or select another approved Engine.");
    }
}

/// Signed bytes [`prepare_home`] downloads for Browser in this state: missing
/// host helpers and the Engine image. Keep in step with `prepare_home`. This
/// is for the setup summary only; a selection or row it cannot read counts as
/// nothing, and `prepare_home` reports that problem itself.
pub(super) fn pending_download_bytes(
    data: &Path,
    manifest: &super::ComponentsManifest,
    platform: &str,
    browser_selected: bool,
) -> u64 {
    if !browser_available(platform) || !browser_selected {
        return 0;
    }
    let Ok(Some(rows)) = host_rows(manifest, platform) else {
        return 0;
    };
    if !local_vm_selected(data, platform).unwrap_or(false) {
        return 0;
    }
    let helpers = rows
        .iter()
        .filter(|row| row.missing(data))
        .map(|row| row.info.size.unwrap_or_default())
        .fold(0u64, u64::saturating_add);
    let image = manifest
        .external
        .contains_key(super::browser_vm_image::NAME)
        .then(|| super::browser_vm_image::component_info(manifest, platform).ok())
        .flatten()
        .filter(|info| !super::browser_vm_image::receipts_match(data, info))
        .and_then(|info| info.size)
        .unwrap_or_default();
    helpers.saturating_add(image)
}

/// A host without Browser keeps no Engine selection and none of the
/// release-owned Browser host files an earlier release installed, so neither
/// Runtime nor the installer's Linux host step acts on them. Browser profiles
/// under `Users/` belong to their owner and stay.
fn retire_host(data: &Path, manifest: &super::ComponentsManifest) -> anyhow::Result<()> {
    let browser_installed = data.join("capsules/browser").is_dir();
    let selection = data.join("config/browser-engine-adapter.json");
    if browser_installed || selection.symlink_metadata().is_ok() {
        fs::create_dir_all(data.join("config"))?;
        super::atomic_write_file(&selection, RETIRED_SELECTION)?;
    }
    // Exact release paths, relative to the data directory.
    let mut paths = vec![
        PathBuf::from(super::browser_vm_image::INSTALL_PATH),
        // Exact launcher and pool paths retained by older release manifests.
        PathBuf::from("bin/browser-vm-local-crosvm-launcher"),
        PathBuf::from("bin/browser-vm-local-crosvm-launcher.mjs"),
        PathBuf::from("bin/browser-vm-prepare-rootfs-pool"),
        PathBuf::from("scripts/browser-vm-prepare-rootfs-pool.mjs"),
    ];
    if let Some(profile) = manifest.profiles.get("browser-host") {
        for name in &profile.components {
            let shared = manifest
                .profiles
                .iter()
                .any(|(other, value)| other != "browser-host" && value.components.contains(name));
            let Some(component) = manifest.external.get(name).filter(|_| !shared) else {
                continue;
            };
            // Every platform's path: an earlier release may have used another.
            for info in component.platforms.values() {
                if let Some(relative) = super::resolve_install_path(component, Some(info)) {
                    crate::install_transaction::validate_support_path(Path::new(relative))?;
                    paths.push(PathBuf::from(relative));
                }
                if info.binary_path.is_some() {
                    paths.push(Path::new("bin").join(name));
                }
            }
        }
    }
    let mut removed = false;
    for relative in paths {
        // Never through a linked folder; a link itself is removed, its target never followed.
        let expected = if matches!(
            relative.to_str(),
            Some(
                "bin/browser-vm-local-crosvm-launcher"
                    | "bin/browser-vm-local-crosvm-launcher.mjs"
                    | "bin/browser-vm-prepare-rootfs-pool"
                    | "scripts/browser-vm-prepare-rootfs-pool.mjs"
            )
        ) {
            match data.join(&relative).symlink_metadata() {
                Ok(info) => Some(elastos_common::owned_removal::FileIdentity::of(&info)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            }
        } else {
            None
        };
        match elastos_common::owned_removal::remove_owned(data, &relative, expected) {
            Ok(found) => removed |= found,
            Err(elastos_common::owned_removal::RemovalError::LinkedParent(_)) => println!(
                "Kept {}: a folder on its path is a link, so removing it could reach files outside the data directory.",
                relative.display()
            ),
            Err(error) => return Err(error.into()),
        }
    }
    if removed {
        println!("Removed the Browser host files an earlier release installed.");
    }
    if browser_installed {
        println!("Browser is not available on this computer yet.");
    }
    Ok(())
}

pub(super) fn configure(data: &Path, platform: &str, release_image: bool) -> anyhow::Result<()> {
    if !data.join("capsules/browser").is_dir() {
        return Ok(());
    }
    let config = data.join("config");
    ensure_viewer_ingress(data)?;
    // Existing Engine/Exit selection belongs to the Home owner, including a
    // remote Engine. Setup can replace its exact retired/default local selection.
    let inline = std::env::var("ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG").ok();
    let mut replace_selection = inline.is_none()
        && fs::read(config.join("browser-engine-adapter.json"))
            .is_ok_and(|bytes| bytes == RETIRED_SELECTION);
    if let Some(existing) = read_engine_selection(data, inline.as_deref())? {
        if existing["adapters"].as_array().is_some_and(|adapters| {
            adapters
                .iter()
                .any(|adapter| adapter["kind"] != "chromium_microvm")
        }) {
            println!("The selected Browser Engine is retired. Select the VM Engine or an approved remote Engine. Your Browser profile remains with its owner.");
        }
        replace_selection = inline.is_none()
            && platform == "linux-arm64"
            && existing["adapters"].as_array().is_some_and(|adapters| {
                adapters.len() == 1
                    && adapters[0]["kind"] == "chromium_microvm"
                    && adapters[0]["supervisor"]["program"].as_str()
                        == data.join("bin/browser-vm-engine-supervisor").to_str()
                    && adapters[0]["supervisor"]["env"]["ELASTOS_BROWSER_VM_CONTROL_LAUNCHER"]
                        .as_str()
                        == data.join("bin/browser-vm-local-crosvm-launcher").to_str()
            });
        if !replace_selection {
            return Ok(());
        }
    }
    if !browser_available(platform) {
        super::atomic_write_file(
            &config.join("browser-engine-adapter.json"),
            RETIRED_SELECTION,
        )?;
        println!("Browser is not available on this computer yet.");
        return Ok(());
    }
    let mut required = vec![
        "bin/browser-engine-adapter",
        "bin/exit-provider",
        "bin/browser-local-exit",
        "bin/browser-vm-engine-supervisor",
        "bin/browser-vm-control-service",
        "scripts/browser-vm-artifact-preflight.sh",
    ];
    required.push("bin/browser-vz-engine-supervisor");
    if platform == "linux-arm64" {
        required.extend(["bin/crosvm", "scripts/browser-vm-linux-network.py"]);
    }
    let mut missing: Vec<_> = required
        .into_iter()
        .filter(|name| !executable(&data.join(name)))
        .map(str::to_owned)
        .collect();
    // Wrappers and their module files form one host helper installation.
    for name in ["browser-vm-engine-supervisor", "browser-vm-control-service"] {
        if !data.join(format!("bin/{name}.mjs")).is_file() {
            missing.push(format!("bin/{name}.mjs"));
        }
    }
    if platform == "darwin-arm64"
        && !data
            .join("scripts/browser-selkies-control-service.mjs")
            .is_file()
    {
        missing.push("scripts/browser-selkies-control-service.mjs".into());
    }
    let node = host_program(data, "node");
    let turn = host_program(data, "turnserver");
    if node.is_none() {
        missing.push("node".into());
    }
    if turn.is_none() {
        missing.push("turnserver".into());
    }
    if !missing.is_empty() {
        println!("Browser local Engine setup needs host helpers: {}. Use a release that supplies these helpers, then run elastos setup again, or select a remote Engine.", missing.join(", "));
        return Ok(());
    }
    generate_with_migration(
        data,
        platform,
        release_image,
        &node.unwrap(),
        &turn.unwrap(),
        replace_selection,
    )
}

#[cfg(test)]
fn generate(
    data: &Path,
    platform: &str,
    release_image: bool,
    node: &Path,
    turn: &Path,
) -> anyhow::Result<()> {
    generate_with_migration(data, platform, release_image, node, turn, false)
}

fn generate_with_migration(
    data: &Path,
    platform: &str,
    release_image: bool,
    node: &Path,
    turn: &Path,
    replace_selection: bool,
) -> anyhow::Result<()> {
    fs::create_dir_all(data.join("config"))?;
    let staged = tempfile::tempdir_in(data.join("config"))?;
    let mut command = Command::new(node);
    command
        .env_clear()
        .env("ELASTOS_BROWSER_VM_TURN_PROGRAM", turn)
        .env("ELASTOS_BROWSER_VM_TURNSERVER_BIN", turn)
        .env("ELASTOS_BROWSER_VM_PYTHON", data.join("bin/python3"))
        .env("ELASTOS_DEBUGFS_BIN", data.join("bin/debugfs"))
        .args(["--input-type=module", "-", "--data-dir"])
        .arg(data)
        .args(["--platform", platform, "--out-dir"])
        .arg(staged.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if release_image {
        command.arg("--release-image");
    }
    let mut child = command.spawn()?;
    let written = child.stdin.take().unwrap().write_all(GENERATOR.as_bytes());
    let output = child.wait_with_output()?;
    written?;
    anyhow::ensure!(
        output.status.success(),
        "Browser configuration failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    for name in CONFIG_FILES {
        let source = staged.path().join(name);
        if source.exists()
            && (!data.join("config").join(name).exists()
                || (replace_selection && name == "browser-engine-adapter.json"))
        {
            super::atomic_write_file(&data.join("config").join(name), &fs::read(&source)?)?;
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                data.join("config").join(name),
                fs::Permissions::from_mode(0o600),
            )?;
        }
    }
    println!("Browser local Engine and Exit configuration is ready. Runtime owns the selected Browser profile and its verified image.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use sha2::Digest;

    #[test]
    fn arm_browser_requires_its_selected_managed_native_host() {
        let data = Path::new("/fixture/elastos");
        let selected = |program: &str| {
            serde_json::json!({"adapters":[{
                "kind":"chromium_microvm", "supervisor":{"env":{
                    "ELASTOS_BROWSER_VM_CONTROL_LAUNCHER": program
                }}
            }]})
            .to_string()
        };
        let native = selected("/fixture/elastos/bin/browser-vz-engine-supervisor");
        assert!(home_browser_available_with_probe(
            data,
            "linux-arm64",
            Some(&native),
            || true
        ));
        assert!(!home_browser_available_with_probe(
            data,
            "linux-arm64",
            Some(&native),
            || false
        ));
        for platform in ["linux-amd64", "darwin-amd64", "linux-riscv64"] {
            assert!(!home_browser_available_with_probe(
                data,
                platform,
                Some(&native),
                || true
            ));
        }
        for program in [
            "/other/browser-vz-engine-supervisor",
            "/fixture/elastos/bin/browser-vm-local-crosvm-launcher",
            "/fixture/elastos/bin/browser-vm-remote-vz-launcher",
        ] {
            assert!(!home_browser_available_with_probe(
                data,
                "linux-arm64",
                Some(&selected(program)),
                || true
            ));
        }
        for raw in ["invalid", r#"{"adapters":[]}"#] {
            assert!(!home_browser_available_with_probe(
                data,
                "linux-arm64",
                Some(raw),
                || true
            ));
        }
        let remote = selected("/fixture/elastos/bin/browser-vm-remote-vz-launcher");
        assert!(home_browser_available_with_probe(
            data,
            "darwin-arm64",
            Some(&remote),
            || false
        ));
    }

    #[test]
    fn native_host_probe_requires_its_contract_and_has_a_deadline() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::Duration;
        let temp = tempfile::tempdir().unwrap();
        let program = temp.path().join("probe");
        for (output, accepted) in [
            (
                r#"{"schema":"elastos.browser.vm-host-capabilities/v1","available":true}"#,
                true,
            ),
            (
                r#"{"schema":"elastos.browser.vm-host-capabilities/v1","available":false}"#,
                false,
            ),
            (r#"{"schema":"other","available":true}"#, false),
            ("invalid", false),
        ] {
            fs::write(&program, format!("#!/bin/sh\n[ \"$1\" = --host-capabilities ] || exit 2\nprintf '%s\\n' '{output}'\n")).unwrap();
            fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(
                probe_native_host(&program, temp.path(), Duration::from_secs(1)).is_ok(),
                accepted
            );
        }
        fs::write(&program, "#!/bin/sh\nwhile :; do :; done\n").unwrap();
        assert!(
            probe_native_host(&program, temp.path(), Duration::from_millis(20))
                .unwrap_err()
                .to_string()
                .contains("did not complete")
        );
        // Admission precedes execution even when the named file is executable.
        fs::create_dir(temp.path().join("bin")).unwrap();
        fs::rename(
            &program,
            temp.path().join("bin/browser-vz-engine-supervisor"),
        )
        .unwrap();
        assert!(native_host_available(temp.path()).is_err());
    }

    fn fixture_node() -> PathBuf {
        let output = Command::new("node")
            .args(["-p", "process.execPath"])
            .output()
            .unwrap();
        assert!(output.status.success());
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
    }

    #[test]
    fn host_helpers_require_the_managed_installation() {
        for name in ["node", "turnserver"] {
            assert!(host_program(Path::new("/nonexistent"), name).is_none());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kvm_admission_refuses_missing_regular_and_non_kvm_devices() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kvm");
        assert!(require_kvm(&path).is_err());
        fs::write(&path, "ordinary file").unwrap();
        assert!(require_kvm(&path).is_err());
        assert!(require_kvm(Path::new("/dev/null")).is_err());
    }

    #[tokio::test]
    async fn failed_browser_preparation_preserves_home_and_profile() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path();
        fs::create_dir_all(data.join("capsules/browser")).unwrap();
        fs::create_dir_all(data.join("config")).unwrap();
        fs::write(
            data.join("config/browser-engine-adapter.json"),
            b"invalid selection",
        )
        .unwrap();
        fs::write(data.join("profile.ext4"), b"owner state").unwrap();
        let manifest = serde_json::from_str::<super::super::ComponentsManifest>(
            r#"{"external":{},"profiles":{}}"#,
        )
        .unwrap();
        prepare_home(data, &manifest, "darwin-arm64", true).await;
        assert_eq!(fs::read(data.join("profile.ext4")).unwrap(), b"owner state");
        assert_eq!(
            fs::read(data.join("config/browser-engine-adapter.json")).unwrap(),
            b"invalid selection"
        );
    }

    #[test]
    fn setup_summary_counts_the_browser_helpers_and_image_setup_downloads() {
        let mut manifest: super::super::ComponentsManifest =
            serde_json::from_str(include_str!("../../../../../components.json")).unwrap();
        let platform = "darwin-arm64";
        let mut helpers = 0;
        for name in manifest.profiles["browser-host"].components.clone() {
            let component = manifest.external.get_mut(&name).unwrap();
            let Some(info) = component.platforms.get_mut(platform) else {
                continue;
            };
            info.strategy = None;
            info.checksum = Some(format!("sha256:{}", "a".repeat(64)));
            info.size = Some(1000);
            helpers += 1000;
        }
        assert!(helpers > 0);
        let image = manifest
            .external
            .get_mut(super::super::browser_vm_image::NAME)
            .unwrap()
            .platforms
            .get_mut(platform)
            .unwrap();
        image.checksum = Some(format!("sha256:{}", "b".repeat(64)));
        image.size = Some(911_000_000);
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path();
        assert_eq!(
            pending_download_bytes(data, &manifest, platform, true),
            helpers + 911_000_000
        );
        // No Browser selected, a host without Browser, or a remote Engine: nothing.
        assert_eq!(pending_download_bytes(data, &manifest, platform, false), 0);
        assert_eq!(
            pending_download_bytes(data, &manifest, "linux-arm64", true),
            0
        );
        fs::create_dir_all(data.join("config")).unwrap();
        fs::write(
            data.join("config/browser-engine-adapter.json"),
            br#"{"adapters": []}"#,
        )
        .unwrap();
        assert_eq!(pending_download_bytes(data, &manifest, platform, true), 0);
    }

    #[tokio::test]
    async fn browser_host_profile_is_on_demand_and_setup_uses_admitted_helpers() {
        use std::os::unix::fs::PermissionsExt;
        let mut manifest: super::super::ComponentsManifest =
            serde_json::from_str(include_str!("../../../../../components.json")).unwrap();
        let names = manifest.profiles["browser-host"].components.clone();
        for (profile, value) in &manifest.profiles {
            if profile != "browser-host" {
                // crosvm also belongs to the existing generic VM profiles.
                assert!(value
                    .components
                    .iter()
                    .all(|name| name == "crosvm" || !names.contains(name)));
            }
        }
        let platform = "darwin-arm64";
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path();
        fs::create_dir_all(data.join("capsules/browser")).unwrap();
        configure(data, platform, true).unwrap();
        assert!(!data.join("config/browser-engine-adapter.json").exists());
        // Each helper is a signed release artifact served over Carrier: a bundle
        // archive, or the program itself.
        let node = fixture_node();
        let mut artifacts = std::collections::HashMap::new();
        let mut turnserver_cid = String::new();
        for name in &names {
            let component = manifest.external.get_mut(name).unwrap();
            let Some(info) = component.platforms.get_mut(platform) else {
                continue;
            };
            let program = if name == "node" {
                // The fixture delegates execution to the test runner's Node;
                // the managed bundle and link still follow installed setup.
                format!(
                    "#!/bin/sh\nexec '{}' \"$@\"\n",
                    node.to_str().unwrap().replace('\'', "'\\''")
                )
                .into_bytes()
            } else {
                format!("{name} release fixture").into_bytes()
            };
            let bytes = match (&info.extract_path, &info.binary_path) {
                (Some(root), Some(binary)) => {
                    let encoder =
                        flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
                    let mut archive = tar::Builder::new(encoder);
                    let mut header = tar::Header::new_gnu();
                    header.set_size(program.len() as u64);
                    header.set_mode(0o755);
                    header.set_cksum();
                    archive
                        .append_data(&mut header, format!("{root}/{binary}"), program.as_slice())
                        .unwrap();
                    archive.into_inner().unwrap().finish().unwrap()
                }
                _ => program,
            };
            let digest = sha2::Sha256::digest(&bytes);
            let cid = cid::Cid::new_v1(
                0x55,
                cid::multihash::Multihash::<64>::wrap(0x12, &digest).unwrap(),
            )
            .to_string();
            info.strategy = None;
            info.checksum = Some(format!("sha256:{digest:x}"));
            info.size = Some(bytes.len() as u64);
            info.cid = Some(cid.clone());
            if name == "turnserver" {
                turnserver_cid = cid.clone();
            }
            artifacts.insert(cid, bytes);
        }
        fs::create_dir_all(data.join("bin")).unwrap();
        for name in [
            "browser-engine-adapter",
            "exit-provider",
            "browser-local-exit",
            "crosvm",
        ] {
            if name == "crosvm" && manifest.external["crosvm"].platforms.contains_key(platform) {
                continue; // The admitted bundle owns this executable link.
            }
            let path = data.join("bin").join(name);
            fs::write(&path, b"fixture").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let source: crate::sources::TrustedSource = serde_json::from_value(serde_json::json!({
            "name": "fixture",
            "publisher_dids": [elastos_identity::derive_did(&[217; 32]).1],
            "connect_ticket": "",
            "publisher_node_id": ""
        }))
        .unwrap();
        let (_, server, serving) =
            crate::setup::tests::carrier_cid_source(data, &source, artifacts.clone()).await;
        let runtime = super::super::FirstPartyCarrierContext::Runtime;
        let setup = super::super::FirstPartyCarrierContext::Setup;
        // An open before setup installs the helpers; the first Engine selection
        // stays with setup.
        let error = ensure_host_components(data, &manifest, platform, runtime)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("Close Home, run elastos setup"),
            "{platform}: {error:#}"
        );
        assert!(!data.join("config/browser-engine-adapter.json").exists());
        // Setup uses the helpers that open installed.
        ensure_host_components(data, &manifest, platform, setup)
            .await
            .unwrap();
        assert!(data.join("config/browser-engine-adapter.json").is_file());
        assert!(data.join("config/exit-provider.json").is_file());
        assert!(data.join("bin/node").is_symlink());
        assert!(data.join("bin/turnserver").is_symlink());
        assert_eq!(
            fs::read(data.join("bin/turnserver")).unwrap(),
            b"turnserver release fixture"
        );
        // Setup reinstalls a missing helper, and the next open uses it as is.
        let turnserver = manifest.external["turnserver"].platforms[platform]
            .install_path
            .as_deref()
            .unwrap();
        fs::remove_dir_all(data.join(turnserver)).unwrap();
        ensure_host_components(data, &manifest, platform, setup)
            .await
            .unwrap();
        // A repeated open repairs a missing managed executable link.
        fs::remove_file(data.join("bin/turnserver")).unwrap();
        ensure_host_components(data, &manifest, platform, runtime)
            .await
            .unwrap();
        assert!(data.join("bin/turnserver").is_symlink());
        server.close().await;
        // Each install fetched its helpers once; no later open or setup fetched
        // an installed helper again.
        let mut requested = serving.await.unwrap();
        let mut expected = artifacts
            .into_keys()
            .chain([turnserver_cid])
            .collect::<Vec<_>>();
        requested.sort();
        expected.sort();
        assert_eq!(requested, expected);
    }

    #[tokio::test]
    async fn browser_host_admission_refuses_unprepared_or_url_only_helpers() {
        let temp = tempfile::tempdir().unwrap();
        let mut manifest: super::super::ComponentsManifest = serde_json::from_value(serde_json::json!({
            "schema":"elastos.components/v1", "capsules":{},
            "profiles":{"browser-host":{"components":["helper"]}},
            "external":{"helper":{"install_path":"bin/helper","platforms":{"darwin-arm64":{
                "url":"https://upstream.invalid/helper", "checksum":format!("sha256:{}", "a".repeat(64)),
                "strategy":"source-build"}}}}
        })).unwrap();
        let error = ensure_host_components(
            temp.path(),
            &manifest,
            "darwin-arm64",
            super::super::FirstPartyCarrierContext::Runtime,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("prepared release artifact"));
        manifest
            .external
            .get_mut("helper")
            .unwrap()
            .platforms
            .get_mut("darwin-arm64")
            .unwrap()
            .strategy = None;
        let error = ensure_host_components(
            temp.path(),
            &manifest,
            "darwin-arm64",
            super::super::FirstPartyCarrierContext::Runtime,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("signed release"));
        assert!(!temp.path().join("bin").exists());
    }

    #[tokio::test]
    async fn linux_setup_installs_no_browser_host_helpers_or_image() {
        let manifest: super::super::ComponentsManifest =
            serde_json::from_str(include_str!("../../../../../components.json")).unwrap();
        assert!(manifest.profiles["browser-host"]
            .components
            .iter()
            .any(|name| name == "browser-vm-linux-network"));
        let platform = "linux-amd64";
        assert!(!browser_available(platform));
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path();
        fs::create_dir_all(data.join("capsules/browser")).unwrap();
        prepare_home(data, &manifest, platform, true).await;
        assert!(!local_vm_selected(data, platform).unwrap());
        // install.sh asks for sudo only when this helper exists.
        assert!(!data.join("scripts/browser-vm-linux-network.py").exists());
        assert!(!data.join("scripts").exists());
        assert!(!data.join("bin").exists());
        assert!(!data.join("browser-vm").exists());
        let config: Value = serde_json::from_slice(
            &fs::read(data.join("config/browser-engine-adapter.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(config["adapters"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn linux_setup_removes_browser_host_files_an_earlier_release_installed() {
        use std::os::unix::fs::symlink;
        let mut manifest: super::super::ComponentsManifest =
            serde_json::from_str(include_str!("../../../../../components.json")).unwrap();
        // A component another profile also lists keeps its files.
        manifest.profiles.insert(
            "other".into(),
            super::super::Profile {
                extra: Default::default(),
                description: None,
                components: vec!["browser-vm-artifact-preflight".into()],
            },
        );
        let temp = tempfile::tempdir().unwrap();
        let canonical = temp.path().canonicalize().unwrap();
        let data = canonical.as_path();
        let crosvm = "libexec/browser/crosvm/0.1.0-2ac141082d17/linux-arm64";
        // This Home supplies the manifest of the earlier installed release.
        // Keep its exact path independent of the current producer version.
        manifest
            .external
            .get_mut("crosvm")
            .unwrap()
            .platforms
            .get_mut("linux-arm64")
            .unwrap()
            .install_path = Some(crosvm.into());
        let removed = [
            "scripts/browser-vm-linux-network.py",
            "bin/browser-vm-control-service",
            "bin/browser-vm-control-service.mjs",
            "bin/browser-vm-local-crosvm-launcher",
            "bin/browser-vm-local-crosvm-launcher.mjs",
            "bin/browser-vm-prepare-rootfs-pool",
            "scripts/browser-vm-prepare-rootfs-pool.mjs",
            "browser-vm/image-set/rootfs.ext4",
            "libexec/browser/crosvm/0.1.0-2ac141082d17/linux-arm64/bin/crosvm",
        ];
        let kept = [
            "Users/owner/BrowserProfiles/default/profile.ext4",
            "bin/browser-engine-adapter",
            "bin/elastos-unrelated",
            "scripts/unrelated.sh",
            "capsules/browser/capsule.json",
            "scripts/browser-vm-artifact-preflight.sh",
            // Only exact component paths go; a matching name alone does not.
            "scripts/browser-vm-retired-by-an-earlier-release.mjs",
        ];
        for name in removed.iter().chain(&kept) {
            let path = data.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, name.as_bytes()).unwrap();
        }
        symlink(
            data.join(crosvm).join("bin/crosvm"),
            data.join("bin/crosvm"),
        )
        .unwrap();
        fs::create_dir_all(data.join("config")).unwrap();
        let selection = data.join("config/browser-engine-adapter.json");
        fs::write(
            &selection,
            r#"{"adapters":[{"kind":"chromium_microvm","supervisor":{"program":"/vm/local"}}]}"#,
        )
        .unwrap();
        prepare_home(data, &manifest, "linux-amd64", true).await;
        for name in removed {
            assert!(!data.join(name).exists(), "{name}");
        }
        assert!(data.join("bin/crosvm").symlink_metadata().is_err());
        assert!(!data.join(crosvm).exists());
        assert!(!data.join("browser-vm/image-set").exists());
        for name in kept {
            assert_eq!(
                fs::read(data.join(name)).unwrap(),
                name.as_bytes(),
                "{name}"
            );
        }
        let config: Value = serde_json::from_slice(&fs::read(&selection).unwrap()).unwrap();
        assert_eq!(config["adapters"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn linux_setup_never_removes_through_a_linked_folder() {
        use std::os::unix::fs::symlink;
        let manifest: super::super::ComponentsManifest =
            serde_json::from_str(include_str!("../../../../../components.json")).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let (data, outside) = (base.join("elastos"), base.join("outside"));
        let kept = [
            data.join("Users/owner/image-set/profile.ext4"),
            outside.join("browser-vm-control-service"),
            outside.join("crosvm"),
        ];
        for path in &kept {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"owner data").unwrap();
        }
        symlink(data.join("Users/owner"), data.join("browser-vm")).unwrap();
        symlink(&outside, data.join("bin")).unwrap();
        fs::create_dir_all(data.join("scripts")).unwrap();
        fs::write(data.join("scripts/browser-vm-linux-network.py"), b"helper").unwrap();
        prepare_home(&data, &manifest, "linux-amd64", true).await;
        for path in &kept {
            assert_eq!(fs::read(path).unwrap(), b"owner data", "{}", path.display());
        }
        assert!(data.join("bin").symlink_metadata().unwrap().is_symlink());
        assert!(data
            .join("browser-vm")
            .symlink_metadata()
            .unwrap()
            .is_symlink());
        // Paths under real folders are still removed.
        assert!(!data.join("scripts/browser-vm-linux-network.py").exists());
    }

    #[test]
    fn removal_never_goes_through_a_linked_data_directory() {
        use std::os::unix::fs::symlink;
        let manifest: super::super::ComponentsManifest =
            serde_json::from_str(include_str!("../../../../../components.json")).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let (real, linked) = (base.join("real"), base.join("elastos"));
        let image = real.join(super::super::browser_vm_image::INSTALL_PATH);
        fs::create_dir_all(&image).unwrap();
        fs::write(image.join("rootfs.ext4"), b"owner data").unwrap();
        symlink(&real, &linked).unwrap();
        retire_host(&linked, &manifest).unwrap();
        assert_eq!(fs::read(image.join("rootfs.ext4")).unwrap(), b"owner data");
        retire_host(&real, &manifest).unwrap();
        assert!(!image.exists());
    }

    #[test]
    fn retirement_keeps_launchers_under_a_linked_data_ancestor() {
        use std::os::unix::fs::symlink;
        let manifest: super::super::ComponentsManifest =
            serde_json::from_str(include_str!("../../../../../components.json")).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let real = base.join("real");
        let data = real.join("elastos");
        let names = [
            "bin/browser-vm-local-crosvm-launcher",
            "bin/browser-vm-local-crosvm-launcher.mjs",
            "bin/browser-vm-prepare-rootfs-pool",
            "scripts/browser-vm-prepare-rootfs-pool.mjs",
        ];
        for name in names {
            let path = data.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"kept helper").unwrap();
        }
        symlink(&real, base.join("linked")).unwrap();
        retire_host(&base.join("linked/elastos"), &manifest).unwrap();
        for name in names {
            assert_eq!(fs::read(data.join(name)).unwrap(), b"kept helper");
        }
    }

    #[test]
    fn inline_engine_selection_is_json_and_overrides_the_owner_file() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path();
        let local =
            r#"{"adapters":[{"kind":"chromium_microvm","supervisor":{"program":"/vm/local"}}]}"#;
        fs::create_dir_all(data.join("config")).unwrap();
        let file = data.join("config/browser-engine-adapter.json");
        fs::write(&file, local).unwrap();
        assert!(local_vm_selected_with_override(data, "darwin-arm64", None).unwrap());
        for (platform, selected) in [("linux-arm64", true), ("linux-amd64", false)] {
            assert_eq!(
                local_vm_selected_with_override(data, platform, None).unwrap(),
                selected
            );
            assert_eq!(
                local_vm_selected_with_override(data, platform, Some(local)).unwrap(),
                selected
            );
        }
        for inline in [
            r#"{"adapters":[]}"#,
            r#"{"adapters":[{"kind":"chromium_microvm","supervisor":{"program":"/vm/browser-vm-remote-vz-launcher"}}]}"#,
        ] {
            assert!(!local_vm_selected_with_override(data, "darwin-arm64", Some(inline)).unwrap());
        }
        assert!(local_vm_selected_with_override(data, "darwin-arm64", Some(local)).unwrap());
        assert!(local_vm_selected_with_override(data, "darwin-arm64", Some("invalid")).is_err());
        assert_eq!(fs::read_to_string(file).unwrap(), local);
    }

    #[test]
    fn normal_browser_setup_reuses_generator_for_each_host() {
        let node = fixture_node();
        for platform in ["darwin-arm64", "linux-arm64"] {
            let temp = tempfile::tempdir().unwrap();
            let data = temp.path();
            generate(data, platform, true, &node, Path::new("/host/turnserver")).unwrap();
            let read = |name| -> Value {
                serde_json::from_slice(&fs::read(data.join("config").join(name)).unwrap()).unwrap()
            };
            let adapter = read("browser-engine-adapter.json");
            let env = &adapter["adapters"][0]["supervisor"]["env"];
            assert_eq!(adapter["adapters"][0]["network_mode"], "runtime_net_only");
            assert_eq!(
                adapter["adapters"][0]["display_modes"][0],
                "webrtc_remote_display"
            );
            assert_eq!(
                env["ELASTOS_BROWSER_VM_KERNEL"],
                data.join("browser-vm/image-set/vmlinux").to_str().unwrap()
            );
            assert_eq!(
                env["ELASTOS_BROWSER_VM_ROOTFS"],
                data.join("browser-vm/image-set/rootfs.ext4")
                    .to_str()
                    .unwrap()
            );
            assert!(env.get("ELASTOS_BROWSER_VM_PROFILE_ROOT").is_none());
            assert_eq!(
                env["ELASTOS_BROWSER_VM_PYTHON"],
                data.join("bin/python3").to_str().unwrap()
            );
            assert_eq!(
                env["ELASTOS_DEBUGFS_BIN"],
                data.join("bin/debugfs").to_str().unwrap()
            );
            let exit = read("exit-provider.json");
            assert_eq!(
                exit["backends"][0]["relay_ipc"]["path"],
                read("browser-local-exit.json")["relay_ipc_path"]
            );
            if platform == "darwin-arm64" {
                assert_eq!(read("browser-vz-vsock-transport.json")["enabled"], true);
                assert_eq!(env["ELASTOS_BROWSER_VM_TURN_PROGRAM"], "/host/turnserver");
            }
        }
    }

    #[test]
    fn fresh_linux_setup_uses_private_bound_session_transport() {
        let platform = "linux-arm64";
        let node = fixture_node();
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path();
        generate(data, platform, true, &node, Path::new("/host/turnserver")).unwrap();
        let read = |name: &str| -> Value {
            serde_json::from_slice(&fs::read(data.join("config").join(name)).unwrap()).unwrap()
        };
        let adapter = read("browser-engine-adapter.json");
        let env = &adapter["adapters"][0]["supervisor"]["env"];
        assert_eq!(
            env["ELASTOS_BROWSER_VM_CONTROL_LAUNCHER"],
            data.join("bin/browser-vz-engine-supervisor")
                .to_str()
                .unwrap()
        );
        assert_eq!(env["ELASTOS_BROWSER_VM_TURN_PROGRAM"], "/host/turnserver");
        assert!(env
            .as_object()
            .unwrap()
            .keys()
            .all(|key| !key.starts_with("ELASTOS_BROWSER_VM_ICE_")));
        assert!(env.get("ELASTOS_BROWSER_VM_ROOTFS_COPY_MODE").is_none());
        // A launch boots an overlay of the seed: no rootfs pool (#90).
        assert!(env.get("ELASTOS_BROWSER_VM_ROOTFS_POOL_DIR").is_none());
        let transport = read("browser-vz-vsock-transport.json");
        assert_eq!(transport["enabled"], true);
        assert_eq!(transport["turn_advertised_host"], "127.0.0.1");
        let issued = fs::read(data.join("config/browser-vz-vsock-transport.json")).unwrap();
        generate(data, platform, true, &node, Path::new("/host/turnserver")).unwrap();
        assert_eq!(
            fs::read(data.join("config/browser-vz-vsock-transport.json")).unwrap(),
            issued
        );
        // Runtime issues credentials for each launch authority. The setup
        // configuration owns port policy, rather than a reusable secret.
        assert!(transport.get("auth_secret").is_none());
        assert!(transport.get("credential").is_none());
    }

    #[test]
    fn normal_browser_setup_initializes_only_after_host_helpers_arrive() {
        use std::os::unix::{fs::symlink, fs::PermissionsExt};
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path();
        fs::create_dir_all(data.join("capsules/browser")).unwrap();
        let node = fixture_node();
        for name in [
            "bin/browser-engine-adapter",
            "bin/exit-provider",
            "bin/browser-local-exit",
            "bin/browser-vm-engine-supervisor",
            "bin/browser-vm-control-service",
            "bin/browser-vm-engine-supervisor.mjs",
            "bin/browser-vm-control-service.mjs",
            "bin/browser-vz-engine-supervisor",
            "scripts/browser-vm-artifact-preflight.sh",
            "scripts/browser-selkies-control-service.mjs",
            "bin/turnserver",
        ] {
            let file = data.join(name);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, b"isolated helper presence fixture").unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        }
        symlink(node, data.join("bin/node")).unwrap();
        configure(data, "darwin-arm64", true).unwrap();
        assert!(data.join("config/browser-engine-adapter.json").is_file());
        assert!(data
            .join("config/browser-vz-vsock-transport.json")
            .is_file());
    }

    #[test]
    fn normal_browser_setup_preserves_selection_and_waits_for_helpers() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path();
        fs::create_dir_all(data.join("capsules/browser")).unwrap();
        configure(data, "darwin-arm64", true).unwrap();
        assert!(!data.join("config/browser-engine-adapter.json").exists());
        fs::create_dir_all(data.join("config")).unwrap();
        let selection = data.join("config/browser-engine-adapter.json");
        let owner_selection = b"{\"adapters\": []}";
        fs::write(&selection, owner_selection).unwrap();
        configure(data, "darwin-arm64", true).unwrap();
        assert_eq!(fs::read(selection).unwrap(), owner_selection);
        assert!(!data.join("config/exit-provider.json").exists());
    }

    #[test]
    fn arm_setup_migrates_only_its_retired_or_default_local_selection() {
        use std::os::unix::{fs::symlink, fs::PermissionsExt};
        let node = fixture_node();
        for selection in ["retired", "legacy", "owner-off", "remote"] {
            let temp = tempfile::tempdir().unwrap();
            let data = temp.path();
            for directory in ["capsules/browser", "config", "bin", "scripts"] {
                fs::create_dir_all(data.join(directory)).unwrap();
            }
            for name in [
                "bin/browser-engine-adapter",
                "bin/exit-provider",
                "bin/browser-local-exit",
                "bin/browser-vm-engine-supervisor",
                "bin/browser-vm-control-service",
                "bin/browser-vm-engine-supervisor.mjs",
                "bin/browser-vm-control-service.mjs",
                "bin/browser-vz-engine-supervisor",
                "bin/crosvm",
                "bin/turnserver",
                "scripts/browser-vm-artifact-preflight.sh",
                "scripts/browser-vm-linux-network.py",
            ] {
                fs::write(data.join(name), b"presence fixture").unwrap();
                fs::set_permissions(data.join(name), fs::Permissions::from_mode(0o700)).unwrap();
            }
            symlink(&node, data.join("bin/node")).unwrap();
            let bytes = match selection {
                "retired" => RETIRED_SELECTION.to_vec(),
                "owner-off" => br#"{"adapters":[]}"#.to_vec(),
                _ => serde_json::to_vec(&serde_json::json!({"adapters":[{
                    "kind":"chromium_microvm", "supervisor":{
                        "program":data.join("bin/browser-vm-engine-supervisor"),
                        "env":{"ELASTOS_BROWSER_VM_CONTROL_LAUNCHER": data.join("bin").join(
                            if selection == "legacy" { "browser-vm-local-crosvm-launcher" }
                            else { "browser-vm-remote-vz-launcher" })}
                    }
                }]}))
                .unwrap(),
            };
            let file = data.join("config/browser-engine-adapter.json");
            fs::write(&file, &bytes).unwrap();
            configure(data, "linux-arm64", true).unwrap();
            let after = fs::read(&file).unwrap();
            if matches!(selection, "retired" | "legacy") {
                let config: Value = serde_json::from_slice(&after).unwrap();
                assert_eq!(
                    config["adapters"][0]["supervisor"]["env"]
                        ["ELASTOS_BROWSER_VM_CONTROL_LAUNCHER"],
                    data.join("bin/browser-vz-engine-supervisor")
                        .to_str()
                        .unwrap()
                );
                assert!(!data.join("runtime-turn/turn-credentials.env").exists());
            } else {
                assert_eq!(after, bytes, "{selection}");
            }
        }
    }

    #[test]
    fn interrupted_configuration_preserves_completed_files_and_finishes_selection() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path();
        fs::create_dir_all(data.join("config")).unwrap();
        fs::write(
            data.join("config/exit-provider.json"),
            b"owner exit configuration",
        )
        .unwrap();
        let node = fixture_node();
        generate(
            data,
            "darwin-arm64",
            true,
            &node,
            Path::new("/host/turnserver"),
        )
        .unwrap();
        assert_eq!(
            fs::read(data.join("config/exit-provider.json")).unwrap(),
            b"owner exit configuration"
        );
        assert!(data.join("config/browser-engine-adapter.json").is_file());
        assert!(data.join("config/browser-local-exit.json").is_file());
    }
}
