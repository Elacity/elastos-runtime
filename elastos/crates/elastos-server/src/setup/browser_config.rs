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
    let Some(profile) = manifest.profiles.get("browser-host") else {
        return Ok(()); // Source Homes and older releases own their existing helpers.
    };
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
        let url = super::resolve_component_download_url(info).ok_or_else(|| {
            anyhow::anyhow!("Browser host component '{name}' has no release path")
        })?;
        anyhow::ensure!(
            info.release_path
                .as_deref()
                .is_some_and(|p| !p.trim().is_empty()),
            "Browser host component '{name}' must come from the signed release"
        );
        if !matches!(
            super::component_install_state(data, component, Some(info)),
            super::InstallState::Installed
        ) || (info.extract_path.is_none() && !executable(&data.join(relative)))
        {
            super::download_component(
                data,
                name,
                &url,
                info,
                &data.join(relative),
                &super::build_gateway_list(data),
                context,
            )
            .await?;
        }
        super::ensure_bundle_executable_link(data, name, info)?;
    }
    // Engine and Exit bridges load the owner's selection when Home starts.
    // Setup publishes a first selection before that startup. A running Home
    // can repair helper bytes, but needs setup/restart for its first selection.
    if matches!(context, super::FirstPartyCarrierContext::Runtime) {
        anyhow::ensure!(
            data.join("config/browser-engine-adapter.json").exists()
                || std::env::var_os("ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG").is_some(),
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

pub(super) fn local_vm_selected(data: &Path, platform: &str) -> anyhow::Result<bool> {
    if !matches!(platform, "darwin-arm64" | "linux-arm64") {
        return Ok(false);
    }
    let path = std::env::var_os("ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| data.join("config/browser-engine-adapter.json"));
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error.into()),
    };
    let config: serde_json::Value = serde_json::from_slice(&bytes)?;
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
    std::iter::once(data.join("bin").join(name))
        .chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|dir| dir.join(name)),
        )
        .find(|path| executable(path))
}

