//! ElastOS Browser Engine host supervisor.
//!
//! This helper is launched by `browser-engine-adapter` for native engines such
//! as CEF/Chromium. It reads a typed Runtime launch request from
//! `ELASTOS_BROWSER_ENGINE_REQUEST`, reads operator config from
//! `ELASTOS_BROWSER_ENGINE_SUPERVISOR_CONFIG`, starts the configured engine with
//! a Linux network namespace, and prints a typed supervisor result.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::env;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

const REQUEST_ENV: &str = "ELASTOS_BROWSER_ENGINE_REQUEST";
const CONFIG_ENV: &str = "ELASTOS_BROWSER_ENGINE_SUPERVISOR_CONFIG";
const DEFAULT_VIEWPORT_WIDTH: u32 = 1280;
const DEFAULT_VIEWPORT_HEIGHT: u32 = 720;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchRequest {
    schema: String,
    adapter: String,
    engine: EngineKind,
    url: String,
    stream_id: String,
    target: String,
    #[serde(default)]
    principal_id: Option<String>,
    network_mode: NetworkMode,
    direct_network: bool,
    wallet_injection: bool,
    adapter_ipc: AdapterIpcEndpoint,
    #[serde(default)]
    relay_ipc: Option<RelayIpcEndpoint>,
    display_mode: BrowserDisplayMode,
    #[serde(default)]
    wallet: serde_json::Value,
    #[serde(default)]
    viewport: Option<ViewportRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SupervisorConfig {
    schema: String,
    adapter: String,
    engine: EngineKind,
    program: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    network_sandbox: NetworkSandbox,
    #[serde(default = "default_startup_grace_ms")]
    startup_grace_ms: u64,
    #[serde(default)]
    stream_bridge: Option<StreamBridgeConfig>,
    #[serde(default)]
    display_capabilities: DisplayCapabilities,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StreamBridgeConfig {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    replace_existing_socket: bool,
    #[serde(default = "default_stream_bridge_startup_wait_ms")]
    startup_wait_ms: u64,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct DisplayCapabilities {
    #[serde(default)]
    audio: bool,
    #[serde(default)]
    video: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdapterIpcEndpoint {
    schema: String,
    kind: AdapterIpcKind,
    path: String,
    stream_id: String,
    #[serde(default)]
    runtime_stream_path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayIpcEndpoint {
    schema: String,
    kind: AdapterIpcKind,
    path: String,
    #[serde(default)]
    stream_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewportRequest {
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum EngineKind {
    Cef,
    ChromiumMicrovm,
    Webview2,
    Geckoview,
    Wkwebview,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum BrowserDisplayMode {
    NativeSurface,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum NetworkMode {
    RuntimeNetOnly,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AdapterIpcKind {
    UnixSocket,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum NetworkSandbox {
    LinuxNewNetns,
}

fn main() {
    match run() {
        Ok(result) => {
            println!("{result}");
        }
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<String, String> {
    let request = parse_env_json::<LaunchRequest>(REQUEST_ENV)?;
    let config = parse_env_json::<SupervisorConfig>(CONFIG_ENV)?;
    validate_request(&request)?;
    validate_config(&config, &request)?;
    let stream_bridge_pid = match spawn_stream_bridge(config.stream_bridge.as_ref(), &request) {
        Ok(pid) => pid,
        Err(err) => return Err(format!("browser stream bridge unavailable: {err}")),
    };
    let child_id = spawn_engine(&config, &request)?;
    Ok(supervisor_result(&config, &request, child_id, stream_bridge_pid).to_string())
}

fn supervisor_result(
    config: &SupervisorConfig,
    request: &LaunchRequest,
    child_id: u32,
    stream_bridge_pid: Option<u32>,
) -> serde_json::Value {
    let surface_id = stable_surface_id(&request.url, &request.stream_id);
    let viewport = request.viewport.unwrap_or(ViewportRequest {
        width: DEFAULT_VIEWPORT_WIDTH,
        height: DEFAULT_VIEWPORT_HEIGHT,
    });
    json!({
        "schema": "elastos.browser.engine.supervisor-result/v1",
        "page_id": stable_page_id(&request.url, &request.stream_id),
        "adapter": request.adapter,
        "engine": request.engine,
        "stream_id": request.stream_id,
        "network_mode": "runtime_net_only",
        "direct_network": false,
        "wallet_injection": false,
        "process": {
            "pid": child_id,
            "stream_bridge_pid": stream_bridge_pid,
            "network_sandbox": "linux_new_netns"
        },
        "display_session": {
            "schema": "elastos.browser.display-session/v1",
            "session_id": format!("display:{}", request.stream_id),
            "mode": "native_surface",
            "surface_id": surface_id,
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "input": "native_ipc",
            "width": viewport.width,
            "height": viewport.height,
            "audio": config.display_capabilities.audio,
            "video": config.display_capabilities.video
        },
        "view": {
            "schema": "elastos.browser.view/v1",
            "mode": "native_surface",
            "width": viewport.width,
            "height": viewport.height
        }
    })
}

fn parse_env_json<T: for<'de> Deserialize<'de>>(name: &str) -> Result<T, String> {
    let raw = env::var(name).map_err(|_| format!("{name} is required"))?;
    serde_json::from_str(&raw).map_err(|err| format!("{name} is invalid JSON: {err}"))
}

fn validate_request(request: &LaunchRequest) -> Result<(), String> {
    if request.schema != "elastos.browser.engine.launch-request/v1" {
        return Err("unsupported launch request schema".to_string());
    }
    if !is_safe_id(&request.adapter) {
        return Err("adapter must be a safe identifier".to_string());
    }
    if !is_safe_id(&request.stream_id) {
        return Err("stream_id must be a safe identifier".to_string());
    }
    if let Some(principal_id) = &request.principal_id {
        if !is_safe_id(principal_id) {
            return Err("principal_id must be a safe identifier".to_string());
        }
    }
    if !request.url.starts_with("https://") && !request.url.starts_with("http://") {
        return Err("url must use http or https".to_string());
    }
    if !request.target.starts_with("tls://") && !request.target.starts_with("tcp://") {
        return Err("target must use tls or tcp".to_string());
    }
    if request.network_mode != NetworkMode::RuntimeNetOnly {
        return Err("request must be runtime_net_only".to_string());
    }
    if request.direct_network {
        return Err("request must not grant direct network".to_string());
    }
    if request.wallet_injection {
        return Err("request must not grant wallet injection".to_string());
    }
    if request.display_mode != BrowserDisplayMode::NativeSurface {
        return Err("browser-engine-supervisor supports only native_surface".to_string());
    }
    if let Some(viewport) = request.viewport {
        validate_viewport(viewport)?;
    }
    let _ = &request.wallet;
    validate_adapter_ipc(&request.adapter_ipc, &request.stream_id)?;
    if let Some(relay_ipc) = &request.relay_ipc {
        validate_relay_ipc(relay_ipc)?;
    }
    Ok(())
}

fn validate_viewport(viewport: ViewportRequest) -> Result<(), String> {
    if viewport.width < 320
        || viewport.width > 3840
        || viewport.height < 240
        || viewport.height > 2160
    {
        return Err("viewport must be within 320x240 and 3840x2160".to_string());
    }
    Ok(())
}

fn validate_config(config: &SupervisorConfig, request: &LaunchRequest) -> Result<(), String> {
    if config.schema != "elastos.browser.engine.supervisor-config/v1" {
        return Err("unsupported supervisor config schema".to_string());
    }
    if config.adapter != request.adapter {
        return Err("supervisor config adapter does not match request".to_string());
    }
    if config.engine != request.engine {
        return Err("supervisor config engine does not match request".to_string());
    }
    if config.network_sandbox != NetworkSandbox::LinuxNewNetns {
        return Err("supervisor config must require linux_new_netns".to_string());
    }
    if config.program.is_empty() || !config.program.starts_with('/') {
        return Err("supervisor program must be absolute".to_string());
    }
    if config.program.bytes().any(|byte| byte == b'\0') {
        return Err("supervisor program must not contain NUL".to_string());
    }
    if config
        .args
        .iter()
        .any(|arg| arg.bytes().any(|byte| byte == b'\0'))
    {
        return Err("supervisor args must not contain NUL".to_string());
    }
    for (key, value) in &config.env {
        if key.is_empty()
            || key.bytes().any(|byte| byte == b'=' || byte == b'\0')
            || value.bytes().any(|byte| byte == b'\0')
        {
            return Err("supervisor env must use non-empty keys and no NUL".to_string());
        }
    }
    if config.startup_grace_ms > 10_000 {
        return Err("startup_grace_ms must be <= 10000".to_string());
    }
    if let Some(stream_bridge) = &config.stream_bridge {
        validate_stream_bridge(stream_bridge)?;
    }
    Ok(())
}

fn validate_stream_bridge(stream_bridge: &StreamBridgeConfig) -> Result<(), String> {
    if stream_bridge.program.is_empty() || !stream_bridge.program.starts_with('/') {
        return Err("stream bridge program must be absolute".to_string());
    }
    if stream_bridge.program.bytes().any(|byte| byte == b'\0') {
        return Err("stream bridge program must not contain NUL".to_string());
    }
    if stream_bridge
        .args
        .iter()
        .any(|arg| arg.bytes().any(|byte| byte == b'\0'))
    {
        return Err("stream bridge args must not contain NUL".to_string());
    }
    if stream_bridge.startup_wait_ms > 10_000 {
        return Err("stream bridge startup_wait_ms must be <= 10000".to_string());
    }
    Ok(())
}

fn validate_adapter_ipc(endpoint: &AdapterIpcEndpoint, stream_id: &str) -> Result<(), String> {
    if endpoint.schema != "elastos.adapter-ipc/v1" {
        return Err("unsupported adapter_ipc schema".to_string());
    }
    if endpoint.kind != AdapterIpcKind::UnixSocket {
        return Err("unsupported adapter_ipc kind".to_string());
    }
    if endpoint.stream_id != stream_id {
        return Err("adapter_ipc stream_id mismatch".to_string());
    }
    if endpoint.path.is_empty() || !endpoint.path.starts_with('/') {
        return Err("adapter_ipc path must be absolute".to_string());
    }
    if endpoint
        .path
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte == b'\0')
    {
        return Err("adapter_ipc path must not contain whitespace or NUL".to_string());
    }
    if let Some(runtime_stream_path) = &endpoint.runtime_stream_path {
        if runtime_stream_path.is_empty() || !runtime_stream_path.starts_with('/') {
            return Err("adapter_ipc runtime_stream_path must be absolute".to_string());
        }
        if runtime_stream_path
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte == b'\0')
        {
            return Err(
                "adapter_ipc runtime_stream_path must not contain whitespace or NUL".to_string(),
            );
        }
        if runtime_stream_path == &endpoint.path {
            return Err("adapter_ipc runtime_stream_path must differ from path".to_string());
        }
    }
    Ok(())
}

fn validate_relay_ipc(endpoint: &RelayIpcEndpoint) -> Result<(), String> {
    if endpoint.schema != "elastos.exit.relay-ipc/v1" {
        return Err("unsupported relay_ipc schema".to_string());
    }
    if endpoint.kind != AdapterIpcKind::UnixSocket {
        return Err("unsupported relay_ipc kind".to_string());
    }
    if endpoint.path.is_empty() || !endpoint.path.starts_with('/') {
        return Err("relay_ipc path must be absolute".to_string());
    }
    if endpoint
        .path
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte == b'\0')
    {
        return Err("relay_ipc path must not contain whitespace or NUL".to_string());
    }
    if let Some(stream_id) = &endpoint.stream_id {
        if !is_safe_id(stream_id) {
            return Err("relay_ipc stream_id must be a safe identifier".to_string());
        }
    }
    Ok(())
}

fn spawn_stream_bridge(
    stream_bridge: Option<&StreamBridgeConfig>,
    request: &LaunchRequest,
) -> Result<Option<u32>, String> {
    let Some(stream_bridge) = stream_bridge else {
        return Ok(None);
    };
    let bridge_config = stream_bridge_env_config(stream_bridge, request)?;
    let mut child = Command::new(&stream_bridge.program)
        .args(&stream_bridge.args)
        .env("ELASTOS_BROWSER_STREAM_BRIDGE_CONFIG", bridge_config)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| err.to_string())?;
    wait_for_stream_bridge(
        &mut child,
        &request.adapter_ipc.path,
        stream_bridge.startup_wait_ms,
    )?;
    Ok(Some(child.id()))
}

fn stream_bridge_env_config(
    stream_bridge: &StreamBridgeConfig,
    request: &LaunchRequest,
) -> Result<String, String> {
    let Some(runtime_stream_path) = &request.adapter_ipc.runtime_stream_path else {
        return Err("stream bridge requires adapter_ipc runtime_stream_path".to_string());
    };
    Ok(json!({
        "schema": "elastos.browser.stream-bridge.config/v1",
        "stream_id": request.stream_id,
        "target": request.target,
        "adapter_ipc_path": request.adapter_ipc.path,
        "runtime_stream_path": runtime_stream_path,
        "network_mode": "runtime_net_only",
        "direct_network": false,
        "replace_existing_socket": stream_bridge.replace_existing_socket,
    })
    .to_string())
}

fn wait_for_stream_bridge(
    child: &mut std::process::Child,
    adapter_ipc_path: &str,
    startup_wait_ms: u64,
) -> Result<(), String> {
    let path = Path::new(adapter_ipc_path);
    let mut waited_ms = 0_u64;
    loop {
        if path.exists() {
            return Ok(());
        }
        if let Some(status) = child.try_wait().map_err(|err| err.to_string())? {
            return Err(format!("stream bridge exited during startup: {status}"));
        }
        if waited_ms >= startup_wait_ms {
            return Err("stream bridge did not create adapter IPC socket".to_string());
        }
        std::thread::sleep(Duration::from_millis(10));
        waited_ms += 10;
    }
}

fn spawn_engine(config: &SupervisorConfig, request: &LaunchRequest) -> Result<u32, String> {
    let args: Vec<String> = config
        .args
        .iter()
        .map(|arg| expand_arg(arg, request))
        .collect();
    let mut command = Command::new(&config.program);
    command
        .args(args)
        .envs(&config.env)
        .env("ELASTOS_BROWSER_ENGINE_IPC", &request.adapter_ipc.path)
        .env(
            "ELASTOS_BROWSER_ENGINE_RELAY_IPC",
            request
                .relay_ipc
                .as_ref()
                .map(|relay| relay.path.as_str())
                .unwrap_or(""),
        )
        .env("ELASTOS_BROWSER_ENGINE_STREAM_ID", &request.stream_id)
        .env("ELASTOS_BROWSER_ENGINE_TARGET", &request.target)
        .env("ELASTOS_BROWSER_ENGINE_URL", &request.url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    unsafe {
        command.pre_exec(|| {
            if libc::unshare(libc::CLONE_NEWNET) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            bring_loopback_up()?;
            Ok(())
        });
    }

    let mut child = command.spawn().map_err(|err| err.to_string())?;
    if config.startup_grace_ms > 0 {
        std::thread::sleep(Duration::from_millis(config.startup_grace_ms));
        match child.try_wait() {
            Ok(Some(status)) if !status.success() => {
                return Err(format!("browser engine exited during startup: {status}"));
            }
            Ok(_) => {}
            Err(err) => return Err(err.to_string()),
        }
    }
    Ok(child.id())
}

#[repr(C)]
struct LinuxIfReq {
    name: [libc::c_char; libc::IFNAMSIZ],
    data: [u8; 24],
}

impl LinuxIfReq {
    fn loopback_up() -> Self {
        let mut req = Self {
            name: [0; libc::IFNAMSIZ],
            data: [0; 24],
        };
        for (dest, src) in req.name.iter_mut().zip(b"lo\0") {
            *dest = *src as libc::c_char;
        }
        req.set_flags(libc::IFF_UP as libc::c_short);
        req
    }

    fn set_flags(&mut self, flags: libc::c_short) {
        let bytes = flags.to_ne_bytes();
        self.data[0] = bytes[0];
        self.data[1] = bytes[1];
    }

    #[cfg(test)]
    fn flags(&self) -> libc::c_short {
        libc::c_short::from_ne_bytes([self.data[0], self.data[1]])
    }
}

fn bring_loopback_up() -> std::io::Result<()> {
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut req = LinuxIfReq::loopback_up();
    let result = unsafe { libc::ioctl(fd, libc::SIOCSIFFLAGS, &mut req) };
    let close_result = unsafe { libc::close(fd) };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if close_result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn expand_arg(arg: &str, request: &LaunchRequest) -> String {
    arg.replace("{url}", &request.url)
        .replace("{ipc_path}", &request.adapter_ipc.path)
        .replace("{stream_id}", &request.stream_id)
        .replace("{target}", &request.target)
}

fn is_safe_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
}

fn stable_page_id(url: &str, stream_id: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in url.bytes().chain(stream_id.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("page:{hash:016x}")
}

fn stable_surface_id(url: &str, stream_id: &str) -> String {
    stable_page_id(url, stream_id).replacen("page:", "surface:", 1)
}

fn default_startup_grace_ms() -> u64 {
    500
}

fn default_stream_bridge_startup_wait_ms() -> u64 {
    500
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> LaunchRequest {
        LaunchRequest {
            schema: "elastos.browser.engine.launch-request/v1".to_string(),
            adapter: "linux-cef".to_string(),
            engine: EngineKind::Cef,
            url: "https://glidefinance.io/".to_string(),
            stream_id: "stream:proof:test".to_string(),
            target: "tls://glidefinance.io:443".to_string(),
            principal_id: Some("person:local:test".to_string()),
            network_mode: NetworkMode::RuntimeNetOnly,
            direct_network: false,
            wallet_injection: false,
            display_mode: BrowserDisplayMode::NativeSurface,
            wallet: json!({}),
            viewport: Some(ViewportRequest {
                width: 1280,
                height: 720,
            }),
            adapter_ipc: AdapterIpcEndpoint {
                schema: "elastos.adapter-ipc/v1".to_string(),
                kind: AdapterIpcKind::UnixSocket,
                path: "/tmp/elastos-browser-stream.sock".to_string(),
                stream_id: "stream:proof:test".to_string(),
                runtime_stream_path: Some("/tmp/elastos-runtime-stream.sock".to_string()),
            },
            relay_ipc: Some(RelayIpcEndpoint {
                schema: "elastos.exit.relay-ipc/v1".to_string(),
                kind: AdapterIpcKind::UnixSocket,
                path: "/tmp/elastos-browser-exit.sock".to_string(),
                stream_id: Some("stream:proof:test".to_string()),
            }),
        }
    }

    fn config() -> SupervisorConfig {
        SupervisorConfig {
            schema: "elastos.browser.engine.supervisor-config/v1".to_string(),
            adapter: "linux-cef".to_string(),
            engine: EngineKind::Cef,
            program: "/usr/bin/chromium".to_string(),
            args: vec![
                "--user-data-dir=/tmp/elastos-browser".to_string(),
                "--app={url}".to_string(),
            ],
            env: BTreeMap::new(),
            network_sandbox: NetworkSandbox::LinuxNewNetns,
            startup_grace_ms: 0,
            stream_bridge: None,
            display_capabilities: DisplayCapabilities::default(),
        }
    }

    fn stream_bridge_config() -> StreamBridgeConfig {
        StreamBridgeConfig {
            program: "/usr/bin/browser-stream-bridge".to_string(),
            args: Vec::new(),
            replace_existing_socket: true,
            startup_wait_ms: 0,
        }
    }

    #[test]
    fn validates_matching_request_and_config() {
        let request = request();
        let config = config();
        assert!(validate_request(&request).is_ok());
        assert!(validate_config(&config, &request).is_ok());
    }

    #[test]
    fn rejects_direct_network_or_wallet_authority() {
        let mut direct_request = request();
        direct_request.direct_network = true;
        assert!(validate_request(&direct_request)
            .unwrap_err()
            .contains("direct network"));

        let mut wallet_request = request();
        wallet_request.wallet_injection = true;
        assert!(validate_request(&wallet_request)
            .unwrap_err()
            .contains("wallet injection"));

        let mut bad_viewport_request = request();
        bad_viewport_request.viewport = Some(ViewportRequest {
            width: 100,
            height: 720,
        });
        assert!(validate_request(&bad_viewport_request)
            .unwrap_err()
            .contains("viewport"));

        let mut bad_relay_request = request();
        bad_relay_request.relay_ipc.as_mut().unwrap().path = "relative.sock".to_string();
        assert!(validate_request(&bad_relay_request)
            .unwrap_err()
            .contains("relay_ipc"));
    }

    #[test]
    fn rejects_config_mismatch_or_non_absolute_program() {
        let request = request();
        let mut mismatch_config = config();
        mismatch_config.adapter = "other".to_string();
        assert!(validate_config(&mismatch_config, &request)
            .unwrap_err()
            .contains("adapter"));

        let mut relative_config = config();
        relative_config.program = "chromium".to_string();
        assert!(validate_config(&relative_config, &request)
            .unwrap_err()
            .contains("absolute"));

        let mut bad_env_config = config();
        bad_env_config
            .env
            .insert("BAD=KEY".to_string(), "value".to_string());
        assert!(validate_config(&bad_env_config, &request)
            .unwrap_err()
            .contains("env"));
    }

    #[test]
    fn expands_engine_args_without_shell() {
        let request = request();
        assert_eq!(
            expand_arg("--app={url}", &request),
            "--app=https://glidefinance.io/"
        );
        assert_eq!(
            expand_arg("--ipc={ipc_path}", &request),
            "--ipc=/tmp/elastos-browser-stream.sock"
        );
    }

    #[test]
    fn builds_stream_bridge_config_without_network_authority() {
        let request = request();
        let config = stream_bridge_config();
        let bridge_config = stream_bridge_env_config(&config, &request).unwrap();
        let bridge_config: serde_json::Value = serde_json::from_str(&bridge_config).unwrap();
        assert_eq!(
            bridge_config["schema"],
            "elastos.browser.stream-bridge.config/v1"
        );
        assert_eq!(
            bridge_config["adapter_ipc_path"],
            "/tmp/elastos-browser-stream.sock"
        );
        assert_eq!(
            bridge_config["runtime_stream_path"],
            "/tmp/elastos-runtime-stream.sock"
        );
        assert_eq!(bridge_config["network_mode"], "runtime_net_only");
        assert_eq!(bridge_config["direct_network"], false);
    }

    #[test]
    fn stream_bridge_requires_runtime_stream_path() {
        let mut request = request();
        request.adapter_ipc.runtime_stream_path = None;
        let config = stream_bridge_config();
        assert!(stream_bridge_env_config(&config, &request)
            .unwrap_err()
            .contains("runtime_stream_path"));
    }

    #[test]
    fn loopback_ifreq_sets_only_loopback_name_and_up_flag() {
        let req = LinuxIfReq::loopback_up();
        assert_eq!(req.name[0] as u8, b'l');
        assert_eq!(req.name[1] as u8, b'o');
        assert_eq!(req.name[2], 0);
        assert_eq!(
            req.flags() & libc::IFF_UP as libc::c_short,
            libc::IFF_UP as libc::c_short
        );
    }

    #[test]
    fn output_page_id_is_stable() {
        assert_eq!(
            stable_page_id("https://glidefinance.io/", "stream:proof:test"),
            stable_page_id("https://glidefinance.io/", "stream:proof:test")
        );
    }

    #[test]
    fn supervisor_result_includes_native_surface_display_session() {
        let request = request();
        let mut config = config();
        config.display_capabilities = DisplayCapabilities {
            audio: true,
            video: true,
        };
        let result = supervisor_result(&config, &request, 42, Some(7));
        assert_eq!(
            result["schema"],
            "elastos.browser.engine.supervisor-result/v1"
        );
        assert_eq!(result["network_mode"], "runtime_net_only");
        assert_eq!(result["direct_network"], false);
        assert_eq!(
            result["display_session"]["schema"],
            "elastos.browser.display-session/v1"
        );
        assert_eq!(result["display_session"]["mode"], "native_surface");
        assert_eq!(result["display_session"]["input"], "native_ipc");
        assert_eq!(result["display_session"]["direct_network"], false);
        assert_eq!(result["display_session"]["width"], 1280);
        assert_eq!(result["display_session"]["height"], 720);
        assert_eq!(result["display_session"]["audio"], true);
        assert_eq!(result["display_session"]["video"], true);
        assert_eq!(result["view"]["schema"], "elastos.browser.view/v1");
        assert_eq!(result["view"]["mode"], "native_surface");
        assert_eq!(result["view"]["width"], 1280);
        assert_eq!(result["view"]["height"], 720);
        assert_eq!(result["process"]["pid"], 42);
        assert_eq!(result["process"]["stream_bridge_pid"], 7);
    }

    #[test]
    fn supervisor_result_does_not_claim_media_without_operator_capability() {
        let request = request();
        let config = config();
        let result = supervisor_result(&config, &request, 42, None);
        assert_eq!(result["display_session"]["audio"], false);
        assert_eq!(result["display_session"]["video"], false);
    }
}
