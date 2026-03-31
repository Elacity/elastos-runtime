use anyhow::{anyhow, bail};
use base64::Engine as _;
use elastos_server::sources::default_data_dir;
use std::path::{Path, PathBuf};

fn parse_config_object(
    name: &str,
    config: Option<String>,
) -> anyhow::Result<serde_json::Map<String, serde_json::Value>> {
    let value = match config {
        Some(raw) => serde_json::from_str::<serde_json::Value>(&raw)
            .map_err(|e| anyhow!("Invalid --config JSON for capsule '{}': {}", name, e))?,
        None => serde_json::json!({}),
    };

    match value {
        serde_json::Value::Object(map) => Ok(map),
        _ => bail!(
            "Invalid --config JSON for capsule '{}': expected a JSON object payload",
            name
        ),
    }
}

async fn runtime_for_capsule(
    interactive_surface: bool,
) -> anyhow::Result<crate::shell_cmd::RuntimeCoords> {
    let data_dir = default_data_dir();
    let coords_path = crate::shell_cmd::runtime_coord_path(&data_dir);

    if interactive_surface {
        if let Some(coords) = crate::shell_cmd::read_runtime_coords(&coords_path).await {
            if coords.is_operator_runtime()
                || matches!(
                    coords.runtime_kind.as_str(),
                    crate::shell_cmd::RUNTIME_KIND_MANAGED_CHAT
                        | crate::shell_cmd::RUNTIME_KIND_MANAGED_PC2
                )
            {
                return Ok(coords);
            }
        }
        return crate::shell_cmd::ensure_runtime_for_pc2(&data_dir).await;
    }

    crate::shell_cmd::read_operator_runtime_coords(&coords_path)
        .await
        .ok_or_else(|| anyhow!(crate::shell_cmd::OPERATOR_RUNTIME_REQUIRED_MESSAGE))
}

async fn supervisor_post(
    client: &reqwest::Client,
    api_url: &str,
    token: &str,
    path: &str,
    body: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let response = client
        .post(format!("{}{}", api_url.trim_end_matches('/'), path))
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .await?;

    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        bail!("supervisor endpoint {} failed ({}): {}", path, status, text);
    }

    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| anyhow!("supervisor endpoint {} returned invalid JSON: {}", path, e))?;
    if json.get("status").and_then(|value| value.as_str()) != Some("ok") {
        bail!(
            "supervisor endpoint {} returned error: {}",
            path,
            json.get("error")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown")
        );
    }
    Ok(json)
}

fn load_capsule_manifest(
    capsule_dir: &Path,
    name: &str,
) -> anyhow::Result<elastos_common::CapsuleManifest> {
    let manifest_path = capsule_dir.join("capsule.json");
    let data = std::fs::read_to_string(&manifest_path)
        .map_err(|e| anyhow!("Failed to read capsule manifest for '{}': {}", name, e))?;
    let manifest: elastos_common::CapsuleManifest = serde_json::from_str(&data)
        .map_err(|e| anyhow!("Invalid capsule manifest for '{}': {}", name, e))?;
    manifest
        .validate()
        .map_err(|e| anyhow!("Invalid capsule manifest for '{}': {}", name, e))?;
    Ok(manifest)
}

