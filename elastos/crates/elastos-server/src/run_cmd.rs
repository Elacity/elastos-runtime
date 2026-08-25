use std::path::{Path, PathBuf};

use elastos_logger::log_info;

const LOG_COMPONENT: &str = "run";

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
                if !manifest.is_component_capsule() {
                    anyhow::bail!(
                        "WASI Preview 1 capsules are not supported; '{}' must use runtime_abi=\"elastos.component/v1\"",
                        manifest.name
                    );
                }
                return run_component_via_operator_runtime(&capsule_dir, manifest, capsule_args)
                    .await;
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
        log_info!(component: LOG_COMPONENT, "Running capsule from CID: {}", cid);
        let content_registry = crate::get_content_registry().await?;
        return elastos_server::content::prepare_capsule_from_content_provider(
            &content_registry,
            &cid,
        )
        .await;
    }

    if let Some(path) = path {
        log_info!(component: LOG_COMPONENT, "Running capsule from: {}", path.display());
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

async fn operator_runtime_coords() -> anyhow::Result<crate::runtime_control::RuntimeCoords> {
    let data_dir = crate::default_data_dir();
    let coords_path = crate::runtime_control::runtime_coord_path(&data_dir);
    crate::runtime_control::read_operator_runtime_coords(&coords_path)
        .await
        .ok_or_else(|| anyhow::anyhow!(crate::runtime_control::OPERATOR_RUNTIME_REQUIRED_MESSAGE))
}

async fn run_microvm_via_operator_runtime(
    manifest: &elastos_common::CapsuleManifest,
    capsule_args: &[String],
) -> anyhow::Result<()> {
    let coords = operator_runtime_coords().await?;
    eprintln!("[run] Attaching to runtime at {}", coords.api_url);

    let tokens = crate::runtime_control::attach_to_runtime(&coords).await?;
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
    let saved = crate::runtime_control::enable_host_raw_mode_pub();
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

async fn run_component_via_operator_runtime(
    capsule_dir: &Path,
    manifest: &elastos_common::CapsuleManifest,
    capsule_args: Vec<String>,
) -> anyhow::Result<()> {
    let coords = operator_runtime_coords().await?;
    eprintln!(
        "[run] Component capsule attached to runtime at {}",
        coords.api_url
    );

    let runtime = crate::create_runtime("/tmp/elastos/storage").await?;
    let tokens = crate::runtime_control::attach_to_runtime(&coords).await?;
    let api_url = coords.api_url.clone();
    let client_token = tokens.client_token;
    let manifest_capabilities = manifest.resource_authority_bounds();
    let audit_data_dir = crate::default_data_dir();
    let hostcall_handle = tokio::runtime::Handle::current();
    runtime.set_bridge_hostcall(std::sync::Arc::new(
        move |line, capsule_id, principal_id| {
            let response = hostcall_handle
                .block_on(
                    elastos_server::resource_bridge::handle_remote_request_with_audit_dir(
                        line,
                        &api_url,
                        &client_token,
                        capsule_id,
                        &manifest_capabilities,
                        principal_id,
                        Some(audit_data_dir.as_path()),
                    ),
                )
                .map_err(|err| err.to_string())?;
            serde_json::to_string(&response).map_err(|err| err.to_string())
        },
    ));

    let _saved_termios = crate::runtime_control::enable_host_raw_mode_pub();
    let _term_env = ScopedTerminalEnv::capture();
    let handle = runtime
        .run_local(capsule_dir, capsule_args)
        .await
        .map_err(|e| anyhow::anyhow!("Component capsule failed: {}", e))?;
    eprintln!("[run] Component capsule '{}' exited", handle.manifest.name);
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
