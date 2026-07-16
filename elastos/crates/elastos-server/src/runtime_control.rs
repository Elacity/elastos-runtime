use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::Duration;

use crate::local_http::LoopbackHttpBaseUrl;
use sha2::Digest;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RuntimeCoords {
    pub api_url: String,
    /// Opaque secret exchanged at `/api/auth/attach` for a short-lived bearer
    /// token.  Never used directly as a bearer — callers must call the attach
    /// endpoint first.
    pub attach_secret: String,
    pub pid: u32,
    #[serde(default = "default_runtime_kind")]
    pub runtime_kind: String,
    #[serde(default)]
    pub binary_sha256: String,
    #[serde(default)]
    pub policy_sha256: String,
    #[serde(default)]
    pub dependency_sha256: String,
}

pub const RUNTIME_KIND_OPERATOR: &str = "operator";
pub const RUNTIME_KIND_MANAGED_IDENTITY: &str = "managed-identity";
pub const RUNTIME_KIND_MANAGED_CHAT: &str = "managed-chat";
pub const RUNTIME_KIND_MANAGED_HOME: &str = "managed-home";
pub const OPERATOR_RUNTIME_REQUIRED_MESSAGE: &str =
    "This command requires a running runtime.\n\n  elastos serve\n\nThen run this command again.";
pub(crate) const GATEWAY_OWNED_HOME_TERMINAL_ENV: &str = "ELASTOS_GATEWAY_OWNED_HOME_TERMINAL";
const CARRIER_MDNS_ENV: &str = "ELASTOS_CARRIER_MDNS";
const MANAGED_RUNTIME_REQUIRED_PROVIDERS: &[&str] =
    &["localhost-provider", "shell", "did-provider"];

enum ManagedRuntimeStart {
    Owner(ManagedRuntimeStartGuard),
    Waiter,
}

struct ManagedRuntimeStartGuard {
    path: PathBuf,
    _file: std::fs::File,
}

fn default_runtime_kind() -> String {
    RUNTIME_KIND_OPERATOR.to_string()
}

fn managed_runtime_default_carrier_mdns(runtime_kind: &str) -> Option<&'static str> {
    match runtime_kind {
        RUNTIME_KIND_MANAGED_CHAT | RUNTIME_KIND_MANAGED_HOME | RUNTIME_KIND_MANAGED_IDENTITY => {
            Some("0")
        }
        _ => None,
    }
}

fn managed_runtime_start_lock_path(data_dir: &Path) -> PathBuf {
    data_dir.join("managed-runtime-start.lock")
}

fn acquire_managed_runtime_start_lock(data_dir: &Path) -> anyhow::Result<ManagedRuntimeStart> {
    std::fs::create_dir_all(data_dir)?;
    let path = managed_runtime_start_lock_path(data_dir);
    match create_managed_runtime_start_marker(&path) {
        Ok(file) => Ok(ManagedRuntimeStart::Owner(ManagedRuntimeStartGuard {
            path,
            _file: file,
        })),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            if managed_runtime_start_marker_is_stale(&path) {
                let _ = std::fs::remove_file(&path);
                let file = create_managed_runtime_start_marker(&path).map_err(|retry| {
                    anyhow::anyhow!(
                        "create managed runtime start lock {} after stale cleanup: {}",
                        path.display(),
                        retry
                    )
                })?;
                return Ok(ManagedRuntimeStart::Owner(ManagedRuntimeStartGuard {
                    path,
                    _file: file,
                }));
            }
            Ok(ManagedRuntimeStart::Waiter)
        }
        Err(err) => Err(anyhow::anyhow!(
            "create managed runtime start lock {}: {}",
            path.display(),
            err
        )),
    }
}

fn create_managed_runtime_start_marker(path: &Path) -> std::io::Result<std::fs::File> {
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)?;
    use std::io::Write as _;
    writeln!(
        file,
        "{{\"pid\":{},\"created_at_ms\":{}}}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default()
    )?;
    Ok(file)
}

fn managed_runtime_start_marker_is_stale(path: &Path) -> bool {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed > Duration::from_secs(60))
}

impl Drop for ManagedRuntimeStartGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

