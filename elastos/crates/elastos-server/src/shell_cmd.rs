use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::Duration;

use elastos_server::local_http::LoopbackHttpBaseUrl;
use elastos_server::sources::{default_data_dir, OwnershipRepairGuard};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct RuntimeCoords {
    pub(crate) api_url: String,
    /// Opaque secret exchanged at `/api/auth/attach` for a short-lived bearer
    /// token.  Never used directly as a bearer — callers must call the attach
    /// endpoint first.
    pub(crate) attach_secret: String,
    /// Legacy fields — kept for deserialization of old coords files during
    /// the transition.  New code must use `attach_secret` instead.
    #[serde(default)]
    pub(crate) shell_token: String,
    #[serde(default)]
    pub(crate) client_token: String,
    pub(crate) pid: u32,
    #[serde(default = "default_runtime_kind")]
    pub(crate) runtime_kind: String,
}

pub(crate) const RUNTIME_KIND_OPERATOR: &str = "operator";
pub(crate) const RUNTIME_KIND_MANAGED_IDENTITY: &str = "managed-identity";
pub(crate) const RUNTIME_KIND_MANAGED_CHAT: &str = "managed-chat";
pub(crate) const RUNTIME_KIND_MANAGED_PC2: &str = "managed-pc2";
pub(crate) const OPERATOR_RUNTIME_REQUIRED_MESSAGE: &str =
    "This command requires a running runtime.\n\n  elastos serve\n\nThen run this command again.";

fn default_runtime_kind() -> String {
    RUNTIME_KIND_OPERATOR.to_string()
}

fn is_managed_user_runtime_kind(kind: &str) -> bool {
    matches!(
        kind,
        RUNTIME_KIND_MANAGED_IDENTITY | RUNTIME_KIND_MANAGED_CHAT | RUNTIME_KIND_MANAGED_PC2
    )
}

fn managed_runtime_kind_label(kind: &str) -> &str {
    match kind {
        RUNTIME_KIND_MANAGED_IDENTITY => "identity",
        RUNTIME_KIND_MANAGED_CHAT => "chat",
        RUNTIME_KIND_MANAGED_PC2 => "pc2",
        _ => kind,
    }
}

impl RuntimeCoords {
    pub(crate) fn is_operator_runtime(&self) -> bool {
        !is_managed_user_runtime_kind(&self.runtime_kind)
    }
}

struct SavedTermios(libc::termios);

fn enable_host_raw_mode() -> Option<SavedTermios> {
    unsafe {
        let mut original: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(libc::STDIN_FILENO, &mut original) != 0 {
            return None;
        }

        let saved = SavedTermios(original);
        let mut raw = original;

        raw.c_iflag &= !(libc::ICRNL
            | libc::IXON
            | libc::BRKINT
            | libc::INPCK
            | libc::ISTRIP
            | libc::IXOFF
            | libc::INLCR
            | libc::IGNCR
            | libc::PARMRK);
        raw.c_oflag &= !libc::OPOST;
        raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN);
        // Keep ISIG so Ctrl+C generates SIGINT even in raw mode.
        raw.c_cflag &= !(libc::CSIZE | libc::PARENB);
        raw.c_cflag |= libc::CS8;
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;

        if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) != 0 {
            return None;
        }

        Some(saved)
    }
}

fn restore_host_terminal(saved: SavedTermios) {
    unsafe {
        libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &saved.0);
    }
}

/// Public wrapper for raw mode — used by run_cmd, chat_cmd, and pc2_cmd.
/// Returns a guard that restores the terminal on drop.
pub(crate) fn enable_host_raw_mode_pub() -> Option<TermiosGuard> {
    enable_host_raw_mode().map(|s| TermiosGuard(Some(s)))
}

pub(crate) struct TermiosGuard(Option<SavedTermios>);

impl Drop for TermiosGuard {
    fn drop(&mut self) {
        if let Some(saved) = self.0.take() {
            restore_host_terminal(saved);
        }
    }
}

pub(crate) fn runtime_coord_path(data_dir: &Path) -> PathBuf {
    if let Some(path) = std::env::var_os("ELASTOS_RUNTIME_COORDS_FILE") {
        return PathBuf::from(path);
    }
    data_dir.join("runtime-coords.json")
}

