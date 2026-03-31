use std::path::{Path, PathBuf};

use crate::{setup, sources::default_data_dir};

/// Find a provider binary from installed runtime paths only.
pub fn find_installed_provider_binary(name: &str) -> Option<PathBuf> {
    let data_dir = default_data_dir();

    let installed_component = data_dir.join("bin").join(name);
    if installed_component.is_file() {
        return Some(installed_component);
    }

    let installed_capsule = data_dir.join("capsules").join(name).join(name);
    if installed_capsule.is_file() {
        return Some(installed_capsule);
    }

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let from_exe = exe_dir.join("../share/elastos/bin").join(name);
            if from_exe.is_file() {
                return Some(from_exe);
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
