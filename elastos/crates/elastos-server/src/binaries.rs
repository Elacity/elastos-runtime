use std::path::{Path, PathBuf};

use crate::{setup, sources::default_data_dir};

/// Find a provider binary from an operator override or installed runtime paths.
pub fn find_installed_provider_binary(name: &str) -> Option<PathBuf> {
    let data_dir = default_data_dir();
    let env_name = format!(
        "ELASTOS_{}_BIN",
        name.to_ascii_uppercase().replace('-', "_")
    );

    if let Some(path) = std::env::var_os(env_name) {
        let candidate = PathBuf::from(path);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    if let Some(dir) = std::env::var_os("ELASTOS_CAPSULE_BIN_DIR") {
        let candidate = PathBuf::from(dir).join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    let installed_component = data_dir.join("bin").join(name);
    if installed_component.is_file() {
        return Some(installed_component);
    }

    let installed_capsule = data_dir.join("capsules").join(name).join(name);
    if installed_capsule.is_file() {
        return Some(installed_capsule);
    }

    if let Some(global_data_dir) = dirs::data_dir().map(|dir| dir.join("elastos")) {
        let global_component = global_data_dir.join("bin").join(name);
        if global_component.is_file() {
            return Some(global_component);
        }

        let global_capsule = global_data_dir.join("capsules").join(name).join(name);
        if global_capsule.is_file() {
            return Some(global_capsule);
        }
    }

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let from_exe = exe_dir.join("../share/elastos/bin").join(name);
            if from_exe.is_file() {
                return Some(from_exe);
            }
        }
    }

    if let Some(path_dirs) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_dirs) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

pub fn verify_component_binary_with_data_dir(
    data_dir: &Path,
    name: &str,
    path: &Path,
) -> anyhow::Result<()> {
    let checksum = setup::verify_installed_component_binary(data_dir, name, path)?;
    tracing::info!(
        "{} binary verified against installed manifest ({})",
        name,
        checksum
    );
    Ok(())
}

pub fn verify_component_binary(name: &str, path: &Path) -> anyhow::Result<()> {
    let data_dir = default_data_dir();
    verify_component_binary_with_data_dir(&data_dir, name, path)
}

pub fn resolve_verified_provider_binary(
    name: &str,
    missing_guidance: &str,
) -> anyhow::Result<PathBuf> {
    let path = find_installed_provider_binary(name)
        .ok_or_else(|| anyhow::anyhow!("{}", missing_guidance))?;
    verify_component_binary(name, &path)?;
    Ok(path)
}

pub fn resolve_verified_native_provider_binary_with_data_dir(
    data_dir: &Path,
    name: &str,
) -> anyhow::Result<Option<PathBuf>> {
    let manifest_path = data_dir.join("components.json");
    let manifest_bytes = std::fs::read(&manifest_path).map_err(|err| {
        anyhow::anyhow!(
            "cannot resolve native provider '{}': failed to read {}: {}",
            name,
            manifest_path.display(),
            err
        )
    })?;
    let manifest: setup::ComponentsManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|err| {
            anyhow::anyhow!(
                "cannot resolve native provider '{}': invalid {}: {}",
                name,
                manifest_path.display(),
                err
            )
        })?;
    let Some(component) = manifest.external.get(name) else {
        return Ok(None);
    };
    setup::validate_provider_runtime(name, component)?;
    let expected_install_path = format!("bin/{name}");
    let install_path = component.install_path.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "component '{}' is missing install_path for native provider resolution",
            name
        )
    })?;
    if install_path != expected_install_path {
        anyhow::bail!(
            "component '{}' native provider install_path must be '{}'",
            name,
            expected_install_path
        );
    }
    let path = data_dir.join(install_path);
    if !path.is_file() {
        return Ok(None);
    }
    verify_component_binary_with_data_dir(data_dir, name, &path)?;
    Ok(Some(path))
}

