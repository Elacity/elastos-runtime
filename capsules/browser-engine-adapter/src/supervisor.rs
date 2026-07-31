use super::*;
use std::fs;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MAX_BROWSER_ENGINE_LAUNCH_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub(super) struct SupervisorLaunchError {
    pub(super) code: String,
    pub(super) message: String,
}

impl SupervisorLaunchError {
    fn process(message: impl Into<String>) -> Self {
        Self {
            code: "engine_process_unavailable".to_string(),
            message: message.into(),
        }
    }
}

#[derive(Deserialize)]
struct TypedSupervisorLaunchError {
    schema: String,
    code: String,
    message: String,
}

fn supervisor_launch_error(
    status: std::process::ExitStatus,
    stderr: &str,
) -> SupervisorLaunchError {
    if let Some(error) = stderr
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<TypedSupervisorLaunchError>(line).ok())
        .filter(|error| error.schema == "elastos.browser.engine.launch-error/v1")
    {
        return SupervisorLaunchError {
            code: error.code,
            message: error.message,
        };
    }
    SupervisorLaunchError::process(format!(
        "browser engine supervisor exited with status {}; {}",
        status,
        stderr.trim()
    ))
}

pub(super) fn run_supervisor_launch(
    supervisor: &EngineSupervisorConfig,
    adapter: &AdapterConfig,
    context: &LaunchContext<'_>,
    lifecycle_generation: &str,
    transport: Option<&VzTransportLaunchContext>,
) -> Result<SupervisorLaunchResult, SupervisorLaunchError> {
    let mut request = json!({
        "schema": "elastos.browser.engine.launch-request/v1",
        "adapter": &adapter.id,
        "engine": adapter.kind,
        "url": context.url,
        "stream_id": &context.stream_session.stream_id,
        "lifecycle_generation": lifecycle_generation,
        "target": &context.stream_session.target,
        "principal_id": &context.principal_id,
        "profile": &context.profile,
        "network_mode": adapter.network_mode,
        "direct_network": false,
        "wallet_injection": false,
        "adapter_ipc": &context.stream_session.adapter_ipc,
        "relay_ipc": &context.stream_session.relay_ipc,
        "wallet": &context.wallet,
        "viewport": context.viewport,
        "display_mode": context.display_mode,
        "guarantee_level": context.guarantee_level,
    });
    if let Some(transport) = transport {
        request["page_id"] = json!(transport.page_id);
        request["vm_id"] = json!(transport.vm_id);
        request["transport_authority"] = transport.authority.clone();
        request["transport_secret"] = transport.secret.clone();
    }
    let serialized = request.to_string();
    ensure_bounded_supervisor_request(&serialized)?;
    let mut command = Command::new(&supervisor.program);
    command
        .args(&supervisor.args)
        .envs(&supervisor.env)
        .env_remove("ELASTOS_BROWSER_VM_PREWARM_CONTROL_SERVICE")
        .env_remove("ELASTOS_BROWSER_ENGINE_REQUEST")
        .stdin(if transport.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if transport.is_none() {
        command.env("ELASTOS_BROWSER_ENGINE_REQUEST", &serialized);
    }
    let mut child = command
        .spawn()
        .map_err(|err| SupervisorLaunchError::process(err.to_string()))?;
    if transport.is_some() {
        write_private_supervisor_request(&mut child, serialized.as_bytes())?;
    }
    let deadline = Instant::now() + Duration::from_millis(supervisor.timeout_ms);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(SupervisorLaunchError::process(
                    "browser engine supervisor timed out",
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(err) => return Err(SupervisorLaunchError::process(err.to_string())),
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
        return Err(supervisor_launch_error(status, &stderr));
    }
    let result_value = serde_json::from_str::<Value>(stdout.trim()).map_err(|err| {
        SupervisorLaunchError::process(format!("invalid browser engine supervisor output: {err}"))
    })?;
    if transport.is_some_and(|transport| {
        value_contains_transport_secret(&result_value)
            || value_contains_exact_transport_secret(&result_value, &transport.secret)
    }) {
        return Err(SupervisorLaunchError::process(
            "browser engine supervisor exposed a private VZ transport secret",
        ));
    }
    let result = serde_json::from_value::<SupervisorLaunchResult>(result_value).map_err(|err| {
        SupervisorLaunchError::process(format!("invalid browser engine supervisor output: {err}"))
    })?;
    Ok(result)
}

fn ensure_bounded_supervisor_request(serialized: &str) -> Result<(), SupervisorLaunchError> {
    if serialized.len() <= MAX_BROWSER_ENGINE_LAUNCH_REQUEST_BYTES {
        Ok(())
    } else {
        Err(SupervisorLaunchError::process(
            "Browser engine launch request exceeds its bounded size",
        ))
    }
}

fn write_private_supervisor_request(
    child: &mut std::process::Child,
    serialized: &[u8],
) -> Result<(), SupervisorLaunchError> {
    let Some(mut stdin) = child.stdin.take() else {
        kill_and_reap(child);
        return Err(SupervisorLaunchError::process(
            "Browser VZ supervisor private request pipe is unavailable",
        ));
    };
    let result = stdin
        .write_all(serialized)
        .and_then(|_| stdin.write_all(b"\n"));
    drop(stdin);
    if let Err(err) = result {
        kill_and_reap(child);
        return Err(SupervisorLaunchError::process(format!(
            "Browser VZ supervisor private request write failed: {err}"
        )));
    }
    Ok(())
}

fn kill_and_reap(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

pub(super) fn run_supervisor_prewarm(supervisor: &EngineSupervisorConfig) -> Result<Value, String> {
    let mut child = Command::new(&supervisor.program)
        .args(&supervisor.args)
        .envs(&supervisor.env)
        .env("ELASTOS_BROWSER_VM_PREWARM_CONTROL_SERVICE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| err.to_string())?;
    let deadline = Instant::now() + Duration::from_millis(supervisor.timeout_ms.min(30_000));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("browser engine supervisor prewarm timed out".to_string());
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
            "browser engine supervisor prewarm exited with status {}; {}",
            status,
            stderr.trim()
        ));
    }
    let result = serde_json::from_str::<Value>(stdout.trim())
        .map_err(|err| format!("invalid browser engine supervisor prewarm output: {err}"))?;
    if result.get("schema").and_then(Value::as_str) != Some("elastos.browser.vm-engine-prewarm/v1")
        || result.get("ok").and_then(Value::as_bool) != Some(true)
        || result.get("network_mode").and_then(Value::as_str) != Some("runtime_net_only")
        || result.get("direct_network").and_then(Value::as_bool) != Some(false)
    {
        return Err("browser engine supervisor prewarm returned invalid readiness".to_string());
    }
    Ok(result)
}

pub(super) fn supervisor_control_json(
    socket_path: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    supervisor_control_json_inner(socket_path, method, path, body, None, None)
}

pub(super) fn supervisor_control_json_bounded(
    socket_path: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    timeout: Duration,
    max_response_bytes: usize,
) -> Result<Value, String> {
    supervisor_control_json_inner(
        socket_path,
        method,
        path,
        body,
        Some(timeout),
        Some(max_response_bytes),
    )
}

fn supervisor_control_json_inner(
    socket_path: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    timeout: Option<Duration>,
    max_response_bytes: Option<usize>,
) -> Result<Value, String> {
    validate_control_socket_path(socket_path)?;
    let body_bytes = body
        .map(|body| serde_json::to_vec(&body).map_err(|err| err.to_string()))
        .transpose()?
        .unwrap_or_default();
    let mut stream = std::os::unix::net::UnixStream::connect(socket_path)
        .map_err(|err| format!("browser engine control socket unavailable: {err}"))?;
    stream
        .set_read_timeout(timeout)
        .map_err(|err| format!("browser engine control read timeout setup failed: {err}"))?;
    stream
        .set_write_timeout(timeout)
        .map_err(|err| format!("browser engine control write timeout setup failed: {err}"))?;
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
    if let Some(max_response_bytes) = max_response_bytes {
        stream
            .take(max_response_bytes.saturating_add(1) as u64)
            .read_to_end(&mut response)
            .map_err(|err| err.to_string())?;
        if response.len() > max_response_bytes {
            return Err("browser engine control response exceeded its byte limit".to_string());
        }
    } else {
        stream
            .read_to_end(&mut response)
            .map_err(|err| err.to_string())?;
    }
    parse_http_json_response(&response)
}

pub(super) fn cleanup_isolated_session(
    session: &PageControlSession,
    binding: &EngineCleanupBinding,
) -> Result<Value, String> {
    let session_dir = session
        .isolation_session_dir
        .as_deref()
        .ok_or_else(|| "isolated browser session did not report a session directory".to_string())?;
    if session.isolation_kind.as_deref() == Some("per_launch_vm_target") {
        return Err(
            "Browser VM cleanup requires a typed terminal receipt from its control supervisor"
                .to_string(),
        );
    }
    cleanup_selkies_session(session_dir, &session.socket_path, binding)
}

fn cleanup_selkies_session(
    session_dir: &str,
    socket_path: &str,
    binding: &EngineCleanupBinding,
) -> Result<Value, String> {
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

    let _ = fs::remove_file(socket_path);

    if std::path::Path::new(socket_path).exists() {
        return Err("isolated Browser control socket still exists after cleanup".to_string());
    }
    if !cleanup_processes_are_absent(binding.process.as_ref()) {
        return Err("isolated Browser child process is still active after cleanup".to_string());
    }

    Ok(json!({
        "schema": BROWSER_SUPERVISOR_CLEANUP_RESULT_SCHEMA,
        "page_id": binding.page_id,
        "generation": binding.generation,
        "binding": binding,
        "terminal": true,
        "effects": {
            "page_absent": true,
            "child_absent": true,
            "vm_absent": true,
            "route_absent": true,
            "socket_absent": true,
        },
        "cleanup": {
            "schema": "elastos.browser.isolated-session-cleanup/v1",
            "session_dir": session_dir,
            "actions": actions,
        }
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

fn cleanup_processes_are_absent(process: Option<&Value>) -> bool {
    let Some(process) = process.and_then(Value::as_object) else {
        return false;
    };
    let pids = ["pid", "stream_bridge_pid"]
        .into_iter()
        .filter_map(|key| process.get(key).and_then(Value::as_u64))
        .filter(|pid| *pid > 0)
        .collect::<Vec<_>>();
    !pids.is_empty()
        && pids.into_iter().all(|pid| {
            !Command::new("kill")
                .args(["-0", &pid.to_string()])
                .status()
                .is_ok_and(|status| status.success())
        })
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
    expected_stream_id: &str,
    expected_display_mode: BrowserDisplayMode,
    expected_transport: Option<&VzTransportLaunchContext>,
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
    if result.stream_id != expected_stream_id {
        return Err("browser engine supervisor stream_id mismatch".to_string());
    }
    match expected_transport {
        Some(transport)
            if result.page_id == transport.page_id
                && result.vm_id.as_deref() == Some(transport.vm_id.as_str())
                && result.transport_authority.as_ref() == Some(&transport.authority) =>
        {
            validate_vz_transport_authority(&transport.authority)?;
            validate_vz_transport_effect_receipt(
                result.transport_receipt.as_ref().ok_or_else(|| {
                    "browser engine supervisor omitted the VZ transport effect receipt".to_string()
                })?,
                &transport.authority,
            )?;
            if result
                .display_session
                .get("ice_connection_policy")
                .and_then(Value::as_str)
                != Some("engine_relay_only")
                || result.display_session.get("ice_servers").is_some()
            {
                return Err(
                    "browser engine supervisor exposed an invalid VZ ICE policy".to_string()
                );
            }
        }
        Some(_) => {
            return Err("browser engine supervisor VZ transport authority mismatch".to_string())
        }
        None if result.vm_id.is_some()
            || result.transport_authority.is_some()
            || result.transport_receipt.is_some() =>
        {
            return Err(
                "browser engine supervisor returned unexpected VZ transport authority".to_string(),
            )
        }
        None => {}
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
            || !matches!(
                isolation.kind.as_str(),
                "per_launch_selkies_target" | "per_launch_vm_target"
            )
            || !isolation.session_dir.starts_with('/')
            || isolation.session_dir.contains(['\0', '\r', '\n'])
        {
            return Err(
                "browser engine supervisor returned invalid isolation metadata".to_string(),
            );
        }
        if isolation.kind == "per_launch_vm_target" {
            let control_service = result.control_service.as_ref().ok_or_else(|| {
                "Browser VM supervisor result omitted control-service identity".to_string()
            })?;
            validate_control_service_identity(
                control_service,
                Some(control_service.control_socket_path.as_str()),
            )?;
            if !host_process_binding_is_safe(result.process.as_ref()) {
                return Err(
                    "Browser VM supervisor result omitted exact host-process binding".to_string(),
                );
            }
            if result
                .display_session
                .get("media_transport")
                .and_then(|value| value.as_str())
                != Some("runtime_relay")
            {
                return Err(
                    "Browser VM display sessions must report media_transport=runtime_relay"
                        .to_string(),
                );
            }
            if expected_display_mode == BrowserDisplayMode::WebrtcRemoteDisplay
                && (result
                    .display_session
                    .get("audio")
                    .and_then(|value| value.as_bool())
                    != Some(true)
                    || result
                        .display_session
                        .get("video")
                        .and_then(|value| value.as_bool())
                        != Some(true))
            {
                return Err(
                    "Browser VM product display sessions must advertise audio=true and video=true"
                        .to_string(),
                );
            }
        }
    }
    if result.isolated_session
        && result
            .isolation
            .as_ref()
            .map(|isolation| isolation.kind.as_str())
            != Some("per_launch_vm_target")
    {
        let Some(process) = result.process.as_ref().and_then(Value::as_object) else {
            return Err(
                "isolated Browser supervisor result omitted exact child process ownership"
                    .to_string(),
            );
        };
        let pid = process.get("pid").and_then(Value::as_u64).unwrap_or(0);
        if pid == 0 || pid > u32::MAX as u64 {
            return Err(
                "isolated Browser supervisor result returned invalid child process ownership"
                    .to_string(),
            );
        }
        let stream_bridge_pid = process
            .get("stream_bridge_pid")
            .filter(|value| !value.is_null())
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if stream_bridge_pid > u32::MAX as u64 {
            return Err(
                "isolated Browser supervisor result returned invalid stream child ownership"
                    .to_string(),
            );
        }
    }
    validate_display_session(&result.display_session, expected_display_mode)?;
    if expected_display_mode == BrowserDisplayMode::NativeSurface {
        validate_native_surface_geometry(result)?;
    }
    Ok(())
}

fn validate_native_surface_geometry(result: &SupervisorLaunchResult) -> Result<(), String> {
    let view = result
        .view
        .as_ref()
        .ok_or_else(|| "native_surface supervisor result omitted view geometry".to_string())?;
    if view.get("schema").and_then(|value| value.as_str()) != Some("elastos.browser.view/v1")
        || view.get("mode").and_then(|value| value.as_str()) != Some("native_surface")
    {
        return Err("native_surface supervisor result returned invalid view geometry".to_string());
    }

    let view_width = native_surface_dimension(view, "width", "view")?;
    let view_height = native_surface_dimension(view, "height", "view")?;
    let display_width =
        native_surface_dimension(&result.display_session, "width", "display_session")?;
    let display_height =
        native_surface_dimension(&result.display_session, "height", "display_session")?;
    if view_width != display_width || view_height != display_height {
        return Err(
            "native_surface display dimensions must match Runtime view geometry".to_string(),
        );
    }
    Ok(())
}

fn native_surface_dimension(value: &Value, field: &str, label: &str) -> Result<u64, String> {
    let dimension = value
        .get(field)
        .and_then(|entry| entry.as_u64())
        .ok_or_else(|| format!("native_surface supervisor result {label}.{field} is required"))?;
    let valid = match field {
        "width" => (320..=3840).contains(&dimension),
        "height" => (240..=2160).contains(&dimension),
        _ => false,
    };
    if !valid {
        return Err(format!(
            "native_surface supervisor result {label}.{field} is outside the supported viewport range"
        ));
    }
    Ok(dimension)
}

#[cfg(test)]
mod private_request_tests {
    use super::*;

    #[test]
    fn serialized_launch_request_is_bounded() {
        assert!(ensure_bounded_supervisor_request(
            &"x".repeat(MAX_BROWSER_ENGINE_LAUNCH_REQUEST_BYTES)
        )
        .is_ok());
        assert!(ensure_bounded_supervisor_request(
            &"x".repeat(MAX_BROWSER_ENGINE_LAUNCH_REQUEST_BYTES + 1)
        )
        .is_err());
    }

    #[test]
    fn private_request_write_failure_kills_and_reaps_child() {
        let marker = std::env::temp_dir().join(format!(
            "elastos-browser-private-request-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut child = Command::new("/bin/sh")
            .args([
                "-c",
                "exec 0<&-; : > \"$1\"; sleep 30",
                "browser-private-request-test",
            ])
            .arg(&marker)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        for _ in 0..100 {
            if marker.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists());

        let result = write_private_supervisor_request(&mut child, b"private request");

        assert!(result.is_err());
        assert!(child.try_wait().unwrap().is_some());
        let _ = fs::remove_file(marker);
    }
}