async fn wait_for_managed_runtime_ready(
    coords_path: &Path,
    expected_version: &str,
    timeout: Duration,
) -> Option<RuntimeCoords> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(coords) = managed_runtime_ready(coords_path, expected_version).await {
            return Some(coords);
        }
        if std::time::Instant::now() > deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn current_process_managed_runtime_child(runtime_kind: &str) -> Option<u32> {
    #[cfg(unix)]
    {
        current_process_child_pids()
            .into_iter()
            .find(|child_pid| process_env_contains_runtime_kind(*child_pid, runtime_kind))
    }

    #[cfg(not(unix))]
    {
        let _ = runtime_kind;
        None
    }
}

#[cfg(target_os = "linux")]
fn current_process_child_pids() -> Vec<u32> {
    let pid = std::process::id();
    let task_dir = format!("/proc/{pid}/task");
    let Ok(tasks) = std::fs::read_dir(task_dir) else {
        return Vec::new();
    };
    let mut children = Vec::new();
    for task in tasks.flatten() {
        let children_path = task.path().join("children");
        let Ok(contents) = std::fs::read_to_string(children_path) else {
            continue;
        };
        children.extend(
            contents
                .split_whitespace()
                .filter_map(|value| value.parse::<u32>().ok()),
        );
    }
    children.sort_unstable();
    children.dedup();
    children
}

#[cfg(all(unix, not(target_os = "linux")))]
fn current_process_child_pids() -> Vec<u32> {
    let parent_pid = std::process::id();
    let Ok(output) = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid="])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut children = Vec::new();
    for line in stdout.lines() {
        let mut parts = line.split_whitespace();
        let Some(pid) = parts.next().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        let Some(ppid) = parts.next().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        if ppid == parent_pid {
            children.push(pid);
        }
    }
    children.sort_unstable();
    children.dedup();
    children
}

#[cfg(target_os = "linux")]
fn process_env_contains_runtime_kind(pid: u32, runtime_kind: &str) -> bool {
    let env_path = format!("/proc/{pid}/environ");
    let Ok(env) = std::fs::read(env_path) else {
        return false;
    };
    env.split(|byte| *byte == 0)
        .any(|entry| entry == format!("ELASTOS_RUNTIME_KIND={runtime_kind}").as_bytes())
}

#[cfg(all(unix, not(target_os = "linux")))]
fn process_env_contains_runtime_kind(pid: u32, runtime_kind: &str) -> bool {
    let _ = runtime_kind;
    let Ok(output) = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let command = String::from_utf8_lossy(&output.stdout);
    command.contains("elastos") && command.contains(" serve ") && command.contains(" --addr ")
}

fn terminate_sibling_managed_runtime_children(runtime_kind: &str, keep_pid: u32) {
    #[cfg(unix)]
    {
        for child_pid in current_process_child_pids() {
            if child_pid == keep_pid {
                continue;
            }
            if process_env_contains_runtime_kind(child_pid, runtime_kind) {
                unsafe {
                    libc::kill(child_pid as i32, libc::SIGTERM);
                }
            }
        }
    }

    #[cfg(not(unix))]
    {
        let _ = runtime_kind;
        let _ = keep_pid;
    }
}

fn is_managed_user_runtime_kind(kind: &str) -> bool {
    matches!(
        kind,
        RUNTIME_KIND_MANAGED_IDENTITY | RUNTIME_KIND_MANAGED_CHAT | RUNTIME_KIND_MANAGED_HOME
    )
}

fn managed_runtime_kind_label(kind: &str) -> &str {
    match kind {
        RUNTIME_KIND_MANAGED_IDENTITY => "identity",
        RUNTIME_KIND_MANAGED_CHAT => "chat",
        RUNTIME_KIND_MANAGED_HOME => "home",
        _ => kind,
    }
}

impl RuntimeCoords {
    pub fn is_operator_runtime(&self) -> bool {
        !is_managed_user_runtime_kind(&self.runtime_kind)
    }
}