struct ScopedEnvVar {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
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

async fn run_wasm_capsule(
    capsule_dir: PathBuf,
    coords: &crate::shell_cmd::RuntimeCoords,
    client_token: String,
    config: &serde_json::Map<String, serde_json::Value>,
    interactive: bool,
) -> anyhow::Result<()> {
    let runtime = crate::create_runtime(default_data_dir().join("storage")).await?;
    let api_url = coords.api_url.clone();
    runtime.set_wasm_bridge_spawner(std::sync::Arc::new(move |pipes| {
        elastos_server::carrier_bridge::spawn_wasm_api_bridge(
            pipes,
            api_url.clone(),
            client_token.clone(),
        );
    }));

    let config_json = serde_json::Value::Object(config.clone());
    let config_text = serde_json::to_string(&config_json)?;
    let config_b64 = base64::engine::general_purpose::STANDARD.encode(config_text.as_bytes());
    let _command_env = ScopedEnvVar::set("ELASTOS_COMMAND", &config_text);
    let _command_b64_env = ScopedEnvVar::set("ELASTOS_COMMAND_B64", &config_b64);
    let _term_env = ScopedTerminalEnv::capture();
    let _saved_termios = interactive.then(crate::shell_cmd::enable_host_raw_mode_pub);

    runtime
        .run_local(&capsule_dir, Vec::new())
        .await
        .map_err(|e| anyhow!("WASM capsule failed: {}", e))?;

    Ok(())
}

pub async fn run_capsule(
    name: String,
    config: Option<String>,
    lifecycle: String,
    interactive: bool,
) -> anyhow::Result<()> {
    let interactive_surface = interactive || lifecycle.trim().eq_ignore_ascii_case("interactive");
    let target_name = name.clone();
    let coords = runtime_for_capsule(interactive_surface).await?;
    let tokens = crate::shell_cmd::attach_to_runtime(&coords).await?;

    let client = reqwest::Client::new();
    let resolve = supervisor_post(
        &client,
        &coords.api_url,
        &tokens.shell_token,
        "/api/supervisor/resolve-plan",
        serde_json::json!({ "target": target_name }),
    )
    .await?;

    let capsules = resolve
        .get("capsules")
        .and_then(|value| value.as_array())
        .ok_or_else(|| anyhow!("resolve-plan response missing capsules"))?;
    let externals = resolve
        .get("externals")
        .and_then(|value| value.as_array())
        .ok_or_else(|| anyhow!("resolve-plan response missing externals"))?;

    let platform = crate::setup::detect_platform();
    let mut ensured_paths = std::collections::HashMap::<String, PathBuf>::new();
    for external in externals {
        let external_name = external
            .as_str()
            .ok_or_else(|| anyhow!("resolve-plan externals entry must be a string"))?;
        let _ = supervisor_post(
            &client,
            &coords.api_url,
            &tokens.shell_token,
            "/api/supervisor/ensure-external",
            serde_json::json!({ "name": external_name, "platform": platform }),
        )
        .await?;
    }
    for capsule in capsules {
        let capsule_name = capsule
            .as_str()
            .ok_or_else(|| anyhow!("resolve-plan capsules entry must be a string"))?;
        let ensured = supervisor_post(
            &client,
            &coords.api_url,
            &tokens.shell_token,
            "/api/supervisor/ensure-capsule",
            serde_json::json!({ "name": capsule_name }),
        )
        .await?;
        if let Some(path) = ensured.get("path").and_then(|value| value.as_str()) {
            ensured_paths.insert(capsule_name.to_string(), PathBuf::from(path));
        }
    }

    let mut config = parse_config_object(&name, config)?;
    if interactive {
        config.insert("_elastos_interactive".into(), serde_json::json!(true));
    }

    let capsule_dir = ensured_paths
        .get(&name)
        .cloned()
        .ok_or_else(|| anyhow!("ensure-capsule response missing path for '{}'", name))?;
    let manifest = load_capsule_manifest(&capsule_dir, &name)?;
    if manifest.capsule_type == elastos_common::CapsuleType::Wasm {
        return run_wasm_capsule(
            capsule_dir,
            &coords,
            tokens.client_token,
            &config,
            interactive_surface,
        )
        .await;
    }

    let launch = supervisor_post(
        &client,
        &coords.api_url,
        &tokens.shell_token,
        "/api/supervisor/launch-capsule",
        serde_json::json!({
            "name": name,
            "config": serde_json::Value::Object(config),
        }),
    )
    .await?;

    let handle = launch
        .get("handle")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow!("launch response missing handle"))?;

    let _ = supervisor_post(
        &client,
        &coords.api_url,
        &tokens.shell_token,
        "/api/supervisor/wait-capsule",
        serde_json::json!({ "handle": handle }),
    )
    .await?;

    Ok(())
}