pub(super) fn configure(data: &Path, platform: &str, release_image: bool) -> anyhow::Result<()> {
    if !data.join("capsules/browser").is_dir() {
        return Ok(());
    }
    let config = data.join("config");
    ensure_viewer_ingress(data)?;
    // Existing Engine/Exit selection belongs to the Home owner, including a
    // remote Engine. Setup initializes a fresh selection only.
    if config
        .join("browser-engine-adapter.json")
        .symlink_metadata()
        .is_ok()
    {
        let existing: serde_json::Value =
            serde_json::from_slice(&fs::read(config.join("browser-engine-adapter.json"))?)?;
        if existing["adapters"].as_array().is_some_and(|adapters| {
            adapters
                .iter()
                .any(|adapter| adapter["kind"] != "chromium_microvm")
        }) {
            println!("The selected Browser Engine is retired. Select the VM Engine or an approved remote Engine. Your Browser profile remains with its owner.");
        }
        return Ok(());
    }
    if !matches!(platform, "darwin-arm64" | "linux-arm64") {
        super::atomic_write_file(
            &config.join("browser-engine-adapter.json"),
            b"{\"adapters\": []}\n",
        )?;
        println!("Browser uses a remote Engine on this computer. Select an approved Engine in Home Services.");
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
    if platform == "darwin-arm64" {
        required.push("bin/browser-vz-engine-supervisor");
    } else if platform.starts_with("linux-") {
        required.extend([
            "bin/crosvm",
            "bin/browser-vm-local-crosvm-launcher",
            "bin/browser-vm-local-crosvm-launcher.mjs",
            "bin/browser-vm-prepare-rootfs-pool",
            "scripts/browser-vm-prepare-rootfs-pool.mjs",
            "scripts/browser-vm-linux-network.py",
        ]);
    } else {
        return Ok(());
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
    generate(
        data,
        platform,
        release_image,
        &node.unwrap(),
        &turn.unwrap(),
    )
}

fn generate(
    data: &Path,
    platform: &str,
    release_image: bool,
    node: &Path,
    turn: &Path,
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
        if source.exists() && !data.join("config").join(name).exists() {
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
        for platform in ["darwin-arm64", "linux-arm64"] {
            let temp = tempfile::tempdir().unwrap();
            let data = temp.path();
            fs::create_dir_all(data.join("capsules/browser")).unwrap();
            configure(data, platform, true).unwrap();
            assert!(!data.join("config/browser-engine-adapter.json").exists());
            for name in &names {
                let component = manifest.external.get_mut(name).unwrap();
                let Some(info) = component.platforms.get_mut(platform) else {
                    continue;
                };
                let relative = info
                    .install_path
                    .as_ref()
                    .or(component.install_path.as_ref())
                    .unwrap();
                let path = if let Some(binary) = &info.binary_path {
                    data.join(relative).join(binary)
                } else {
                    data.join(relative)
                };
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                if name == "node" {
                    // The fixture delegates execution to the test runner's Node;
                    // the managed bundle and link still follow installed setup.
                    let node = host_program(Path::new("/nonexistent"), "node").unwrap();
                    fs::write(
                        &path,
                        format!(
                            "#!/bin/sh\nexec '{}' \"$@\"\n",
                            node.to_str().unwrap().replace('\'', "'\\''")
                        ),
                    )
                    .unwrap();
                } else {
                    fs::write(&path, b"admitted host fixture").unwrap();
                }
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
                info.strategy = None;
                info.checksum = Some(format!(
                    "sha256:{}",
                    hex::encode(sha2::Sha256::digest(b"admitted host fixture"))
                ));
                info.size = Some(21);
                if info.binary_path.is_some() {
                    fs::write(
                        data.join(relative)
                            .join(super::super::CACHED_ARTIFACT_SHA_FILE),
                        info.checksum.as_deref().unwrap(),
                    )
                    .unwrap();
                }
            }
            for name in [
                "browser-engine-adapter",
                "exit-provider",
                "browser-local-exit",
                "crosvm",
            ] {
                if name == "crosvm" && manifest.external["crosvm"].platforms.contains_key(platform)
                {
                    continue; // The admitted bundle owns this executable link.
                }
                let path = data.join("bin").join(name);
                fs::write(&path, b"fixture").unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            let error = ensure_host_components(
                data,
                &manifest,
                platform,
                super::super::FirstPartyCarrierContext::Runtime,
            )
            .await
            .unwrap_err();
            assert!(
                error.to_string().contains("Close Home, run elastos setup"),
                "{platform}: {error:#}"
            );
            assert!(!data.join("config/browser-engine-adapter.json").exists());
            ensure_host_components(
                data,
                &manifest,
                platform,
                super::super::FirstPartyCarrierContext::Setup,
            )
            .await
            .unwrap();
            assert!(data.join("config/browser-engine-adapter.json").is_file());
            assert!(data.join("config/exit-provider.json").is_file());
            assert!(data.join("bin/node").is_symlink());
            assert!(data.join("bin/turnserver").is_symlink());
            assert_eq!(
                fs::read(data.join("bin/turnserver")).unwrap(),
                b"admitted host fixture"
            );
            // A repeated open repairs a missing managed executable link.
            fs::remove_file(data.join("bin/turnserver")).unwrap();
            ensure_host_components(
                data,
                &manifest,
                platform,
                super::super::FirstPartyCarrierContext::Runtime,
            )
            .await
            .unwrap();
            assert!(data.join("bin/turnserver").is_symlink());
        }
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

    #[test]
    fn x86_setup_prepares_remote_viewer_without_host_helpers() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path();
        fs::create_dir_all(data.join("capsules/browser")).unwrap();
        configure(data, "linux-amd64", true).unwrap();
        assert!(!local_vm_selected(data, "linux-amd64").unwrap());
        let config: Value = serde_json::from_slice(
            &fs::read(data.join("config/browser-engine-adapter.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(config["adapters"], serde_json::json!([]));
        assert!(crate::carrier::browser_engine_media::configuration_ready(data).is_ok());
        assert!(!data.join("bin").exists());
    }

    #[test]
    fn normal_browser_setup_reuses_generator_for_each_host() {
        let node = host_program(Path::new("/nonexistent"), "node")
            .expect("Node is required for setup configuration tests");
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
    fn normal_browser_setup_initializes_only_after_host_helpers_arrive() {
        use std::os::unix::{fs::symlink, fs::PermissionsExt};
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path();
        fs::create_dir_all(data.join("capsules/browser")).unwrap();
        let node = host_program(Path::new("/nonexistent"), "node").unwrap();
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
    fn interrupted_configuration_preserves_completed_files_and_finishes_selection() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path();
        fs::create_dir_all(data.join("config")).unwrap();
        fs::write(
            data.join("config/exit-provider.json"),
            b"owner exit configuration",
        )
        .unwrap();
        let node = host_program(Path::new("/nonexistent"), "node").unwrap();
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