fn pc2_runtime_coord_path(data_dir: &Path) -> PathBuf {
    data_dir.join("pc2-runtime-coords.json")
}

pub(crate) fn write_runtime_coords(path: &Path, coords: &RuntimeCoords) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_vec_pretty(coords)?;
    std::fs::write(path, &data)?;
    // Restrict to owner-only (contains attach secret).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub(crate) async fn read_runtime_coords(path: &Path) -> Option<RuntimeCoords> {
    let data = std::fs::read(path).ok()?;
    let coords: RuntimeCoords = serde_json::from_slice(&data).ok()?;
    let api_base = match LoopbackHttpBaseUrl::parse(&coords.api_url) {
        Ok(api_base) => api_base,
        Err(_) => {
            let _ = std::fs::remove_file(path);
            return None;
        }
    };

    if !PathBuf::from(format!("/proc/{}", coords.pid)).exists() {
        let _ = std::fs::remove_file(path);
        return None;
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .ok()?;

    let healthy = client
        .get(api_base.join("/api/health").ok()?)
        .send()
        .await
        .ok()
        .is_some_and(|r| r.status().is_success());
    if !healthy {
        let _ = std::fs::remove_file(path);
        return None;
    }

    if !attach_secret_matches(&client, &api_base, &coords.attach_secret).await {
        let _ = std::fs::remove_file(path);
        return None;
    }

    Some(coords)
}

async fn attach_secret_matches(
    client: &reqwest::Client,
    api_base: &LoopbackHttpBaseUrl,
    attach_secret: &str,
) -> bool {
    if attach_secret.is_empty() {
        return true;
    }

    client
        .post(match api_base.join("/api/auth/attach") {
            Ok(url) => url,
            Err(_) => return false,
        })
        .json(&serde_json::json!({
            "secret": attach_secret,
            "scope": "client",
        }))
        .send()
        .await
        .ok()
        .is_some_and(|resp| resp.status().is_success())
}

async fn runtime_version_from_health(api_url: &str) -> Option<String> {
    let api_base = LoopbackHttpBaseUrl::parse(api_url).ok()?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .ok()?;

    let resp = client
        .get(api_base.join("/api/health").ok()?)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }

    let json: serde_json::Value = resp.json().await.ok()?;
    json.get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

async fn terminate_managed_chat_runtime(coords: &RuntimeCoords, coords_path: &Path) {
    let _ = std::fs::remove_file(coords_path);

    #[cfg(unix)]
    unsafe {
        libc::kill(coords.pid as i32, libc::SIGTERM);
    }

    let proc_path = PathBuf::from(format!("/proc/{}", coords.pid));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while proc_path.exists() && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    #[cfg(unix)]
    if proc_path.exists() {
        unsafe {
            libc::kill(coords.pid as i32, libc::SIGKILL);
        }
        let kill_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while proc_path.exists() && std::time::Instant::now() < kill_deadline {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
}

pub(crate) async fn read_operator_runtime_coords(path: &Path) -> Option<RuntimeCoords> {
    let coords = read_runtime_coords(path).await?;
    if coords.is_operator_runtime() {
        Some(coords)
    } else {
        None
    }
}

/// Tokens returned by the attach endpoint.
pub(crate) struct AttachTokens {
    pub(crate) shell_token: String,
    pub(crate) client_token: String,
}

/// Exchange the attach secret for short-lived session tokens.
///
/// Calls `/api/auth/attach` twice — once for shell scope, once for client scope.
pub(crate) async fn attach_to_runtime(coords: &RuntimeCoords) -> anyhow::Result<AttachTokens> {
    if coords.attach_secret.is_empty() {
        anyhow::bail!(
            "runtime coords missing attach_secret; restart the runtime with:\n  elastos serve"
        );
    }

    let api_base = LoopbackHttpBaseUrl::parse(&coords.api_url).map_err(|e| {
        anyhow::anyhow!(
            "runtime API URL must remain local-only for attach/session control: {}",
            e
        )
    })?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let shell_token = call_attach(&client, &api_base, &coords.attach_secret, "shell").await?;
    let client_token = call_attach(&client, &api_base, &coords.attach_secret, "client").await?;

    Ok(AttachTokens {
        shell_token,
        client_token,
    })
}

pub(crate) async fn attach_client_token_to_operator_runtime(
    data_dir: &Path,
) -> anyhow::Result<(RuntimeCoords, String)> {
    let coords_path = runtime_coord_path(data_dir);
    let coords = read_operator_runtime_coords(&coords_path)
        .await
        .ok_or_else(|| anyhow::anyhow!(OPERATOR_RUNTIME_REQUIRED_MESSAGE))?;
    let tokens = attach_to_runtime(&coords).await?;
    Ok((coords, tokens.client_token))
}

async fn call_attach(
    client: &reqwest::Client,
    api_base: &LoopbackHttpBaseUrl,
    secret: &str,
    scope: &str,
) -> anyhow::Result<String> {
    let resp = client
        .post(api_base.join("/api/auth/attach")?)
        .json(&serde_json::json!({
            "secret": secret,
            "scope": scope,
        }))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("attach failed ({}): {}", status, body);
    }

    let json: serde_json::Value = resp.json().await?;
    json.get("token")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("attach response missing token"))
}

