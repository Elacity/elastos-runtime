use super::*;

pub(super) fn parse_config(config: Value) -> Result<EngineConfig, String> {
    let config = match config.get("extra") {
        Some(extra) if extra.is_null() => json!({}),
        Some(extra) => extra.clone(),
        None if looks_like_bridge_provider_config(&config) => json!({}),
        None => config,
    };
    let config = serde_json::from_value::<EngineConfig>(config).map_err(|err| err.to_string())?;
    if config.max_active_sessions == 0 || config.max_active_sessions > 32 {
        return Err("browser max_active_sessions must be 1-32".to_string());
    }
    for adapter in &config.adapters {
        validate_adapter(adapter)?;
    }
    Ok(config)
}

pub(super) fn looks_like_bridge_provider_config(config: &Value) -> bool {
    config.get("base_path").is_some()
        || config.get("allowed_paths").is_some()
        || config.get("read_only").is_some()
        || config.get("encryption_key").is_some()
}

pub(super) fn validate_adapter(adapter: &AdapterConfig) -> Result<(), String> {
    if !is_safe_id(&adapter.id) {
        return Err("adapter id must be a safe identifier".to_string());
    }
    if adapter.network_mode != AdapterNetworkMode::RuntimeNetOnly {
        return Err("adapter network_mode must be runtime_net_only".to_string());
    }
    if adapter.display_modes.is_empty() {
        return Err("adapter must declare at least one display mode".to_string());
    }
    if adapter.kind == AdapterKind::ContractProof
        && adapter.display_modes != vec![BrowserDisplayMode::DiagnosticFrame]
    {
        return Err("contract_proof adapters may only declare diagnostic_frame".to_string());
    }
    match adapter.kind {
        AdapterKind::ContractProof => {
            if adapter.supervisor.is_some() {
                return Err(
                    "contract_proof adapters must not declare a native supervisor".to_string(),
                );
            }
        }
        _ => {
            let Some(supervisor) = &adapter.supervisor else {
                return Err("native browser adapters must declare a supervisor".to_string());
            };
            validate_supervisor(supervisor)?;
        }
    }
    Ok(())
}

pub(super) fn validate_supervisor(supervisor: &EngineSupervisorConfig) -> Result<(), String> {
    if supervisor.program.is_empty() || !supervisor.program.starts_with('/') {
        return Err("browser engine supervisor program must be absolute".to_string());
    }
    if supervisor.program.bytes().any(|byte| byte == b'\0') {
        return Err("browser engine supervisor program must not contain NUL".to_string());
    }
    if let Some(path) = &supervisor.control_socket_path {
        validate_control_socket_path(path)?;
    }
    if supervisor.timeout_ms < 100 || supervisor.timeout_ms > MAX_SUPERVISOR_TIMEOUT_MS {
        return Err("browser engine supervisor timeout_ms must be 100-300000".to_string());
    }
    if supervisor
        .args
        .iter()
        .any(|arg| arg.bytes().any(|byte| byte == b'\0'))
    {
        return Err("browser engine supervisor args must not contain NUL".to_string());
    }
    for (key, value) in &supervisor.env {
        if key.is_empty()
            || key.bytes().any(|byte| byte == b'=' || byte == b'\0')
            || value.bytes().any(|byte| byte == b'\0')
        {
            return Err(
                "browser engine supervisor env must use non-empty keys and no NUL".to_string(),
            );
        }
    }
    Ok(())
}

pub(super) fn validate_control_socket_path(path: &str) -> Result<(), String> {
    if path.is_empty() || !path.starts_with('/') {
        return Err("browser engine control_socket_path must be absolute".to_string());
    }
    if path
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte == b'\0')
    {
        return Err(
            "browser engine control_socket_path must not contain whitespace or NUL".to_string(),
        );
    }
    Ok(())
}

pub(super) fn validate_url(url: &str) -> Result<(), String> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("browser page URL must use http or https".to_string());
    }
    if url.contains(char::is_whitespace) {
        return Err("browser page URL must not contain whitespace".to_string());
    }
    Ok(())
}

pub(super) fn validate_viewport(viewport: ViewportRequest) -> Result<(), String> {
    if viewport.width < 320
        || viewport.width > 3840
        || viewport.height < 240
        || viewport.height > 2160
    {
        return Err("browser viewport must be within 320x240 and 3840x2160".to_string());
    }
    Ok(())
}

pub(super) fn validate_stream_session(receipt: &StreamSessionReceipt) -> Result<(), String> {
    if receipt.schema != "elastos.exit.stream-session/v1" {
        return Err("unsupported stream session schema".to_string());
    }
    if !is_safe_id(&receipt.stream_id) {
        return Err("stream_id must be a safe identifier".to_string());
    }
    if !receipt.target.starts_with("tls://") && !receipt.target.starts_with("tcp://") {
        return Err("stream session target must use tls or tcp".to_string());
    }
    if receipt.byte_transport.is_empty() {
        return Err("stream session missing byte_transport".to_string());
    }
    if receipt.byte_transport == "adapter_ipc" {
        validate_adapter_ipc(receipt)?;
    } else if receipt.adapter_ipc.is_some() {
        return Err("adapter_ipc descriptor requires adapter_ipc byte_transport".to_string());
    }
    if let Some(relay_ipc) = &receipt.relay_ipc {
        validate_relay_ipc(relay_ipc)?;
    }
    Ok(())
}

pub(super) fn validate_adapter_ipc(receipt: &StreamSessionReceipt) -> Result<(), String> {
    let Some(endpoint) = &receipt.adapter_ipc else {
        return Err("adapter_ipc byte transport requires an adapter_ipc descriptor".to_string());
    };
    if endpoint.schema != "elastos.adapter-ipc/v1" {
        return Err("unsupported adapter_ipc schema".to_string());
    }
    if endpoint.kind != AdapterIpcKind::UnixSocket {
        return Err("unsupported adapter_ipc kind".to_string());
    }
    if endpoint.stream_id != receipt.stream_id {
        return Err("adapter_ipc stream_id must match stream session stream_id".to_string());
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

pub(super) fn validate_relay_ipc(endpoint: &RelayIpcEndpoint) -> Result<(), String> {
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