fn managed_runtime_lane_conflict_message(
    surface_name: &str,
    owner: &crate::host_lock::HostProcessInfo,
) -> String {
    let next_step = match owner.role.as_str() {
        "serve" => format!(
            "`elastos serve` already owns this home. Stop it first if you want the managed {} lane, or keep using the operator lane with commands like `elastos node ...`, `elastos agent`, `elastos run`, or `elastos capsule`.",
            surface_name
        ),
        "gateway" | "gateway-public" => format!(
            "A gateway host already owns this home. Stop that gateway first if you want the managed {} lane, or keep using the existing gateway surface.",
            surface_name
        ),
        other => format!(
            "Host role '{}' already owns this home. Stop it first if you want the managed {} lane, or keep using its existing surface.",
            other, surface_name
        ),
    };

    format!(
        "Cannot auto-start local {} runtime while another ElastOS host already owns this identity (pid {}, role '{}', addr {}).\n\n{}",
        surface_name, owner.pid, owner.role, owner.addr, next_step
    )
}

fn gateway_owner_allows_subordinate_managed_runtime(
    owner: &crate::host_lock::HostProcessInfo,
) -> bool {
    if !matches!(owner.role.as_str(), "gateway" | "gateway-public") {
        return false;
    }
    if owner.pid == std::process::id() {
        return true;
    }
    std::env::var_os(GATEWAY_OWNED_HOME_TERMINAL_ENV).as_deref() == Some(std::ffi::OsStr::new("1"))
        && parent_process_id() == Some(owner.pid)
}

fn parent_process_id() -> Option<u32> {
    #[cfg(unix)]
    {
        let ppid = unsafe { libc::getppid() };
        if ppid > 0 {
            Some(ppid as u32)
        } else {
            None
        }
    }

    #[cfg(not(unix))]
    {
        None
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

/// Public wrapper for raw mode — used by run_cmd, chat_cmd, and home_cmd.
/// Returns a guard that restores the terminal on drop.
pub fn enable_host_raw_mode_pub() -> Option<TermiosGuard> {
    enable_host_raw_mode().map(|s| TermiosGuard(Some(s)))
}

pub struct TermiosGuard(Option<SavedTermios>);

impl Drop for TermiosGuard {
    fn drop(&mut self) {
        if let Some(saved) = self.0.take() {
            restore_host_terminal(saved);
        }
    }
}

pub fn runtime_coord_path(data_dir: &Path) -> PathBuf {
    if let Some(path) = std::env::var_os("ELASTOS_RUNTIME_COORDS_FILE") {
        return PathBuf::from(path);
    }
    data_dir.join("runtime-coords.json")
}

/// Portable PID liveness check.
///
/// Previously the codebase used `PathBuf::from(format!("/proc/{}", pid)).exists()`,
/// which works on Linux but silently always returns `false` on macOS (no /proc).
/// That caused `read_runtime_coords` to delete the coords file every time it was
/// invoked on Darwin, which in turn made `elastos home` / `elastos chat` /
/// gateway-launch paths time out waiting for a runtime that was actually alive.
///
/// `kill(pid, 0)` is the portable POSIX way: it returns 0 when the target exists
/// and the caller can signal it, sets `ESRCH` when no such process exists, and
/// sets `EPERM` when the process exists but the caller may not signal it (still
/// alive from our perspective). Anything other than `ESRCH` means alive.
fn pid_is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let pid_i32: libc::pid_t = match pid.try_into() {
            Ok(v) => v,
            Err(_) => return false,
        };
        let ret = unsafe { libc::kill(pid_i32, 0) };
        if ret == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        // Conservative fallback: assume alive so we don't tear down a live
        // runtime's coords. Implement a Win32 OpenProcess-based check when
        // Windows support lands.
        let _ = pid;
        true
    }
}

fn managed_runtime_coord_path(data_dir: &Path, runtime_kind: &str) -> PathBuf {
    if runtime_kind == RUNTIME_KIND_MANAGED_HOME {
        return home_runtime_coord_path(data_dir);
    }
    runtime_coord_path(data_dir)
}

pub(crate) fn home_runtime_coord_path(data_dir: &Path) -> PathBuf {
    data_dir.join("home-runtime-coords.json")
}

fn managed_runtime_child_home_dir(data_dir: &Path, runtime_kind: &str) -> PathBuf {
    data_dir
        .join("managed-runtimes")
        .join(managed_runtime_kind_label(runtime_kind))
        .join("home")
}

fn managed_runtime_child_xdg_data_home(child_home: &Path) -> PathBuf {
    child_home.join(".local").join("share")
}

