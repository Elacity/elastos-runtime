use std::path::{Path, PathBuf};

use elastos_logger::log_info;

use crate::{setup, sources::default_data_dir};

const LOG_COMPONENT: &str = "host.binaries";

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
    log_info!(
        component: LOG_COMPONENT,
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
