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
                // Phase 8 Day 5 — pick the cheapest path that actually
                // works on this host. The operator-runtime lane needs a
                // running `elastos serve` daemon; for a freshly
                // installed standalone capsule (no `requires`, no
                // `providers`) that's an unnecessary prerequisite.
                // Detect "no daemon" via the coords-file check used by
                // `operator_runtime_coords`, and fall back to an
                // in-process boot through the same VzProvider the
                // supervisor would have used. Operators wanting the
                // managed lane explicitly should still run
                // `elastos serve` first; that's the production path.
                if operator_runtime_available().await {
                    return run_microvm_via_operator_runtime(manifest, &capsule_args).await;
                }
                return run_microvm_standalone(&capsule_dir, manifest).await;
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
        // Phase 8 Day 5 — when the operator types `elastos run
        // ubuntu-base` (single positional, no flags), clap binds
        // it as `path = ubuntu-base` and we fall through here.
        // If that path doesn't exist as written, try to resolve
        // it as a capsule name under `<data_dir>/capsules/<name>`
        // — the canonical location `elastos setup` installs to.
        // This gives v0.1 the "name-only, one-command" UX without
        // forcing operators to remember the full data-dir path.
        if !path.exists() {
            if let Some(resolved) = resolve_capsule_by_name(&path) {
                tracing::info!(
                    "Running capsule '{}' from data dir: {}",
                    path.display(),
                    resolved.display()
                );
                return Ok(resolved);
            }
        }
        tracing::info!("Running capsule from: {}", path.display());
        return Ok(path);
    }

    anyhow::bail!("Either path or --cid must be specified");
}