pub fn resolve_verified_native_provider_binary(name: &str) -> anyhow::Result<Option<PathBuf>> {
    let data_dir = default_data_dir();
    resolve_verified_native_provider_binary_with_data_dir(&data_dir, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    fn current_platform() -> &'static str {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => "linux-amd64",
            ("linux", "aarch64") => "linux-arm64",
            ("macos", "aarch64") => "darwin-arm64",
            (os, arch) => panic!("unsupported test platform {os}-{arch}"),
        }
    }

    fn write_components_manifest(
        data_dir: &Path,
        name: &str,
        checksum: &str,
        runtime: Option<serde_json::Value>,
    ) {
        let mut component = serde_json::json!({
            "install_path": format!("bin/{name}"),
            "platforms": {
                current_platform(): {
                    "checksum": checksum,
                    "install_path": format!("bin/{name}")
                }
            }
        });
        if let Some(runtime) = runtime {
            component["provider_runtime"] = runtime;
        }
        std::fs::write(
            data_dir.join("components.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "external": {
                    name: component
                },
                "capsules": {},
                "profiles": {}
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn native_runtime(provides: &str) -> serde_json::Value {
        serde_json::json!({
            "role": "provider",
            "substrate": "native",
            "runtime_abi": "elastos.provider-stdio/v1",
            "execution": "native-provider",
            "provides": provides
        })
    }

    #[test]
    fn resolve_verified_native_provider_binary_rejects_unstamped_external_binary() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let external_dir = temp.path().join("external-bin");
        std::fs::create_dir_all(&external_dir).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(external_dir.join("did-provider"), b"external-only").unwrap();
        write_components_manifest(
            &data_dir,
            "did-provider",
            "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            Some(native_runtime("elastos://did/*")),
        );

        let result =
            resolve_verified_native_provider_binary_with_data_dir(&data_dir, "did-provider")
                .unwrap();

        assert!(result.is_none());
    }

    #[test]
    fn resolve_verified_native_provider_binary_rejects_missing_runtime_contract() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let bin_dir = data_dir.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(bin_dir.join("did-provider"), b"binary").unwrap();
        write_components_manifest(
            data_dir,
            "did-provider",
            "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            None,
        );

        let error = resolve_verified_native_provider_binary_with_data_dir(data_dir, "did-provider")
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("missing provider runtime metadata"));
    }

    #[test]
    fn resolve_verified_native_provider_binary_rejects_malformed_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        std::fs::write(data_dir.join("components.json"), b"{not-json").unwrap();

        let error = resolve_verified_native_provider_binary_with_data_dir(data_dir, "did-provider")
            .unwrap_err();
        assert!(error.to_string().contains("invalid"));
        assert!(error.to_string().contains("components.json"));
    }

    #[test]
    fn resolve_verified_native_provider_binary_rejects_noncanonical_install_path() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        std::fs::write(
            data_dir.join("components.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "external": {
                    "did-provider": {
                        "install_path": "capsules/did-provider",
                        "provider_runtime": native_runtime("elastos://did/*"),
                        "platforms": {
                            current_platform(): {
                                "checksum": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
                                "install_path": "capsules/did-provider"
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

        let error = resolve_verified_native_provider_binary_with_data_dir(data_dir, "did-provider")
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("native provider install_path must be 'bin/did-provider'"));
    }

    #[test]
    fn resolve_verified_native_provider_binary_rejects_checksum_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let bin_dir = data_dir.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(bin_dir.join("did-provider"), b"binary").unwrap();
        write_components_manifest(
            data_dir,
            "did-provider",
            "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            Some(native_runtime("elastos://did/*")),
        );

        let error = resolve_verified_native_provider_binary_with_data_dir(data_dir, "did-provider")
            .unwrap_err();
        assert!(error.to_string().contains("failed checksum verification"));
    }

    #[test]
    fn resolve_verified_native_provider_binary_accepts_stamped_installed_binary() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let bin_dir = data_dir.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let bytes = b"binary";
        let path = bin_dir.join("did-provider");
        std::fs::write(&path, bytes).unwrap();
        let checksum = format!("sha256:{}", hex::encode(sha2::Sha256::digest(bytes)));
        write_components_manifest(
            data_dir,
            "did-provider",
            &checksum,
            Some(native_runtime("elastos://did/*")),
        );

        let resolved =
            resolve_verified_native_provider_binary_with_data_dir(data_dir, "did-provider")
                .unwrap();
        assert_eq!(resolved.as_deref(), Some(path.as_path()));
    }
}