pub fn write_runtime_coords(path: &Path, coords: &RuntimeCoords) -> anyhow::Result<()> {
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

pub async fn read_runtime_coords(path: &Path) -> Option<RuntimeCoords> {
    let data = std::fs::read(path).ok()?;
    let coords: RuntimeCoords = serde_json::from_slice(&data).ok()?;
    let api_base = match LoopbackHttpBaseUrl::parse(&coords.api_url) {
        Ok(api_base) => api_base,
        Err(_) => {
            let _ = std::fs::remove_file(path);
            return None;
        }
    };

    if !pid_is_alive(coords.pid) {
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

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while pid_is_alive(coords.pid) && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    #[cfg(unix)]
    if pid_is_alive(coords.pid) {
        unsafe {
            libc::kill(coords.pid as i32, libc::SIGKILL);
        }
        let kill_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while pid_is_alive(coords.pid) && std::time::Instant::now() < kill_deadline {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
}

pub async fn read_operator_runtime_coords(path: &Path) -> Option<RuntimeCoords> {
    let coords = read_runtime_coords(path).await?;
    if coords.is_operator_runtime() {
        Some(coords)
    } else {
        None
    }
}

/// Tokens returned by the attach endpoint.
pub struct AttachTokens {
    pub shell_token: String,
    pub client_token: String,
}

/// Exchange the attach secret for short-lived session tokens.
///
/// Calls `/api/auth/attach` twice — once for shell scope, once for client scope.
pub async fn attach_to_runtime(coords: &RuntimeCoords) -> anyhow::Result<AttachTokens> {
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

pub async fn attach_client_token_to_operator_runtime(
    data_dir: &Path,
) -> anyhow::Result<(RuntimeCoords, String)> {
    let coords_path = runtime_coord_path(data_dir);
    let coords = read_operator_runtime_coords(&coords_path)
        .await
        .ok_or_else(|| anyhow::anyhow!(OPERATOR_RUNTIME_REQUIRED_MESSAGE))?;
    if let Some(reason) = operator_runtime_staleness_reason(&coords).await? {
        anyhow::bail!("{}", reason);
    }
    let tokens = attach_to_runtime(&coords).await?;
    Ok((coords, tokens.client_token))
}

pub async fn operator_runtime_staleness_reason(
    coords: &RuntimeCoords,
) -> anyhow::Result<Option<String>> {
    if !coords.is_operator_runtime() {
        return Ok(None);
    }

    let expected_version = env!("ELASTOS_VERSION");
    match runtime_version_from_health(&coords.api_url).await {
        Some(actual) if actual != expected_version => {
            return Ok(Some(format!(
                "local operator runtime is stale (running {}, expected {}). Restart it with `elastos serve`.",
                actual, expected_version
            )));
        }
        Some(_) => {}
        None => {
            return Ok(Some(format!(
                "local operator runtime version is unknown (expected {}). Restart it with `elastos serve`.",
                expected_version
            )));
        }
    }

    let self_exe = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("Failed to determine runtime binary: {}", e))?;
    let expected_binary_sha256 = sha256_file(&self_exe)?;
    if coords.binary_sha256.is_empty() {
        return Ok(Some(
            "local operator runtime fingerprint is missing. Restart it with `elastos serve`."
                .to_string(),
        ));
    }
    if coords.binary_sha256 != expected_binary_sha256 {
        return Ok(Some(
            "local operator runtime binary changed on disk. Restart it with `elastos serve`."
                .to_string(),
        ));
    }

    Ok(None)
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

pub(crate) async fn dispatch_via_existing_runtime(
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
pub async fn ensure_runtime_for_chat(data_dir: &Path) -> anyhow::Result<RuntimeCoords> {
    if let Some(coords) = reuse_home_runtime_for_chat(data_dir).await {
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

async fn reuse_home_runtime_for_chat(data_dir: &Path) -> Option<RuntimeCoords> {
    let coords = read_runtime_coords(&home_runtime_coord_path(data_dir)).await?;
    if coords.runtime_kind != RUNTIME_KIND_MANAGED_HOME {
        return None;
    }

    let expected = env!("ELASTOS_VERSION");
    match runtime_version_from_health(&coords.api_url).await {
        Some(actual) if actual == expected => {
            if runtime_notices_enabled() {
                eprintln!("Reusing local Home runtime for chat.");
            }
            Some(coords)
        }
        _ => None,
    }
}

/// Ensure a runtime is running for Home. Like native chat,
/// this is a managed one-terminal bootstrap that hides runtime plumbing from
/// the user-facing home surface.
pub async fn ensure_runtime_for_home(data_dir: &Path) -> anyhow::Result<RuntimeCoords> {
    ensure_managed_runtime(
        data_dir,
        RUNTIME_KIND_MANAGED_HOME,
        "home-auto.json",
        managed_home_allow_resources(),
        "home",
    )
    .await
}

fn managed_chat_allow_resources() -> &'static [&'static str] {
    &[
        "elastos://peer/*",
        "elastos://did/*",
        // The capsule bridge maps `Users/self` to the active principal root
        // before policy evaluation and rejects foreign user roots.
        "localhost://Users/*",
    ]
}

fn managed_home_allow_resources() -> &'static [&'static str] {
    &[
        "elastos://peer/*",
        "elastos://did/*",
        // Broad namespace allowance is safe only because the bridge scopes
        // `Users/self` and rejects explicit non-active `Users/<root>` requests.
        "localhost://Users/*",
        "localhost://Local/SharedByLocalUsersAndBots/Home/*",
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
    let coords_path = managed_runtime_coord_path(data_dir, runtime_kind);
    // Home boot can issue concurrent app/system/browser requests. Serialize the
    // check/start sequence across processes so only one managed runtime can be
    // spawned for a runtime data directory.
    let start_guard = match acquire_managed_runtime_start_lock(data_dir)? {
        ManagedRuntimeStart::Owner(guard) => guard,
        ManagedRuntimeStart::Waiter => {
            let expected = env!("ELASTOS_VERSION");
            if let Some(coords) =
                wait_for_managed_runtime_ready(&coords_path, expected, Duration::from_secs(45))
                    .await
            {
                terminate_sibling_managed_runtime_children(runtime_kind, coords.pid);
                return Ok(coords);
            }
            anyhow::bail!(
                "managed {} runtime start is already in progress but did not become ready within 45s",
                surface_name
            );
        }
    };

    let self_exe = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("Failed to determine runtime binary: {}", e))?;
    let binary_sha256 = sha256_file(&self_exe)?;
    let (_home_launch_signing_key, home_launch_signer_did) =
        elastos_identity::load_or_create_did(data_dir)?;
    let policy = serde_json::json!({
        "allow": allow_resources,
        "home_launch_signer_did": home_launch_signer_did.clone(),
        "home_launch_auth_data_dir": data_dir.display().to_string(),
    });
    let policy_sha256 = sha256_bytes(serde_json::to_vec(&policy)?.as_slice());
    let dependency_sha256 = managed_runtime_dependency_sha256();

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
                        Some(actual) if actual == expected => {
                            if coords.binary_sha256 == binary_sha256
                                && coords.policy_sha256 == policy_sha256
                                && coords.dependency_sha256 == dependency_sha256
                            {
                                terminate_sibling_managed_runtime_children(
                                    runtime_kind,
                                    coords.pid,
                                );
                                return Ok(coords);
                            }
                            if runtime_notices_enabled() {
                                eprintln!(
                                    "Managed {} runtime fingerprint changed. Restarting...",
                                    surface_name
                                );
                            }
                            terminate_managed_chat_runtime(&coords, &coords_path).await;
                        }
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

    let subordinate_gateway_host = match crate::host_lock::active_host_process(data_dir)? {
        Some(owner) if gateway_owner_allows_subordinate_managed_runtime(&owner) => true,
        Some(owner) => anyhow::bail!(managed_runtime_lane_conflict_message(surface_name, &owner)),
        None => false,
    };
    let child_home_dir =
        subordinate_gateway_host.then(|| managed_runtime_child_home_dir(data_dir, runtime_kind));
    if let Some(child_home_dir) = child_home_dir.as_ref() {
        std::fs::create_dir_all(managed_runtime_child_xdg_data_home(child_home_dir))?;
    }

    if let Some(pid) = current_process_managed_runtime_child(runtime_kind) {
        if runtime_notices_enabled() {
            eprintln!(
                "Managed {} runtime is already starting (pid {}). Waiting for readiness...",
                surface_name, pid
            );
        }
        if let Some(coords) = wait_for_managed_runtime_ready(
            &coords_path,
            env!("ELASTOS_VERSION"),
            Duration::from_secs(45),
        )
        .await
        {
            terminate_sibling_managed_runtime_children(runtime_kind, coords.pid);
            drop(start_guard);
            return Ok(coords);
        }
        anyhow::bail!(
            "managed {} runtime child {} did not become ready within 45s",
            surface_name,
            pid
        );
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
    for name in MANAGED_RUNTIME_REQUIRED_PROVIDERS {
        if crate::binaries::find_installed_provider_binary(name).is_none() {
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

    let managed_addr = reserve_managed_chat_addr()
        .map_err(|e| anyhow::anyhow!("Failed to reserve managed runtime port: {}", e))?;

    // Create a chat-scoped policy for the managed runtime.
    // This auto-approves only the capabilities native chat needs.
    // Other commands (share, agent, etc.) that need broader capabilities
    // should use `elastos serve` with operator-configured policy.
    let policy_dir = data_dir.join("policy");
    std::fs::create_dir_all(&policy_dir)?;
    let policy_path = policy_dir.join(policy_file_name);
    std::fs::write(&policy_path, serde_json::to_string_pretty(&policy)?)?;

    // 4. Create log directory
    let log_dir = data_dir.join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join("runtime.log");
    let log_file = std::fs::File::create(&log_path)
        .map_err(|e| anyhow::anyhow!("Failed to create runtime log: {}", e))?;

    // 5. Spawn managed runtime as detached background process
    let mut child = std::process::Command::new(&self_exe);
    child
        .arg("serve")
        .arg("--addr")
        .arg(&managed_addr)
        .env("ELASTOS_POLICY_FILE", &policy_path)
        .env("ELASTOS_SHELL_MODE", "agent")
        .env("ELASTOS_RUNTIME_KIND", runtime_kind)
        .env("ELASTOS_RUNTIME_BINARY_SHA256", &binary_sha256)
        .env("ELASTOS_RUNTIME_POLICY_SHA256", &policy_sha256)
        .env("ELASTOS_RUNTIME_DEPENDENCY_SHA256", &dependency_sha256)
        .stdout(std::process::Stdio::from(log_file.try_clone()?))
        .stderr(std::process::Stdio::from(log_file))
        .stdin(std::process::Stdio::null());
    if std::env::var_os(CARRIER_MDNS_ENV).is_none() {
        if let Some(value) = managed_runtime_default_carrier_mdns(runtime_kind) {
            child.env(CARRIER_MDNS_ENV, value);
        }
    }
    if subordinate_gateway_host {
        child.env("ELASTOS_ALLOW_SUBORDINATE_RUNTIME_HOST", "1");
        child.env("ELASTOS_RUNTIME_COORDS_FILE", &coords_path);
        child.env("ELASTOS_CAPSULE_BIN_DIR", data_dir.join("bin"));
        child.env(
            crate::api::gateway::HOME_LAUNCH_TRUSTED_SIGNER_DID_ENV,
            &home_launch_signer_did,
        );
        child.env(
            crate::api::gateway::HOME_LAUNCH_TRUSTED_AUTH_DATA_DIR_ENV,
            data_dir,
        );
        if let Some(child_home_dir) = child_home_dir.as_ref() {
            child.env("HOME", child_home_dir).env(
                "XDG_DATA_HOME",
                managed_runtime_child_xdg_data_home(child_home_dir),
            );
        }
    }
    let mut child = child
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to start runtime: {}", e))?;

    if runtime_notices_enabled() {
        eprintln!(
            "Runtime started (pid {}). Log: {}",
            child.id(),
            log_path.display()
        );
    }

    let expected_version = env!("ELASTOS_VERSION");

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
        if let Some(coords) = managed_runtime_ready(&coords_path, expected_version).await {
            terminate_sibling_managed_runtime_children(runtime_kind, coords.pid);
            drop(start_guard);
            return Ok(coords);
        }
    }
}

async fn managed_runtime_ready(
    coords_path: &Path,
    expected_version: &str,
) -> Option<RuntimeCoords> {
    let coords = read_runtime_coords(coords_path).await?;
    match runtime_version_from_health(&coords.api_url).await {
        Some(actual) if actual == expected_version => Some(coords),
        _ => None,
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

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(sha256_bytes(&bytes))
}

fn managed_runtime_dependency_sha256() -> String {
    let mut input = String::new();
    for name in MANAGED_RUNTIME_REQUIRED_PROVIDERS {
        input.push_str(name);
        input.push('\n');
        match crate::binaries::find_installed_provider_binary(name) {
            Some(path) => {
                input.push_str(&path.display().to_string());
                input.push('\n');
                match sha256_file(&path) {
                    Ok(hash) => input.push_str(&hash),
                    Err(err) => input.push_str(&format!("unreadable:{err}")),
                }
            }
            None => input.push_str("missing"),
        }
        input.push('\n');
    }
    sha256_bytes(input.as_bytes())
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
            "Installed core components are out of sync with the stamped manifest.\nRun `elastos setup --profile home` (or `elastos setup`) to reconcile installed support assets, then retry.\nLast runtime error: {}",
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

        assert!(summary.contains("Run `elastos setup --profile home`"));
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

    #[tokio::test]
    async fn managed_runtime_ready_requires_runtime_health() {
        let temp_dir = tempfile::tempdir().unwrap();
        let coords_path = temp_dir.path().join("runtime-coords.json");
        std::fs::write(
            &coords_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "api_url": "http://127.0.0.1:9",
                "attach_secret": "secret",
                "pid": 1,
                "runtime_kind": RUNTIME_KIND_MANAGED_HOME,
                "binary_sha256": "",
                "policy_sha256": "",
            }))
            .unwrap(),
        )
        .unwrap();

        let ready = managed_runtime_ready(&coords_path, env!("ELASTOS_VERSION")).await;
        assert!(ready.is_none());
    }

    #[tokio::test]
    async fn managed_runtime_ready_accepts_matching_health() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new()
            .route(
                "/api/health",
                get(|| async {
                    Json(serde_json::json!({
                        "status": "ok",
                        "version": env!("ELASTOS_VERSION"),
                    }))
                }),
            )
            .route(
                "/api/auth/attach",
                axum::routing::post(|| async {
                    Json(serde_json::json!({
                        "session_type": "client",
                        "token": "test-token",
                    }))
                }),
            );

        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let temp_dir = tempfile::tempdir().unwrap();
        let coords_path = temp_dir.path().join("runtime-coords.json");
        std::fs::write(
            &coords_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "api_url": format!("http://{}", addr),
                "attach_secret": "secret",
                "pid": 1,
                "runtime_kind": RUNTIME_KIND_MANAGED_HOME,
                "binary_sha256": "",
                "policy_sha256": "",
            }))
            .unwrap(),
        )
        .unwrap();

        let mut ready = None;
        for _ in 0..20 {
            ready = managed_runtime_ready(&coords_path, env!("ELASTOS_VERSION")).await;
            if ready.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        server.abort();
        assert!(ready.is_some());
    }

    #[test]
    fn managed_home_policy_uses_principal_root_namespace() {
        let allow = managed_home_allow_resources();
        assert!(allow.contains(&"localhost://Users/*"));
        assert!(!allow
            .iter()
            .any(|resource| resource.contains("localhost://Users/self")));
        assert!(allow.contains(&"localhost://Local/SharedByLocalUsersAndBots/Home/*"));
    }

    #[test]
    fn managed_runtimes_default_carrier_mdns_off() {
        assert_eq!(
            managed_runtime_default_carrier_mdns(RUNTIME_KIND_MANAGED_HOME),
            Some("0")
        );
        assert_eq!(
            managed_runtime_default_carrier_mdns(RUNTIME_KIND_MANAGED_CHAT),
            Some("0")
        );
        assert_eq!(
            managed_runtime_default_carrier_mdns(RUNTIME_KIND_MANAGED_IDENTITY),
            Some("0")
        );
        assert_eq!(
            managed_runtime_default_carrier_mdns(RUNTIME_KIND_OPERATOR),
            None
        );
    }

    #[test]
    fn home_runtime_coord_path_uses_data_dir_and_not_env_override() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var(
            "ELASTOS_RUNTIME_COORDS_FILE",
            tmp.path().join("override.json"),
        );
        let actual = home_runtime_coord_path(tmp.path());
        std::env::remove_var("ELASTOS_RUNTIME_COORDS_FILE");
        assert_eq!(actual, tmp.path().join("home-runtime-coords.json"));
    }

    #[test]
    fn managed_home_runtime_uses_home_coord_path() {
        let tmp = tempfile::tempdir().unwrap();
        let override_path = tmp.path().join("override.json");
        std::env::set_var("ELASTOS_RUNTIME_COORDS_FILE", &override_path);

        assert_eq!(
            managed_runtime_coord_path(tmp.path(), RUNTIME_KIND_MANAGED_HOME),
            tmp.path().join("home-runtime-coords.json")
        );
        assert_eq!(
            managed_runtime_coord_path(tmp.path(), RUNTIME_KIND_MANAGED_CHAT),
            override_path
        );

        std::env::remove_var("ELASTOS_RUNTIME_COORDS_FILE");
    }

    #[test]
    fn subordinate_managed_runtime_home_is_nested_under_parent_data_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let home = managed_runtime_child_home_dir(tmp.path(), RUNTIME_KIND_MANAGED_HOME);

        assert!(home.starts_with(tmp.path()));
        assert!(home.ends_with("managed-runtimes/home/home"));
        assert_eq!(
            managed_runtime_child_xdg_data_home(&home),
            home.join(".local/share")
        );
    }

    #[test]
    fn managed_runtime_lane_conflict_mentions_operator_lane_when_serve_owns_home() {
        let owner = crate::host_lock::HostProcessInfo {
            pid: 123,
            role: "serve".to_string(),
            addr: "0.0.0.0:3000".to_string(),
        };

        let message = managed_runtime_lane_conflict_message("home", &owner);

        assert!(message.contains("`elastos serve` already owns this home."));
        assert!(message.contains("managed home lane"));
        assert!(message.contains("`elastos node ...`"));
    }

    #[test]
    fn managed_runtime_lane_conflict_mentions_gateway_when_gateway_owns_home() {
        let owner = crate::host_lock::HostProcessInfo {
            pid: 456,
            role: "gateway".to_string(),
            addr: "127.0.0.1:8090".to_string(),
        };

        let message = managed_runtime_lane_conflict_message("chat", &owner);

        assert!(message.contains("gateway host already owns this home"));
        assert!(message.contains("managed chat lane"));
    }

    #[test]
    fn current_gateway_host_allows_subordinate_managed_runtime() {
        let owner = crate::host_lock::HostProcessInfo {
            pid: std::process::id(),
            role: "gateway".to_string(),
            addr: "127.0.0.1:8090".to_string(),
        };

        assert!(gateway_owner_allows_subordinate_managed_runtime(&owner));
    }

    #[test]
    #[cfg(unix)]
    fn gateway_owned_terminal_child_allows_parent_gateway_subordinate_runtime() {
        let _guard = EnvRestore::set(GATEWAY_OWNED_HOME_TERMINAL_ENV, "1");
        let owner = crate::host_lock::HostProcessInfo {
            pid: parent_process_id().expect("test process should have a parent"),
            role: "gateway".to_string(),
            addr: "127.0.0.1:8090".to_string(),
        };

        assert!(gateway_owner_allows_subordinate_managed_runtime(&owner));
    }

    #[test]
    #[cfg(unix)]
    fn unmarked_terminal_child_does_not_bypass_foreign_gateway_owner() {
        let _guard = EnvRestore::remove(GATEWAY_OWNED_HOME_TERMINAL_ENV);
        let owner = crate::host_lock::HostProcessInfo {
            pid: parent_process_id().expect("test process should have a parent"),
            role: "gateway".to_string(),
            addr: "127.0.0.1:8090".to_string(),
        };

        assert!(!gateway_owner_allows_subordinate_managed_runtime(&owner));
    }

    #[test]
    fn foreign_gateway_host_still_conflicts_with_managed_runtime() {
        let owner = crate::host_lock::HostProcessInfo {
            pid: std::process::id().saturating_add(1),
            role: "gateway-public".to_string(),
            addr: "127.0.0.1:8090".to_string(),
        };

        assert!(!gateway_owner_allows_subordinate_managed_runtime(&owner));
    }

    struct EnvRestore {
        key: &'static str,
        value: Option<std::ffi::OsString>,
    }

    impl EnvRestore {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self {
                key,
                value: previous,
            }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            std::env::remove_var(key);
            Self {
                key,
                value: previous,
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.value {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}
