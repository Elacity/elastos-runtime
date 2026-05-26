use super::*;
use std::fs;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(super) fn run_supervisor_launch(
    supervisor: &EngineSupervisorConfig,
    adapter: &AdapterConfig,
    context: &LaunchContext<'_>,
) -> Result<SupervisorLaunchResult, String> {
    let request = json!({
        "schema": "elastos.browser.engine.launch-request/v1",
        "adapter": &adapter.id,
        "engine": adapter.kind,
        "url": context.url,
        "stream_id": &context.stream_session.stream_id,
        "target": &context.stream_session.target,
        "principal_id": &context.principal_id,
        "network_mode": adapter.network_mode,
        "direct_network": false,
        "wallet_injection": false,
        "adapter_ipc": &context.stream_session.adapter_ipc,
        "relay_ipc": &context.stream_session.relay_ipc,
        "wallet": &context.wallet,
        "viewport": context.viewport,
        "display_mode": context.display_mode,
    });
    let mut child = Command::new(&supervisor.program)
        .args(&supervisor.args)
        .envs(&supervisor.env)
        .env("ELASTOS_BROWSER_ENGINE_REQUEST", request.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| err.to_string())?;
    let deadline = Instant::now() + Duration::from_millis(supervisor.timeout_ms);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("browser engine supervisor timed out".to_string());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(err) => return Err(err.to_string()),
        }
    };
    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    if !status.success() {
        return Err(format!(
            "browser engine supervisor exited with status {}; {}",
            status,
            stderr.trim()
        ));
    }
    let result = serde_json::from_str::<SupervisorLaunchResult>(stdout.trim())
        .map_err(|err| format!("invalid browser engine supervisor output: {err}"))?;
    Ok(result)
}

pub(super) fn supervisor_control_json(
    socket_path: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    validate_control_socket_path(socket_path)?;
    let body_bytes = body
        .map(|body| serde_json::to_vec(&body).map_err(|err| err.to_string()))
        .transpose()?
        .unwrap_or_default();
    let mut stream = std::os::unix::net::UnixStream::connect(socket_path)
        .map_err(|err| format!("browser engine control socket unavailable: {err}"))?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: browser-engine\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body_bytes.len()
    )
    .map_err(|err| err.to_string())?;
    if !body_bytes.is_empty() {
        stream
            .write_all(&body_bytes)
            .map_err(|err| err.to_string())?;
    }
    stream.flush().map_err(|err| err.to_string())?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|err| err.to_string())?;
    parse_http_json_response(&response)
}

pub(super) fn cleanup_isolated_session(session: &PageControlSession) -> Result<Value, String> {
    let session_dir = session
        .isolation_session_dir
        .as_deref()
        .ok_or_else(|| "isolated browser session did not report a session directory".to_string())?;
    validate_isolated_session_dir(session_dir)?;

    let mut actions = Vec::new();
    if let Some(container_name) = read_target_container_name(session_dir)? {
        let docker_status = Command::new("docker")
            .args(["rm", "-f", &container_name])
            .status();
        actions.push(json!({
            "action": "docker_rm_force",
            "target": container_name,
            "ok": docker_status.as_ref().map(|status| status.success()).unwrap_or(false),
        }));
    }

    let term_status = Command::new("pkill")
        .args(["-TERM", "-f", session_dir])
        .status();
    actions.push(json!({
        "action": "pkill_term_session",
        "target": session_dir,
        "ok": term_status.as_ref().map(|status| status.success()).unwrap_or(false),
    }));
    std::thread::sleep(Duration::from_millis(250));
    let kill_status = Command::new("pkill")
        .args(["-KILL", "-f", session_dir])
        .status();
    actions.push(json!({
        "action": "pkill_kill_session",
        "target": session_dir,
        "ok": kill_status.as_ref().map(|status| status.success()).unwrap_or(false),
    }));

    let _ = fs::remove_file(&session.socket_path);

    Ok(json!({
        "schema": "elastos.browser.isolated-session-cleanup/v1",
        "session_dir": session_dir,
        "actions": actions,
    }))
}

