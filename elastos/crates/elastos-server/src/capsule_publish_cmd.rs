use std::path::PathBuf;

pub async fn run_publish(path: PathBuf) -> anyhow::Result<()> {
    let ipfs = crate::get_ipfs_bridge().await?;

    let manifest_path = path.join("capsule.json");
    if !manifest_path.exists() {
        anyhow::bail!("No capsule.json found at {}", path.display());
    }

    let manifest_data = std::fs::read_to_string(&manifest_path)?;
    let mut manifest: elastos_common::CapsuleManifest = serde_json::from_str(&manifest_data)?;
    manifest
        .validate()
        .map_err(|e| anyhow::anyhow!("Invalid manifest: {}", e))?;

    let entrypoint_path = path.join(&manifest.entrypoint);
    if !entrypoint_path.exists() {
        anyhow::bail!("Entrypoint {} not found", manifest.entrypoint);
    }

    if manifest.capsule_type == elastos_common::CapsuleType::MicroVM {
        elastos_server::ipfs::publish_microvm_capsule(&path, &mut manifest, &ipfs).await?;
        return Ok(());
    }

    println!("Publishing capsule '{}' to IPFS...", manifest.name);

    let cid = ipfs.add_directory_from_path(&path).await?;

    println!("\nCapsule published successfully!");
    println!("CID: {}", cid);
    println!("\nRun with:");
    println!("  elastos run --cid {}", cid);

    Ok(())
}
