use std::path::{Path, PathBuf};

pub async fn run_capsule(
    path: Option<PathBuf>,
    cid: Option<String>,
    capsule_args: Vec<String>,
) -> anyhow::Result<()> {
    let capsule_dir = resolve_capsule_dir(path, cid).await?;
    let manifest = load_valid_manifest_if_present(&capsule_dir).await?;

    if let Some(ref manifest) = manifest {
        match manifest.capsule_type {
            elastos_common::CapsuleType::MicroVM => {
                return run_microvm_via_operator_runtime(manifest, &capsule_args).await;
            }
            elastos_common::CapsuleType::Wasm => {
                return run_wasm_via_operator_runtime(&capsule_dir, capsule_args).await;
            }
            elastos_common::CapsuleType::Data => {
                let runtime = crate::create_runtime("/tmp/elastos/storage").await?;
                let addr = crate::find_free_local_addr()?;
                return crate::serve_web_capsule(runtime, capsule_dir, &addr, true, Some(20)).await;
            }
            _ => {}
        }
    }

    let runtime = crate::create_runtime("/tmp/elastos/storage").await?;
    let handle = runtime.run_local(&capsule_dir, capsule_args).await?;

    match handle.manifest.capsule_type {
        elastos_common::CapsuleType::Wasm => {}
        _ => {
            println!(
                "Capsule '{}' running (ID: {})",
                handle.manifest.name, handle.id
            );
            println!("Press Ctrl+C to stop...");
            tokio::signal::ctrl_c().await?;
            println!("\nStopping capsule...");
            runtime.stop(&handle).await?;
            println!("Capsule stopped.");
        }
    }

    Ok(())
}

async fn resolve_capsule_dir(
    path: Option<PathBuf>,
    cid: Option<String>,
) -> anyhow::Result<PathBuf> {
    if let Some(cid) = cid {
        tracing::info!("Running capsule from CID: {}", cid);
        let ipfs_bridge = crate::get_ipfs_bridge().await?;
        return elastos_server::ipfs::prepare_capsule_from_cid(&ipfs_bridge, &cid).await;
    }

    if let Some(path) = path {
        tracing::info!("Running capsule from: {}", path.display());
        return Ok(path);
    }

    anyhow::bail!("Either path or --cid must be specified");
}

async fn load_valid_manifest_if_present(
    capsule_dir: &Path,
) -> anyhow::Result<Option<elastos_common::CapsuleManifest>> {
    let manifest_path = capsule_dir.join("capsule.json");
    if !manifest_path.exists() {
        return Ok(None);
    }

    let data = tokio::fs::read_to_string(&manifest_path).await?;
    let manifest: elastos_common::CapsuleManifest = serde_json::from_str(&data)?;
    manifest
        .validate()
        .map_err(|e| anyhow::anyhow!("Invalid manifest: {}", e))?;
    Ok(Some(manifest))
}

async fn operator_runtime_coords() -> anyhow::Result<crate::shell_cmd::RuntimeCoords> {
    let data_dir = crate::default_data_dir();
    let coords_path = crate::shell_cmd::runtime_coord_path(&data_dir);
    crate::shell_cmd::read_operator_runtime_coords(&coords_path)
        .await
        .ok_or_else(|| anyhow::anyhow!(crate::shell_cmd::OPERATOR_RUNTIME_REQUIRED_MESSAGE))
}