fn validate_isolated_session_dir(session_dir: &str) -> Result<(), String> {
    if !session_dir.starts_with("/tmp/elastos-browser-sessions/stream_")
        || session_dir.contains(['\0', '\r', '\n'])
        || session_dir.contains("/../")
        || session_dir.ends_with("/..")
    {
        return Err("invalid isolated browser session directory".to_string());
    }
    Ok(())
}

fn read_target_container_name(session_dir: &str) -> Result<Option<String>, String> {
    let path = format!("{session_dir}/target.stdout.log");
    let Ok(text) = fs::read_to_string(path) else {
        return Ok(None);
    };
    for line in text.lines().rev() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(container_name) = value.get("container_name").and_then(|entry| entry.as_str())
        else {
            continue;
        };
        if !safe_target_container_name(container_name) {
            return Err("isolated browser target container name is unsafe".to_string());
        }
        return Ok(Some(container_name.to_string()));
    }
    Ok(None)
}

fn safe_target_container_name(value: &str) -> bool {
    value
        .strip_prefix("elastos-selkies-runtime-exit-target-")
        .map(|suffix| !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()))
        .unwrap_or(false)
}

pub(super) fn parse_http_json_response(response: &[u8]) -> Result<Value, String> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "browser engine control response missing HTTP headers".to_string())?;
    let headers = std::str::from_utf8(&response[..header_end])
        .map_err(|err| format!("browser engine control response invalid UTF-8: {err}"))?;
    let status_line = headers
        .lines()
        .next()
        .ok_or_else(|| "browser engine control response missing status".to_string())?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| "browser engine control response invalid status".to_string())?;
    let body = &response[(header_end + 4)..];
    let json: Value = serde_json::from_slice(body)
        .map_err(|err| format!("browser engine control response invalid JSON: {err}"))?;
    if !(200..300).contains(&status) {
        return Err(json
            .get("error")
            .and_then(|value| value.as_str())
            .unwrap_or("browser engine control request failed")
            .to_string());
    }
    Ok(json)
}

pub(super) fn validate_supervisor_result(
    result: &SupervisorLaunchResult,
    adapter: &AdapterConfig,
    stream_session: &StreamSessionReceipt,
    expected_display_mode: BrowserDisplayMode,
) -> Result<(), String> {
    if result.schema != "elastos.browser.engine.supervisor-result/v1" {
        return Err("unsupported browser engine supervisor result schema".to_string());
    }
    if !is_safe_id(&result.page_id) {
        return Err("browser engine supervisor returned an unsafe page_id".to_string());
    }
    if result.adapter != adapter.id {
        return Err("browser engine supervisor adapter mismatch".to_string());
    }
    if result.engine != adapter.kind {
        return Err("browser engine supervisor engine mismatch".to_string());
    }
    if result.stream_id != stream_session.stream_id {
        return Err("browser engine supervisor stream_id mismatch".to_string());
    }
    if result.network_mode != AdapterNetworkMode::RuntimeNetOnly {
        return Err("browser engine supervisor must report runtime_net_only".to_string());
    }
    if result.direct_network {
        return Err("browser engine supervisor reported direct network authority".to_string());
    }
    if result.wallet_injection {
        return Err("browser engine supervisor reported wallet injection authority".to_string());
    }
    if let Some(isolation) = &result.isolation {
        if isolation.schema != "elastos.browser.engine.isolation/v1"
            || isolation.kind != "per_launch_selkies_target"
            || !isolation.session_dir.starts_with('/')
            || isolation.session_dir.contains(['\0', '\r', '\n'])
        {
            return Err(
                "browser engine supervisor returned invalid isolation metadata".to_string(),
            );
        }
    }
    validate_display_session(&result.display_session, expected_display_mode)?;
    Ok(())
}