async fn supervisor_post(
    client: &reqwest::Client,
    api_base: &LoopbackHttpBaseUrl,
    token: &str,
    path: &str,
    body: serde_json::Value,
    timeout: Option<Duration>,
) -> anyhow::Result<serde_json::Value> {
    let url = api_base.join(path)?;
    let mut req = client.post(url).bearer_auth(token).json(&body);
    if let Some(timeout) = timeout {
        req = req.timeout(timeout);
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("supervisor endpoint {} failed ({}): {}", path, status, body);
    }
    Ok(resp.json::<serde_json::Value>().await?)
}

async fn dispatch_via_existing_runtime(
    coords: &RuntimeCoords,
    command: serde_json::Value,
) -> anyhow::Result<()> {
    const SUPERVISOR_PREPARE_TIMEOUT: Duration = Duration::from_secs(300);

    let client = reqwest::Client::new();
    let api_base = LoopbackHttpBaseUrl::parse(&coords.api_url).map_err(|e| {
        anyhow::anyhow!(
            "runtime API URL must remain local-only for supervisor attach/control: {}",
            e
        )
    })?;

    eprintln!(
        "[runtime] Attaching to existing runtime at {}",
        api_base.as_str()
    );

    let tokens = attach_to_runtime(coords).await?;

    let _ = supervisor_post(
        &client,
        &api_base,
        &tokens.shell_token,
        "/api/supervisor/ensure-capsule",
        serde_json::json!({ "name": "shell" }),
        Some(SUPERVISOR_PREPARE_TIMEOUT),
    )
    .await?;

    let launch = supervisor_post(
        &client,
        &api_base,
        &tokens.shell_token,
        "/api/supervisor/launch-capsule",
        serde_json::json!({
            "name": "shell",
            "config": command,
        }),
        Some(SUPERVISOR_PREPARE_TIMEOUT),
    )
    .await?;

    let handle = launch
        .get("handle")
        .and_then(|h| h.as_str())
        .ok_or_else(|| anyhow::anyhow!("launch response missing handle"))?;

    let _ = supervisor_post(
        &client,
        &api_base,
        &tokens.shell_token,
        "/api/supervisor/wait-capsule",
        serde_json::json!({ "handle": handle }),
        None,
    )
    .await?;

    Ok(())
}

/// Ensure a runtime is running for native chat. If none exists, starts a
/// managed background runtime with a chat-safe policy (auto-approves peer,
/// did, and storage capabilities). Returns the runtime coords.
///
/// This is the one-terminal chat bootstrap: `elastos chat` works without
/// requiring a separate `elastos serve` terminal.
pub(crate) async fn ensure_runtime_for_chat(data_dir: &Path) -> anyhow::Result<RuntimeCoords> {
    if let Some(coords) = reuse_pc2_runtime_for_chat(data_dir).await {
        return Ok(coords);
    }

    ensure_managed_runtime(
        data_dir,
        RUNTIME_KIND_MANAGED_CHAT,
        "chat-auto.json",
        managed_chat_allow_resources(),
        "chat",
    )
    .await
}