/// Phase 8 Day 5 — resolve `elastos run <name>` to the canonical
/// install path the setup loop writes to. We deliberately only
/// recognise paths with a single component (no slashes, no `..`):
/// anything more complex is treated verbatim because the operator
/// almost certainly meant a real filesystem path.
fn resolve_capsule_by_name(path: &Path) -> Option<PathBuf> {
    // A "name" is a single, simple component — no separators, no
    // path traversal, no prefix. Reject anything else so a typo
    // like `./ubuntu-base` doesn't silently get rewritten to the
    // data dir.
    if path.components().count() != 1 {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    let candidate = crate::default_data_dir().join("capsules").join(name);
    if candidate.join("capsule.json").is_file() {
        Some(candidate)
    } else {
        None
    }
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
    let tokens = crate::runtime_control::attach_to_runtime(&coords).await?;
    let api_url = coords.api_url.clone();
    let client_token = tokens.client_token;
    runtime.set_wasm_bridge_spawner(std::sync::Arc::new(move |pipes| {
        elastos_server::carrier_bridge::spawn_wasm_api_bridge(
            pipes,
            api_url.clone(),
            client_token.clone(),
        );
    }));

    let _saved_termios = crate::runtime_control::enable_host_raw_mode_pub();
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

// ── Phase 8 Day 5 — In-process microVM boot lane ───────────────────
//
// The operator-runtime lane (`run_microvm_via_operator_runtime`
// above) speaks HTTP to a long-running `elastos serve` daemon — the
// right design for multi-capsule production orchestration with
// shared identity / storage / signing. For the v0.1 "real Linux on
// Mac" demo bar that's overkill: one capsule, no requires, no
// providers. The standalone lane below boots that same capsule
// directly through a `VzProvider` we construct in this process,
// then streams the kernel console to stderr until Ctrl-C.
//
// Linux callers compile this lane out — `elastos-vz` is a no-op
// stub on non-macOS hosts and the lane has no meaning there. On
// macOS without Apple Silicon, `is_supported()` returns false and
// we surface a typed error pointing at `docs/MAC.md`.

/// Cheap check: is an `elastos serve` daemon writing the coord
/// file we'd need to dial? Used to pick between the operator and
/// standalone lanes without forcing the operator into either.
async fn operator_runtime_available() -> bool {
    let data_dir = crate::default_data_dir();
    let coords_path = crate::runtime_control::runtime_coord_path(&data_dir);
    crate::runtime_control::read_operator_runtime_coords(&coords_path)
        .await
        .is_some()
}

#[cfg(target_os = "macos")]
async fn run_microvm_standalone(
    capsule_dir: &Path,
    manifest: &elastos_common::CapsuleManifest,
) -> anyhow::Result<()> {
    use std::time::Duration;

    use elastos_compute::ComputeProvider;
    use elastos_vz::{VmConfig, VzConfig, VzProvider};

    if !elastos_vz::is_supported() {
        anyhow::bail!(
            "Apple Virtualization.framework not available on this host. \
             `elastos run <microvm>` standalone lane requires macOS 12+ on \
             Apple Silicon — see docs/MAC.md."
        );
    }

    let data_dir = crate::default_data_dir();
    let kernel_path = data_dir.join("bin/vmlinux");
    if !kernel_path.is_file() {
        anyhow::bail!(
            "guest kernel not found at {} — run `elastos setup --profile minimal` first",
            kernel_path.display()
        );
    }
    // Phase 8 Day 6 — go through the shared resolver so we pick
    // the writable-rootfs overlay variant when `elastos setup`
    // has built it, falling back to the pristine initrd
    // otherwise. Keeps the standalone lane symmetrical with the
    // supervisor managed lane.
    let initramfs_path_opt =
        elastos_server::overlay_initrd::resolve_initrd_path(&data_dir.join("bin"));

    let rootfs_path = capsule_dir.join(&manifest.entrypoint);
    if !rootfs_path.is_file() {
        anyhow::bail!(
            "rootfs not found at {} — re-run `elastos setup` for this capsule",
            rootfs_path.display()
        );
    }

    let microvm = manifest.microvm.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "capsule '{}' has type=microvm but no `microvm` block in capsule.json",
            manifest.name
        )
    })?;
    let boot_args = microvm.boot_args.clone();
    let vcpu_count = microvm.vcpu_count.unwrap_or(1);

    // Tempdir holds per-launch ephemeral state (matches the
    // `vm-debug boot` pattern — we want Ctrl-C to leave nothing
    // behind, and we don't yet have a CLI for persistent
    // standalone runs).
    let tmp = tempfile::tempdir()
        .map_err(|e| anyhow::anyhow!("standalone boot: cannot create tempdir: {e}"))?;

    let mut vz_config = VzConfig::new()
        .with_state_dir(tmp.path().join("vz"))
        .with_rootfs_cache_dir(tmp.path().join("rootfs-cache"))
        .with_kernel_path(kernel_path);
    if let Some(ref initramfs) = initramfs_path_opt {
        vz_config = vz_config.with_initramfs_path(initramfs.clone());
    }

    let provider = VzProvider::new(vz_config)
        .map_err(|e| anyhow::anyhow!("standalone boot: cannot construct VzProvider: {e}"))?;
    provider
        .init()
        .await
        .map_err(|e| anyhow::anyhow!("standalone boot: provider.init: {e}"))?;

    let vm_config = VmConfig {
        vm_id: format!("{}-standalone", manifest.name),
        kernel_path: data_dir.join("bin/vmlinux"),
        boot_args,
        rootfs_path,
        rootfs_readonly: true,
        mem_size_mib: manifest.resources.memory_mb,
        vcpu_count,
        http_port: microvm.http_port,
        data_disk_path: None,
        vsock_cid: 3,
        network: None,
        interactive_stdio: false,
        carrier_socket_path: None,
        initramfs_path: initramfs_path_opt,
    };

    eprintln!(
        "[run] booting microVM '{}' (in-process; no `elastos serve` daemon detected)",
        manifest.name
    );
    let handle = provider
        .load_with_vm_config(vm_config, manifest.clone())
        .await
        .map_err(|e| anyhow::anyhow!("standalone boot: provider.load: {e}"))?;
    eprintln!("[run] loaded ({}); starting…", handle.id);

    provider
        .start(&handle)
        .await
        .map_err(|e| anyhow::anyhow!("standalone boot: provider.start: {e}"))?;
    eprintln!(
        "[run] guest started. Press Ctrl-C to stop. \
         Guest kernel console streams via tracing target `vm_console`."
    );

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                eprintln!();
                eprintln!("[run] Ctrl-C received");
                break;
            }
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                match provider.status(&handle).await {
                    Ok(elastos_common::CapsuleStatus::Running) => continue,
                    Ok(other) => {
                        eprintln!("[run] guest state transitioned to {other:?}");
                        break;
                    }
                    Err(e) => {
                        eprintln!("[run] status query failed: {e}");
                        break;
                    }
                }
            }
        }
    }

    eprintln!("[run] stopping VM…");
    if let Err(e) = provider.stop(&handle).await {
        eprintln!("[run] provider.stop returned: {e}");
    }
    eprintln!("[run] done.");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
async fn run_microvm_standalone(
    _capsule_dir: &Path,
    _manifest: &elastos_common::CapsuleManifest,
) -> anyhow::Result<()> {
    // Linux callers reach this branch when a MicroVM capsule has
    // been pointed at, no operator-runtime daemon is running, and
    // we'd otherwise have no path forward. crosvm-based standalone
    // boot is on the Phase-8 backlog but not Day-5 scope; for now
    // the error tells the operator the standard managed path.
    anyhow::bail!(
        "`elastos run <microvm>` standalone lane is macOS-only on Phase 8 Day 5. \
         On Linux, start the runtime daemon first with `elastos serve` and re-run \
         to take the managed (crosvm) lane."
    )
}