async fn run_microvm_via_operator_runtime(
    manifest: &elastos_common::CapsuleManifest,
    capsule_args: &[String],
) -> anyhow::Result<()> {
    let coords = operator_runtime_coords().await?;
    eprintln!("[run] Attaching to runtime at {}", coords.api_url);

    let tokens = crate::shell_cmd::attach_to_runtime(&coords).await?;
    let client = reqwest::Client::new();
    let _ = client
        .post(format!("{}/api/supervisor/ensure-capsule", coords.api_url))
        .header("Authorization", format!("Bearer {}", tokens.shell_token))
        .json(&serde_json::json!({"name": &manifest.name}))
        .send()
        .await;

    let resp = client
        .post(format!("{}/api/supervisor/launch-capsule", coords.api_url))
        .header("Authorization", format!("Bearer {}", tokens.shell_token))
        .json(&serde_json::json!({
            "name": &manifest.name,
            "config": {
                "_elastos_interactive": true,
                "_elastos_capsule_args": capsule_args,
            },
        }))
        .send()
        .await?;
    let body: serde_json::Value = resp.json().await?;
    if body.get("status").and_then(|s| s.as_str()) != Some("ok") {
        anyhow::bail!(
            "Launch failed: {}",
            body.get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown")
        );
    }

    let handle = body["handle"].as_str().unwrap_or("?").to_string();
    eprintln!("[run] MicroVM '{}' launched: {}", manifest.name, handle);
    let saved = crate::shell_cmd::enable_host_raw_mode_pub();
    tokio::signal::ctrl_c().await?;
    drop(saved);
    let _ = client
        .post(format!("{}/api/supervisor/stop-capsule", coords.api_url))
        .header("Authorization", format!("Bearer {}", tokens.shell_token))
        .json(&serde_json::json!({"handle": handle}))
        .send()
        .await;
    Ok(())
}

async fn run_wasm_via_operator_runtime(
    capsule_dir: &Path,
    capsule_args: Vec<String>,
) -> anyhow::Result<()> {
    let coords = operator_runtime_coords().await?;
    eprintln!(
        "[run] WASM capsule attached to runtime at {}",
        coords.api_url
    );

    let runtime = crate::create_runtime("/tmp/elastos/storage").await?;
    let tokens = crate::shell_cmd::attach_to_runtime(&coords).await?;
    let api_url = coords.api_url.clone();
    let client_token = tokens.client_token;
    runtime.set_wasm_bridge_spawner(std::sync::Arc::new(move |pipes| {
        elastos_server::carrier_bridge::spawn_wasm_api_bridge(
            pipes,
            api_url.clone(),
            client_token.clone(),
        );
    }));

    let _saved_termios = crate::shell_cmd::enable_host_raw_mode_pub();
    let _term_env = ScopedTerminalEnv::capture();
    let handle = runtime
        .run_local(capsule_dir, capsule_args)
        .await
        .map_err(|e| anyhow::anyhow!("WASM capsule failed: {}", e))?;
    eprintln!("[run] WASM capsule '{}' exited", handle.manifest.name);
    Ok(())
}

struct ScopedTerminalEnv {
    cols_prev: Option<std::ffi::OsString>,
    rows_prev: Option<std::ffi::OsString>,
}

impl ScopedTerminalEnv {
    fn capture() -> Self {
        let cols_prev = std::env::var_os("ELASTOS_TERM_COLS");
        let rows_prev = std::env::var_os("ELASTOS_TERM_ROWS");

        #[cfg(unix)]
        unsafe {
            let mut ws: libc::winsize = std::mem::zeroed();
            if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 {
                if ws.ws_col > 0 {
                    std::env::set_var("ELASTOS_TERM_COLS", ws.ws_col.to_string());
                }
                if ws.ws_row > 0 {
                    std::env::set_var("ELASTOS_TERM_ROWS", ws.ws_row.to_string());
                }
            }
        }

        Self {
            cols_prev,
            rows_prev,
        }
    }
}

impl Drop for ScopedTerminalEnv {
    fn drop(&mut self) {
        match &self.cols_prev {
            Some(value) => std::env::set_var("ELASTOS_TERM_COLS", value),
            None => std::env::remove_var("ELASTOS_TERM_COLS"),
        }
        match &self.rows_prev {
            Some(value) => std::env::set_var("ELASTOS_TERM_ROWS", value),
            None => std::env::remove_var("ELASTOS_TERM_ROWS"),
        }
    }
}