async fn reuse_pc2_runtime_for_chat(data_dir: &Path) -> Option<RuntimeCoords> {
    let coords = read_runtime_coords(&pc2_runtime_coord_path(data_dir)).await?;
    if coords.runtime_kind != RUNTIME_KIND_MANAGED_PC2 {
        return None;
    }

    let expected = env!("ELASTOS_VERSION");
    match runtime_version_from_health(&coords.api_url).await {
        Some(actual) if actual == expected => {
            if runtime_notices_enabled() {
                eprintln!("Reusing local pc2 runtime for chat.");
            }
            Some(coords)
        }
        _ => None,
    }
}

/// Ensure a runtime is running for local identity/profile management.
/// This keeps the approval surface narrow: only DID provider operations
/// are auto-approved for the managed helper runtime.
pub(crate) async fn ensure_runtime_for_identity(data_dir: &Path) -> anyhow::Result<RuntimeCoords> {
    ensure_managed_runtime(
        data_dir,
        RUNTIME_KIND_MANAGED_IDENTITY,
        "identity-auto.json",
        &["elastos://did/*"],
        "identity",
    )
    .await
}

/// Ensure a runtime is running for the local PC2 dashboard. Like native chat,
/// this is a managed one-terminal bootstrap that hides runtime plumbing from
/// the user-facing home surface.
pub(crate) async fn ensure_runtime_for_pc2(data_dir: &Path) -> anyhow::Result<RuntimeCoords> {
    ensure_managed_runtime(
        data_dir,
        RUNTIME_KIND_MANAGED_PC2,
        "pc2-auto.json",
        managed_pc2_allow_resources(),
        "pc2",
    )
    .await
}

fn managed_chat_allow_resources() -> &'static [&'static str] {
    &[
        "elastos://peer/*",
        "elastos://did/*",
        "localhost://Users/self/.AppData/LocalHost/Chat/*",
    ]
}

fn managed_pc2_allow_resources() -> &'static [&'static str] {
    &[
        "elastos://peer/*",
        "elastos://did/*",
        "localhost://Users/self/.AppData/LocalHost/Chat/*",
        "localhost://Users/self/.AppData/LocalHost/GBA/*",
        "localhost://Local/SharedByLocalUsersAndBots/PC2/*",
    ]
}

fn runtime_notices_enabled() -> bool {
    std::env::var("ELASTOS_QUIET_RUNTIME_NOTICES")
        .ok()
        .as_deref()
        != Some("1")
}

async fn ensure_managed_runtime(
    data_dir: &Path,
    runtime_kind: &str,
    policy_file_name: &str,
    allow_resources: &[&str],
    surface_name: &str,
) -> anyhow::Result<RuntimeCoords> {
    let coords_path = runtime_coord_path(data_dir);

    // 1. Check for existing runtime (operator-started or previous chat-started)
    if let Some(coords) = read_runtime_coords(&coords_path).await {
        if is_managed_user_runtime_kind(&coords.runtime_kind) {
            if coords.runtime_kind != runtime_kind {
                if runtime_notices_enabled() {
                    eprintln!(
                        "Managed {} runtime cannot satisfy {}. Restarting with the correct local policy...",
                        managed_runtime_kind_label(&coords.runtime_kind),
                        surface_name
                    );
                }
                terminate_managed_chat_runtime(&coords, &coords_path).await;
            } else {
                let expected = env!("ELASTOS_VERSION");
                // Dev-build restart disabled: multiple surfaces (native chat +
                // WASM chat) share the same managed runtime. Restarting would
                // kill all existing sessions. Always reuse the running runtime.
                {
                    match runtime_version_from_health(&coords.api_url).await {
                        Some(actual) if actual == expected => return Ok(coords),
                        Some(actual) => {
                            if runtime_notices_enabled() {
                                eprintln!(
                                "Managed {} runtime is stale (running {}, expected {}). Restarting...",
                                surface_name, actual, expected
                            );
                            }
                            terminate_managed_chat_runtime(&coords, &coords_path).await;
                        }
                        None => {
                            if runtime_notices_enabled() {
                                eprintln!(
                                "Managed {} runtime version unknown (expected {}). Restarting...",
                                surface_name, expected
                            );
                            }
                            terminate_managed_chat_runtime(&coords, &coords_path).await;
                        }
                    }
                }
            }
        } else {
            return Ok(coords);
        }
    }

    if runtime_notices_enabled() {
        eprintln!(
            "No runtime found. Starting local {} runtime...",
            surface_name
        );
    }

    // 2. Check that the minimal host provider binaries exist.
    //    Native chat needs: localhost-provider (storage), shell (capability approval),
    //    and did-provider (identity). These are provisioned by `elastos setup`.
    //    components.json alone is not sufficient — it's written by install.sh before
    //    any provider binaries are installed.
    let mut missing = Vec::new();
    for name in &["localhost-provider", "shell", "did-provider"] {
        if crate::find_installed_provider_binary(name).is_none() {
            missing.push(*name);
        }
    }
    if !missing.is_empty() {
        anyhow::bail!(
            "{} prerequisites not installed: {}\n\n\
             Run first:\n\n\
             \x20 elastos setup\n\n\
             Then try again.",
            surface_name.to_ascii_uppercase(),
            missing.join(", ")
        );
    }

    // Find the runtime binary (same binary we're running from)
    let self_exe = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("Failed to determine runtime binary: {}", e))?;
    let managed_addr = reserve_managed_chat_addr()
        .map_err(|e| anyhow::anyhow!("Failed to reserve managed runtime port: {}", e))?;

    // Create a chat-scoped policy for the managed runtime.
    // This auto-approves only the capabilities native chat needs.
    // Other commands (share, agent, etc.) that need broader capabilities
    // should use `elastos serve` with operator-configured policy.
    let policy_dir = data_dir.join("policy");
    std::fs::create_dir_all(&policy_dir)?;
    let policy_path = policy_dir.join(policy_file_name);
    let policy = serde_json::json!({
        "allow": allow_resources
    });
    std::fs::write(&policy_path, serde_json::to_string_pretty(&policy)?)?;

    // 4. Create log directory
    let log_dir = data_dir.join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join("runtime.log");
    let log_file = std::fs::File::create(&log_path)
        .map_err(|e| anyhow::anyhow!("Failed to create runtime log: {}", e))?;

    // 5. Spawn managed runtime as detached background process
    let mut child = std::process::Command::new(&self_exe)
        .arg("serve")
        .arg("--addr")
        .arg(&managed_addr)
        .env("ELASTOS_POLICY_FILE", &policy_path)
        .env("ELASTOS_SHELL_MODE", "agent")
        .env("ELASTOS_RUNTIME_KIND", runtime_kind)
        .stdout(std::process::Stdio::from(log_file.try_clone()?))
        .stderr(std::process::Stdio::from(log_file))
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to start runtime: {}", e))?;

    if runtime_notices_enabled() {
        eprintln!(
            "Runtime started (pid {}). Log: {}",
            child.id(),
            log_path.display()
        );
    }

    // 6. Wait for runtime to become ready (coords file + health check)
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| anyhow::anyhow!("Failed to check runtime status: {}", e))?
        {
            anyhow::bail!(
                "{}",
                format_runtime_start_exit_message(surface_name, &log_path, status)
            );
        }
        if std::time::Instant::now() > deadline {
            anyhow::bail!(
                "{}",
                format_runtime_start_timeout_message(surface_name, &log_path)
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if let Some(coords) = read_runtime_coords(&coords_path).await {
            return Ok(coords);
        }
    }
}

fn format_runtime_start_exit_message(
    surface_name: &str,
    log_path: &Path,
    status: ExitStatus,
) -> String {
    let exit = status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "signal".to_string());

    if let Some(summary) = summarize_runtime_start_failure(log_path) {
        format!(
            "{} runtime exited before becoming ready (exit {}).\n{}\nCheck log: {}",
            surface_name,
            exit,
            summary,
            log_path.display()
        )
    } else {
        format!(
            "{} runtime exited before becoming ready (exit {}).\nCheck log: {}",
            surface_name,
            exit,
            log_path.display()
        )
    }
}

fn format_runtime_start_timeout_message(surface_name: &str, log_path: &Path) -> String {
    if let Some(summary) = summarize_runtime_start_failure(log_path) {
        format!(
            "{} runtime did not become ready within 30s.\n{}\nCheck log: {}",
            surface_name,
            summary,
            log_path.display()
        )
    } else {
        format!(
            "{} runtime did not become ready within 30s.\nCheck log: {}",
            surface_name,
            log_path.display()
        )
    }
}

fn summarize_runtime_start_failure(log_path: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(log_path).ok()?;
    summarize_runtime_start_failure_contents(&contents)
}

fn summarize_runtime_start_failure_contents(contents: &str) -> Option<String> {
    let last_line = contents
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())?;

    if last_line.contains("failed checksum verification against")
        && last_line.contains("components.json")
    {
        return Some(format!(
            "Installed core components are out of sync with the stamped manifest.\nRun `elastos setup --profile pc2` (or `elastos setup`) to reconcile installed support assets, then retry.\nLast runtime error: {}",
            last_line
        ));
    }

    Some(format!("Last runtime error: {}", last_line))
}

fn reserve_managed_chat_addr() -> std::io::Result<String> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    Ok(format!("127.0.0.1:{}", addr.port()))
}

/// Forward a command to the shell via the supervisor path.
///
/// This is an operator-runtime command — it requires `elastos serve` to be
/// running. It does NOT auto-start a managed runtime. If the runtime is not
/// running, it fails fast with guidance.
pub(crate) async fn forward_to_shell(command: serde_json::Value) -> anyhow::Result<()> {
    let data_dir = default_data_dir();
    let _ownership_guard = OwnershipRepairGuard::new(data_dir.clone());
    let coords_path = runtime_coord_path(&data_dir);

    if let Some(coords) = read_operator_runtime_coords(&coords_path).await {
        return dispatch_via_existing_runtime(&coords, command).await;
    }

    // No running runtime — fail clearly. This is an operator-runtime command.
    anyhow::bail!(OPERATOR_RUNTIME_REQUIRED_MESSAGE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Json, Router};

    #[test]
    fn runtime_start_failure_summary_surfaces_setup_guidance_for_checksum_mismatch() {
        let summary = summarize_runtime_start_failure_contents(
            "2026-03-24T00:00:00Z  INFO boot\nError: installed component 'localhost-provider' at /tmp/x failed checksum verification against /tmp/components.json",
        )
        .unwrap();

        assert!(summary.contains("Run `elastos setup --profile pc2`"));
        assert!(
            summary.contains("Last runtime error: Error: installed component 'localhost-provider'")
        );
    }

    #[test]
    fn runtime_start_failure_summary_uses_last_nonempty_line() {
        let summary = summarize_runtime_start_failure_contents(
            "line one\n\nError: something else went wrong\n",
        )
        .unwrap();

        assert_eq!(
            summary,
            "Last runtime error: Error: something else went wrong"
        );
    }

    #[tokio::test]
    async fn runtime_version_from_health_returns_reported_version() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/api/health",
            get(|| async {
                Json(serde_json::json!({
                    "status": "ok",
                    "version": "0.20.0-rc99",
                }))
            }),
        );

        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let version = runtime_version_from_health(&format!("http://{}", addr)).await;
        assert_eq!(version.as_deref(), Some("0.20.0-rc99"));

        server.abort();
    }

    #[tokio::test]
    async fn runtime_version_from_health_rejects_non_loopback_url() {
        let version = runtime_version_from_health("https://example.com").await;
        assert!(version.is_none());
    }

    #[test]
    fn managed_pc2_policy_includes_gba_localhost_root() {
        let allow = managed_pc2_allow_resources();
        assert!(allow.contains(&"localhost://Users/self/.AppData/LocalHost/GBA/*"));
    }

    #[test]
    fn pc2_runtime_coord_path_uses_data_dir_and_not_env_override() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var(
            "ELASTOS_RUNTIME_COORDS_FILE",
            tmp.path().join("override.json"),
        );
        let actual = pc2_runtime_coord_path(tmp.path());
        std::env::remove_var("ELASTOS_RUNTIME_COORDS_FILE");
        assert_eq!(actual, tmp.path().join("pc2-runtime-coords.json"));
    }
}
