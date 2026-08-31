//! Runtime communication for capsules
//!
//! This module provides the messaging protocol for capsule-to-runtime communication.
//! Messages are sent via stdout and received via stdin using JSON format.

use std::io::{self, BufRead, Read, Write};
use std::time::Duration;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Request ID for correlating requests and responses
pub type RequestId = u64;

/// Request from capsule to the capsule-kernel bridge.
#[cfg(feature = "serde")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeRequest {
    /// Request a capability token (capsule→shell, waits for approval)
    RequestCapability {
        resource: String,
        action: String,
        #[serde(default)]
        reason: String,
    },

    /// Invoke an ElastOS resource through the capsule-kernel resource contract.
    ResourceInvoke {
        uri: String,
        operation: String,
        #[serde(default)]
        body: serde_json::Value,
        #[serde(default)]
        token: String,
    },

    /// Get runtime info
    GetRuntimeInfo,

    /// Ping (health check)
    Ping,
}

/// Response from the capsule-kernel bridge.
#[cfg(feature = "serde")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeResponse {
    /// Success with optional data
    Ok {
        #[serde(default)]
        data: Option<serde_json::Value>,
    },

    /// Error response
    Error { code: String, message: String },

    /// Capability token received (capsule requested, shell approved)
    CapabilityToken { token: String },

    /// Resource invoke result
    ResourceResult {
        result: serde_json::Value,
        #[serde(default)]
        audit: Option<String>,
    },

    /// Runtime info
    RuntimeInfo {
        version: String,
        capsule_count: usize,
    },

    /// Pong response
    Pong,
}

/// Message envelope for wire protocol
#[cfg(feature = "serde")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub id: RequestId,
    pub request: RuntimeRequest,
}

/// Response envelope for wire protocol
#[cfg(feature = "serde")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub id: RequestId,
    pub response: RuntimeResponse,
}

/// Transport channel for the runtime client.
/// Detected automatically based on the environment.
#[cfg(feature = "serde")]
enum RuntimeBridgeChannel {
    /// Runtime-provided host import for WASM capsules.
    #[cfg(target_os = "wasi")]
    HostCall,
    /// stdin/stdout — explicit standalone host-process mode when no bridge is configured
    Stdio,
    /// Dedicated full-duplex serial device for microVM capsules.
    #[cfg(not(target_os = "wasi"))]
    Serial { file: std::fs::File },
    /// Reader/writer pair for bridge-backed WASM capsules (in-process).
    FilePair {
        reader: io::BufReader<std::fs::File>,
        writer: std::fs::File,
    },
    /// HTTP API to a running runtime (attached mode).
    /// WASM capsules running locally use this to reach the runtime resource bridge.
    Http { api_url: String, token: String },
}

#[cfg(all(feature = "serde", target_os = "wasi"))]
#[link(wasm_import_module = "elastos")]
unsafe extern "C" {
    #[link_name = "carrier_call"]
    fn elastos_carrier_call(
        request_ptr: *const u8,
        request_len: usize,
        response_ptr: *mut u8,
        response_cap: usize,
        response_len_ptr: *mut usize,
    ) -> i32;
}

/// Runtime client for capsules.
///
/// Communicates with the ElastOS runtime via the runtime resource bridge. The transport is
/// detected automatically:
/// - `ELASTOS_CARRIER_FIFOS` set (e.g., "/_carrier/response,/_carrier/request")
///   → open those FIFO paths (WASM bridge mode, wasmtime 24+ compatible)
/// - `ELASTOS_CARRIER_PATH` set → use that file (e.g., /dev/hvc0 for microVM virtio-console)
/// - Otherwise → use stdin/stdout (standalone host-process mode, no bridge)
///
/// Capsule code doesn't change between substrates. Just use `RuntimeClient::new()`.
#[cfg(feature = "serde")]
pub struct RuntimeClient {
    next_id: RequestId,
    channel: RuntimeBridgeChannel,
}

#[cfg(feature = "serde")]
impl RuntimeClient {
    /// Return true when the host attached a capsule-kernel bridge.
    ///
    /// This checks only the boot contract. It does not prove that the runtime
    /// will grant any capability.
    pub fn is_bridge_configured() -> bool {
        if std::env::var_os("ELASTOS_CARRIER_HOSTCALL").is_some()
            || std::env::var_os("ELASTOS_CARRIER_FIFOS").is_some()
            || std::env::var_os("ELASTOS_CARRIER_PATH").is_some()
        {
            return true;
        }
        matches!(
            (std::env::var("ELASTOS_API"), std::env::var("ELASTOS_TOKEN")),
            (Ok(api), Ok(token)) if !api.is_empty() && !token.is_empty()
        )
    }

    /// Create a new runtime client.
    ///
    /// Detects the runtime bridge channel automatically:
    /// 1. `ELASTOS_CARRIER_FIFOS=reader_path,writer_path` → FIFO pair via
    ///    preopened-dir (WASM bridge, wasmtime 24+ compatible)
    /// 2. `ELASTOS_CARRIER_PATH=/dev/hvc0` → file-based (microVM virtio-console device)
    /// 3. `ELASTOS_API` + `ELASTOS_TOKEN` → HTTP API to running runtime (attached mode)
    /// 4. Otherwise → stdin/stdout (standalone host-process mode)
    pub fn new() -> Self {
        let channel = if std::env::var_os("ELASTOS_CARRIER_HOSTCALL").is_some() {
            #[cfg(target_os = "wasi")]
            {
                RuntimeBridgeChannel::HostCall
            }
            #[cfg(not(target_os = "wasi"))]
            {
                panic!("ELASTOS_CARRIER_HOSTCALL is only supported inside WASI capsules")
            }
        } else if std::env::var_os("ELASTOS_CARRIER_FIFOS").is_some() {
            Self::channel_from_fifos()
                .unwrap_or_else(|e| panic!("ELASTOS_CARRIER_FIFOS is set but invalid: {e}"))
        } else if let Ok(path) = std::env::var("ELASTOS_CARRIER_PATH") {
            // MicroVM: use one kept-open serial fd for the runtime bridge. Avoid BufReader
            // and split reader/writer handles on tty devices; they are too easy
            // to deadlock or confuse with line discipline and echo behavior.
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
            {
                Ok(file) => {
                    #[cfg(not(target_os = "wasi"))]
                    {
                        use std::os::fd::AsRawFd;
                        let fd = file.as_raw_fd();
                        if let Err(e) = Self::configure_serial_raw_fd(fd) {
                            eprintln!(
                                "[elastos-guest] warning: failed to configure raw serial mode on {}: {}",
                                path, e
                            );
                        }
                        RuntimeBridgeChannel::Serial { file }
                    }
                    #[cfg(target_os = "wasi")]
                    {
                        let _ = file;
                        RuntimeBridgeChannel::Stdio
                    }
                }
                Err(e) => {
                    panic!(
                        "ELASTOS_CARRIER_PATH is set to {} but the device could not be opened: {}",
                        path, e
                    )
                }
            }
        } else if let (Ok(api_url), Ok(token)) =
            (std::env::var("ELASTOS_API"), std::env::var("ELASTOS_TOKEN"))
        {
            if !api_url.is_empty() && !token.is_empty() {
                RuntimeBridgeChannel::Http { api_url, token }
            } else {
                RuntimeBridgeChannel::Stdio
            }
        } else {
            RuntimeBridgeChannel::Stdio
        };

        Self {
            next_id: 1,
            channel,
        }
    }

    /// Configure a serial device fd for raw mode (no echo, no line discipline).
    /// Must be called on a fd that stays open — Linux resets TTY settings
    /// when all fds to the device close.
    #[cfg(not(target_os = "wasi"))]
    fn configure_serial_raw_fd(fd: i32) -> io::Result<()> {
        unsafe {
            let mut termios: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut termios) != 0 {
                return Err(io::Error::last_os_error());
            }
            libc::cfmakeraw(&mut termios);
            // On real UART-backed ttys, raw mode is not enough by itself:
            // keep the receiver enabled and ignore modem-control gating so
            // host→guest traffic on the Carrier serial port is actually read.
            termios.c_cflag |= libc::CREAD | libc::CLOCAL;
            termios.c_cc[libc::VMIN] = 1;
            termios.c_cc[libc::VTIME] = 0;
            if libc::tcsetattr(fd, libc::TCSANOW, &termios) != 0 {
                return Err(io::Error::last_os_error());
            }
            let _ = libc::tcflush(fd, libc::TCIOFLUSH);
        }
        Ok(())
    }

    /// Execute an SDK request via HTTP API to the running runtime.
    /// Maps capsule-kernel requests to host-adapter HTTP calls.
    #[cfg(feature = "serde")]
    fn http_call(
        _id: RequestId,
        request: &RuntimeRequest,
        api_url: &str,
        token: &str,
    ) -> io::Result<RuntimeResponse> {
        let (path, body, cap_token) = match request {
            RuntimeRequest::RequestCapability {
                resource,
                action,
                reason,
            } => (
                "/api/capability/request".to_string(),
                serde_json::json!({"resource": resource, "action": action, "reason": reason}),
                None,
            ),
            RuntimeRequest::ResourceInvoke {
                uri,
                operation,
                body,
                token: cap_token,
            } => (
                format!(
                    "/api/provider/{}/{}",
                    Self::provider_scheme_for_uri(uri)?,
                    operation
                ),
                Self::resource_body_for_http(uri, body),
                if cap_token.is_empty() {
                    None
                } else {
                    Some(cap_token.as_str())
                },
            ),
            RuntimeRequest::Ping => {
                return Ok(RuntimeResponse::Pong);
            }
            RuntimeRequest::GetRuntimeInfo => {
                return Ok(RuntimeResponse::RuntimeInfo {
                    version: "attached".to_string(),
                    capsule_count: 0,
                });
            }
        };

        let body_str = serde_json::to_string(&body)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        // Simple blocking HTTP POST via TcpStream (no external deps)
        let url = format!("{}{}", api_url, path);
        let resp_body = Self::http_post(&url, token, &body_str, cap_token)?;

        let resp_json: serde_json::Value = serde_json::from_str(&resp_body).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("API response: {}", e))
        })?;

        // Map API responses back to SDK response types
        match request {
            RuntimeRequest::RequestCapability { .. } => {
                if resp_json.get("status").and_then(|s| s.as_str()) == Some("denied") {
                    return Ok(RuntimeResponse::Error {
                        code: "denied".to_string(),
                        message: resp_json
                            .get("reason")
                            .and_then(|r| r.as_str())
                            .unwrap_or("denied")
                            .to_string(),
                    });
                }

                // The capability API returns a request_id for pending requests.
                // Poll the request status until the shell grants it.
                if let Some(req_id) = resp_json.get("request_id").and_then(|r| r.as_str()) {
                    // Poll for grant (shell auto-grants via AutoGrantEngine)
                    let status_url = format!("{}/api/capability/request/{}", api_url, req_id);
                    for _ in 0..30 {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        if let Ok(status_body) = Self::http_get(&status_url, token) {
                            if let Ok(status) =
                                serde_json::from_str::<serde_json::Value>(&status_body)
                            {
                                if let Some(tok) = status.get("token").and_then(|t| t.as_str()) {
                                    return Ok(RuntimeResponse::CapabilityToken {
                                        token: tok.to_string(),
                                    });
                                }
                                if status.get("status").and_then(|s| s.as_str()) == Some("denied") {
                                    return Ok(RuntimeResponse::Error {
                                        code: "denied".to_string(),
                                        message: status
                                            .get("reason")
                                            .and_then(|r| r.as_str())
                                            .unwrap_or("denied")
                                            .to_string(),
                                    });
                                }
                                if status.get("status").and_then(|s| s.as_str()) == Some("expired")
                                {
                                    return Ok(RuntimeResponse::Error {
                                        code: "expired".to_string(),
                                        message: status
                                            .get("reason")
                                            .and_then(|r| r.as_str())
                                            .unwrap_or("expired")
                                            .to_string(),
                                    });
                                }
                            }
                        }
                    }
                    // Timeout — still pending
                    Ok(RuntimeResponse::Error {
                        code: "timeout".to_string(),
                        message: "capability request still pending after 3s".to_string(),
                    })
                } else if let Some(tok) = resp_json.get("token").and_then(|t| t.as_str()) {
                    Ok(RuntimeResponse::CapabilityToken {
                        token: tok.to_string(),
                    })
                } else {
                    Ok(RuntimeResponse::Error {
                        code: "invalid_response".to_string(),
                        message: "capability response missing request_id and token".to_string(),
                    })
                }
            }
            RuntimeRequest::ResourceInvoke { .. } => Ok(RuntimeResponse::ResourceResult {
                result: resp_json,
                audit: None,
            }),
            RuntimeRequest::Ping | RuntimeRequest::GetRuntimeInfo => Ok(RuntimeResponse::Ok {
                data: Some(resp_json),
            }),
        }
    }

    fn provider_scheme_for_uri(uri: &str) -> io::Result<String> {
        if uri.starts_with("localhost://") {
            return Ok("localhost".to_string());
        }
        if let Some(rest) = uri.strip_prefix("elastos://") {
            let head = rest
                .split(['/', '?', '#'])
                .next()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "elastos URI missing provider")
                })?;
            return Ok(head.to_string());
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "resource URI must use elastos:// or localhost://",
        ))
    }

    fn resource_body_for_http(uri: &str, body: &serde_json::Value) -> serde_json::Value {
        let mut body = body.clone();
        if uri.starts_with("localhost://") && body.get("path").is_none() {
            body["path"] = serde_json::Value::String(uri.to_string());
        }
        if body.get("network").is_none() {
            if let Some(network) = uri
                .strip_prefix("elastos://chain/")
                .and_then(|rest| rest.split('/').next())
                .filter(|network| !network.is_empty() && *network != "meta")
            {
                body["network"] = serde_json::Value::String(network.to_string());
            }
        }
        body
    }

    /// Minimal blocking HTTP GET (no external dependencies).
    #[cfg(feature = "serde")]
    fn http_get(url: &str, auth_token: &str) -> io::Result<String> {
        use std::net::TcpStream;

        let url = url.strip_prefix("http://").unwrap_or(url);
        let (host_port, path) = url.split_once('/').unwrap_or((url, ""));
        let path = format!("/{}", path);

        let mut stream = TcpStream::connect(host_port)?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;

        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
            path, host_port, auth_token
        );

        stream.write_all(request.as_bytes())?;
        stream.flush()?;

        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response);
        let response_str = String::from_utf8_lossy(&response);

        if let Some(body_start) = response_str.find("\r\n\r\n") {
            Ok(response_str[body_start + 4..].to_string())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "malformed HTTP response",
            ))
        }
    }

    /// Minimal blocking HTTP POST (no external dependencies).
    #[cfg(feature = "serde")]
    fn http_post(
        url: &str,
        auth_token: &str,
        body: &str,
        cap_token: Option<&str>,
    ) -> io::Result<String> {
        use std::net::TcpStream;

        // Parse URL: http://host:port/path
        let url = url.strip_prefix("http://").unwrap_or(url);
        let (host_port, path) = url.split_once('/').unwrap_or((url, ""));
        let path = format!("/{}", path);

        let mut stream = TcpStream::connect(host_port)?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;

        let mut request = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
            path, host_port, auth_token, body.len()
        );
        if let Some(cap_token) = cap_token {
            request.push_str(&format!("X-Capability-Token: {}\r\n", cap_token));
        }
        request.push_str("\r\n");
        request.push_str(body);

        stream.write_all(request.as_bytes())?;
        stream.flush()?;

        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response);
        let response_str = String::from_utf8_lossy(&response);

        // Extract body after \r\n\r\n
        if let Some(body_start) = response_str.find("\r\n\r\n") {
            let body = &response_str[body_start + 4..];
            // Handle chunked transfer encoding
            if response_str.contains("Transfer-Encoding: chunked") {
                // Simple chunked decoder
                let mut result = String::new();
                let mut remaining = body;
                loop {
                    let line_end = remaining.find("\r\n").unwrap_or(remaining.len());
                    let chunk_size =
                        usize::from_str_radix(remaining[..line_end].trim(), 16).unwrap_or(0);
                    if chunk_size == 0 {
                        break;
                    }
                    let chunk_start = line_end + 2;
                    let chunk_end = chunk_start + chunk_size;
                    if chunk_end <= remaining.len() {
                        result.push_str(&remaining[chunk_start..chunk_end]);
                        remaining = &remaining[chunk_end..];
                        if remaining.starts_with("\r\n") {
                            remaining = &remaining[2..];
                        }
                    } else {
                        result.push_str(&remaining[chunk_start..]);
                        break;
                    }
                }
                Ok(result)
            } else {
                Ok(body.to_string())
            }
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "malformed HTTP response",
            ))
        }
    }

    #[cfg(target_os = "wasi")]
    fn call_hostcall(json: &str) -> io::Result<String> {
        const MAX_RESPONSE_BYTES: usize = 1_048_576;
        let mut response = vec![0_u8; MAX_RESPONSE_BYTES];
        let mut response_len: usize = 0;
        let code = unsafe {
            elastos_carrier_call(
                json.as_ptr(),
                json.len(),
                response.as_mut_ptr(),
                response.len(),
                &mut response_len as *mut usize,
            )
        };

        match code {
            0 => {
                if response_len > response.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "carrier hostcall returned an invalid response length",
                    ));
                }
                String::from_utf8(response[..response_len].to_vec()).map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("carrier hostcall utf8: {e}"),
                    )
                })
            }
            1 => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "carrier hostcall rejected guest memory",
            )),
            2 => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "carrier hostcall request was not UTF-8",
            )),
            3 => Err(io::Error::other("carrier hostcall failed")),
            4 => Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "carrier hostcall response exceeded guest buffer",
            )),
            other => Err(io::Error::other(format!(
                "carrier hostcall returned unknown status {other}"
            ))),
        }
    }

    /// Try to open a runtime bridge channel from ELASTOS_CARRIER_FIFOS env var.
    ///
    /// Format: `"reader_path,writer_path"` — sandbox-relative paths (e.g.,
    /// `"/_carrier/response,/_carrier/request"`). `reader_path` is the FIFO the
    /// capsule reads from (host writes SDK responses to it); `writer_path` is
    /// the FIFO the capsule writes to (host reads SDK requests from it).
    ///
    /// Modern `wasmtime-wasi` removed fd injection for custom handles, so the
    /// runtime preopens a per-launch directory containing both FIFOs via
    /// `WasiCtxBuilder::preopened_dir()`. The capsule opens them by path.
    /// stdin/stdout remain inherited for user I/O.
    fn channel_from_fifos() -> io::Result<RuntimeBridgeChannel> {
        let env = std::env::var("ELASTOS_CARRIER_FIFOS")
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let parts: Vec<&str> = env.split(',').collect();
        if parts.len() != 2 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected ELASTOS_CARRIER_FIFOS=reader_path,writer_path",
            ));
        }
        let reader_path = parts[0].trim();
        let writer_path = parts[1].trim();
        if reader_path.is_empty() || writer_path.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ELASTOS_CARRIER_FIFOS paths must be non-empty",
            ));
        }

        let reader = std::fs::OpenOptions::new()
            .read(true)
            .open(reader_path)
            .map_err(|e| io::Error::new(e.kind(), format!("open {reader_path} (read): {e}")))?;
        let writer = std::fs::OpenOptions::new()
            .write(true)
            .open(writer_path)
            .map_err(|e| io::Error::new(e.kind(), format!("open {writer_path} (write): {e}")))?;

        Ok(RuntimeBridgeChannel::FilePair {
            reader: io::BufReader::new(reader),
            writer,
        })
    }

    #[cfg(any(test, not(target_os = "wasi")))]
    fn read_unbuffered_line<R: Read>(reader: &mut R) -> io::Result<String> {
        let mut bytes = Vec::with_capacity(256);
        let mut byte = [0u8; 1];
        loop {
            match reader.read(&mut byte) {
                Ok(0) => {
                    if bytes.is_empty() {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "runtime bridge channel closed",
                        ));
                    }
                    break;
                }
                Ok(_) => match byte[0] {
                    b'\n' => break,
                    b'\r' => {}
                    b => bytes.push(b),
                },
                Err(e) => return Err(e),
            }
        }

        String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    #[cfg(not(target_os = "wasi"))]
    fn serial_write_line(file: &mut std::fs::File, json: &str) -> io::Result<()> {
        use std::os::fd::AsRawFd;

        file.write_all(json.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;

        unsafe {
            let fd = file.as_raw_fd();
            if libc::isatty(fd) == 1 {
                let _ = libc::tcdrain(fd);
            }
        }
        Ok(())
    }

    /// Send a request to the runtime and wait for response
    pub fn call(&mut self, request: RuntimeRequest) -> io::Result<RuntimeResponse> {
        let id = self.next_id;
        self.next_id += 1;

        let envelope = RequestEnvelope { id, request };
        let json = serde_json::to_string(&envelope)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        // Send request and read response via the detected channel
        let line = match &mut self.channel {
            #[cfg(target_os = "wasi")]
            RuntimeBridgeChannel::HostCall => Self::call_hostcall(&json)?,
            RuntimeBridgeChannel::Stdio => {
                let mut stdout = io::stdout().lock();
                writeln!(stdout, "{}", json)?;
                stdout.flush()?;

                let stdin = io::stdin();
                let mut line = String::new();
                stdin.lock().read_line(&mut line)?;
                line
            }
            #[cfg(not(target_os = "wasi"))]
            RuntimeBridgeChannel::Serial { file } => {
                Self::serial_write_line(file, &json)?;
                Self::read_unbuffered_line(file)?
            }
            RuntimeBridgeChannel::FilePair { reader, writer } => {
                writeln!(writer, "{}", json).map_err(|e| {
                    io::Error::new(e.kind(), format!("carrier fifo write failed: {e}"))
                })?;
                writer.flush().map_err(|e| {
                    io::Error::new(e.kind(), format!("carrier fifo flush failed: {e}"))
                })?;

                let mut line = String::new();
                reader.read_line(&mut line).map_err(|e| {
                    io::Error::new(e.kind(), format!("carrier fifo read failed: {e}"))
                })?;
                line
            }
            RuntimeBridgeChannel::Http { api_url, token } => {
                // Translate SDK request into HTTP API call to the running runtime.
                return Self::http_call(id, &envelope.request, api_url, token);
            }
        };

        let resp_envelope: ResponseEnvelope = serde_json::from_str(&line).map_err(|e| {
            let preview: String = line.chars().take(1024).collect();
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("runtime response decode failed: {e}; response={preview}"),
            )
        })?;

        if resp_envelope.id != id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "response id mismatch",
            ));
        }

        Ok(resp_envelope.response)
    }

    /// Send a request with a timeout (default 30 seconds).
    ///
    /// Spawns a blocking reader on a separate thread and waits up to `timeout`
    /// for the response. Returns `ErrorKind::TimedOut` on expiry.
    pub fn call_with_timeout(
        &mut self,
        request: RuntimeRequest,
        timeout: Duration,
    ) -> io::Result<RuntimeResponse> {
        if !matches!(self.channel, RuntimeBridgeChannel::Stdio) {
            return self.call(request);
        }

        let id = self.next_id;
        self.next_id += 1;

        let envelope = RequestEnvelope { id, request };
        let json = serde_json::to_string(&envelope)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        // Send request via the detected channel
        match &mut self.channel {
            RuntimeBridgeChannel::Stdio => {
                let mut stdout = io::stdout().lock();
                writeln!(stdout, "{}", json)?;
                stdout.flush()?;
            }
            #[cfg(not(target_os = "wasi"))]
            RuntimeBridgeChannel::Serial { .. }
            | RuntimeBridgeChannel::FilePair { .. }
            | RuntimeBridgeChannel::Http { .. } => unreachable!(),
            #[cfg(target_os = "wasi")]
            RuntimeBridgeChannel::HostCall
            | RuntimeBridgeChannel::FilePair { .. }
            | RuntimeBridgeChannel::Http { .. } => unreachable!(),
        }

        // Read response with timeout — spawn a reader thread
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let stdin = io::stdin();
            let mut line = String::new();
            let result = stdin.lock().read_line(&mut line).map(|_| line);
            let _ = tx.send(result);
        });

        let line = rx
            .recv_timeout(timeout)
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "runtime call timed out"))?
            .map_err(|e| io::Error::new(e.kind(), e))?;

        let resp_envelope: ResponseEnvelope = serde_json::from_str(&line).map_err(|e| {
            let preview: String = line.chars().take(1024).collect();
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("runtime response decode failed: {e}; response={preview}"),
            )
        })?;

        if resp_envelope.id != id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "response id mismatch",
            ));
        }

        Ok(resp_envelope.response)
    }

    /// Default timeout for `call_with_timeout` (30 seconds).
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

    /// Get runtime info
    pub fn get_runtime_info(&mut self) -> io::Result<(String, usize)> {
        match self.call(RuntimeRequest::GetRuntimeInfo)? {
            RuntimeResponse::RuntimeInfo {
                version,
                capsule_count,
            } => Ok((version, capsule_count)),
            RuntimeResponse::Error { code, message } => {
                Err(io::Error::other(format!("{}: {}", code, message)))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response",
            )),
        }
    }

    /// Ping the runtime
    pub fn ping(&mut self) -> io::Result<()> {
        match self.call(RuntimeRequest::Ping)? {
            RuntimeResponse::Pong => Ok(()),
            RuntimeResponse::Error { code, message } => {
                Err(io::Error::other(format!("{}: {}", code, message)))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response",
            )),
        }
    }

    /// Request a capability token from the shell.
    /// Blocks until the shell grants or denies the request.
    pub fn request_capability(&mut self, resource: &str, action: &str) -> io::Result<String> {
        match self.call(RuntimeRequest::RequestCapability {
            resource: resource.to_string(),
            action: action.to_string(),
            reason: String::new(),
        })? {
            RuntimeResponse::CapabilityToken { token } => Ok(token),
            RuntimeResponse::Error { code, message } => {
                Err(io::Error::other(format!("{}: {}", code, message)))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response",
            )),
        }
    }

    /// Invoke an ElastOS resource through the capsule-kernel resource contract.
    ///
    /// Capsule code supplies a resource URI and operation. The runtime decides
    /// which local or remote provider handles it.
    pub fn resource_invoke(
        &mut self,
        uri: &str,
        operation: &str,
        body: &serde_json::Value,
        token: &str,
    ) -> io::Result<serde_json::Value> {
        match self.call(RuntimeRequest::ResourceInvoke {
            uri: uri.to_string(),
            operation: operation.to_string(),
            body: body.clone(),
            token: token.to_string(),
        })? {
            RuntimeResponse::ResourceResult { result, .. } => Ok(result),
            RuntimeResponse::Ok { data } => Ok(data.unwrap_or(serde_json::json!({}))),
            RuntimeResponse::Error { code, message } => {
                Err(io::Error::other(format!("{}: {}", code, message)))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response",
            )),
        }
    }
}

#[cfg(feature = "serde")]
impl Default for RuntimeClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "serde")]
    use super::*;
    use std::io::Cursor;
    #[cfg(all(feature = "serde", not(target_os = "wasi")))]
    use std::io::{BufRead, Write};
    #[cfg(all(feature = "serde", not(target_os = "wasi")))]
    use std::os::fd::FromRawFd;
    #[cfg(all(feature = "serde", not(target_os = "wasi")))]
    use std::sync::Mutex;

    #[cfg(all(feature = "serde", not(target_os = "wasi")))]
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[cfg(all(feature = "serde", not(target_os = "wasi")))]
    fn restore_env(key: &str, value: Option<String>) {
        if let Some(value) = value {
            std::env::set_var(key, value);
        } else {
            std::env::remove_var(key);
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_request_serialization() {
        let req = RuntimeRequest::RequestCapability {
            resource: "elastos://did/*".to_string(),
            action: "execute".to_string(),
            reason: String::new(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("request_capability"));
        assert!(json.contains("elastos://did/*"));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_response_serialization() {
        let resp = RuntimeResponse::ResourceResult {
            result: serde_json::json!({"status": "ok"}),
            audit: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("resource_result"));
        assert!(json.contains("ok"));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_envelope_serialization() {
        let envelope = RequestEnvelope {
            id: 42,
            request: RuntimeRequest::Ping,
        };
        let json = serde_json::to_string(&envelope).unwrap();
        assert!(json.contains("42"));
        assert!(json.contains("ping"));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_carrier_uri_maps_to_host_adapter_scheme() {
        assert_eq!(
            RuntimeClient::provider_scheme_for_uri("localhost://Users/self/Documents/a.md")
                .unwrap(),
            "localhost"
        );
        assert_eq!(
            RuntimeClient::provider_scheme_for_uri("elastos://did/*").unwrap(),
            "did"
        );
        assert!(RuntimeClient::provider_scheme_for_uri("https://example.com").is_err());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_carrier_body_adds_host_adapter_defaults() {
        let localhost = RuntimeClient::resource_body_for_http(
            "localhost://Users/self/Documents/a.md",
            &serde_json::json!({}),
        );
        assert_eq!(
            localhost.get("path").and_then(|value| value.as_str()),
            Some("localhost://Users/self/Documents/a.md")
        );

        let chain = RuntimeClient::resource_body_for_http(
            "elastos://chain/esc-mainnet/block_number",
            &serde_json::json!({}),
        );
        assert_eq!(
            chain.get("network").and_then(|value| value.as_str()),
            Some("esc-mainnet")
        );
    }

    #[test]
    fn test_read_unbuffered_line_reads_multiple_lines() {
        let mut cursor = Cursor::new(b"hello\nworld\n".to_vec());
        assert_eq!(
            RuntimeClient::read_unbuffered_line(&mut cursor).unwrap(),
            "hello"
        );
        assert_eq!(
            RuntimeClient::read_unbuffered_line(&mut cursor).unwrap(),
            "world"
        );
    }

    #[test]
    fn test_read_unbuffered_line_strips_crlf() {
        let mut cursor = Cursor::new(b"hello\r\n".to_vec());
        assert_eq!(
            RuntimeClient::read_unbuffered_line(&mut cursor).unwrap(),
            "hello"
        );
    }

    #[cfg(all(feature = "serde", not(target_os = "wasi")))]
    #[test]
    fn test_bridge_configured_detects_runtime_boot_contract() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_fifos = std::env::var("ELASTOS_CARRIER_FIFOS").ok();
        let old_path = std::env::var("ELASTOS_CARRIER_PATH").ok();
        let old_api = std::env::var("ELASTOS_API").ok();
        let old_token = std::env::var("ELASTOS_TOKEN").ok();

        std::env::remove_var("ELASTOS_CARRIER_FIFOS");
        std::env::remove_var("ELASTOS_CARRIER_PATH");
        std::env::remove_var("ELASTOS_API");
        std::env::remove_var("ELASTOS_TOKEN");
        assert!(!RuntimeClient::is_bridge_configured());

        std::env::set_var(
            "ELASTOS_CARRIER_FIFOS",
            "/_carrier/response,/_carrier/request",
        );
        assert!(RuntimeClient::is_bridge_configured());
        std::env::remove_var("ELASTOS_CARRIER_FIFOS");

        std::env::set_var("ELASTOS_CARRIER_PATH", "/dev/hvc0");
        assert!(RuntimeClient::is_bridge_configured());
        std::env::remove_var("ELASTOS_CARRIER_PATH");

        std::env::set_var("ELASTOS_API", "http://127.0.0.1:3000");
        assert!(!RuntimeClient::is_bridge_configured());
        std::env::set_var("ELASTOS_TOKEN", "token");
        assert!(RuntimeClient::is_bridge_configured());

        restore_env("ELASTOS_CARRIER_FIFOS", old_fifos);
        restore_env("ELASTOS_CARRIER_PATH", old_path);
        restore_env("ELASTOS_API", old_api);
        restore_env("ELASTOS_TOKEN", old_token);
    }

    #[cfg(all(feature = "serde", not(target_os = "wasi")))]
    #[test]
    fn test_runtime_client_two_calls_over_tty_serial() {
        let _guard = ENV_LOCK.lock().unwrap();
        let token_payload = "tok-1".repeat(256);

        unsafe {
            let mut master_fd = -1;
            let mut slave_fd = -1;
            // c_char is i8 on x86_64/macOS but u8 on Linux aarch64; the
            // libc alias keeps the buffer portable across targets.
            let mut name = [0 as libc::c_char; 128];

            // Newer libc expects mutable termios/winsize pointers here.
            let rc = libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                name.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            assert_eq!(rc, 0, "openpty failed: {}", std::io::Error::last_os_error());
            assert!(master_fd >= 0);
            assert!(slave_fd >= 0);

            let slave_path = std::ffi::CStr::from_ptr(name.as_ptr())
                .to_str()
                .unwrap()
                .to_string();

            // Keep one slave fd open so the pty master does not see EIO before
            // the client re-opens the slave path on its own.
            let _slave_keeper = std::fs::File::from_raw_fd(slave_fd);

            let mut master = std::fs::File::from_raw_fd(master_fd);
            let bridge_token = token_payload.clone();
            let bridge = std::thread::spawn(move || -> Vec<String> {
                let mut reader = std::io::BufReader::new(master.try_clone().unwrap());
                let mut line = String::new();
                let mut seen = Vec::new();

                // Request 1: request_capability
                reader.read_line(&mut line).unwrap();
                seen.push(line.clone());
                assert!(
                    line.contains("\"request_capability\""),
                    "unexpected first request: {line}"
                );
                master
                    .write_all(
                        format!(
                            "{{\"id\":1,\"response\":{{\"type\":\"capability_token\",\"token\":\"{}\"}}}}",
                            bridge_token
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                master.write_all(b"\n").unwrap();
                master.flush().unwrap();

                line.clear();

                // Request 2: resource_invoke(get_did)
                let _ = reader.read_line(&mut line).unwrap();
                seen.push(line.clone());
                if line.contains("\"resource_invoke\"") {
                    assert!(
                        line.contains("\"uri\":\"elastos://did/*\""),
                        "unexpected resource URI: {line}"
                    );
                    assert!(
                        line.contains("\"operation\":\"get_did\""),
                        "unexpected carrier operation: {line}"
                    );
                    master
                        .write_all(
                            br#"{"id":2,"response":{"type":"resource_result","result":{"data":{"did":"did:key:zTest"}}}}"#,
                        )
                        .unwrap();
                    master.write_all(b"\n").unwrap();
                    master.flush().unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }

                seen
            });

            let old_path = std::env::var("ELASTOS_CARRIER_PATH").ok();
            std::env::set_var("ELASTOS_CARRIER_PATH", &slave_path);

            let result = {
                let mut client = RuntimeClient::new();
                let token = client
                    .request_capability("elastos://did/*", "execute")
                    .unwrap();
                assert_eq!(token, token_payload);

                client.resource_invoke("elastos://did/*", "get_did", &serde_json::json!({}), &token)
            };

            if let Some(value) = old_path {
                std::env::set_var("ELASTOS_CARRIER_PATH", value);
            } else {
                std::env::remove_var("ELASTOS_CARRIER_PATH");
            }

            let seen = bridge.join().unwrap();
            assert!(
                result.is_ok(),
                "resource_invoke failed: {:?}; bridge saw: {:?}",
                result,
                seen
            );

            let resp = result.unwrap();
            assert_eq!(
                resp.get("data")
                    .and_then(|d| d.get("did"))
                    .and_then(|v| v.as_str()),
                Some("did:key:zTest")
            );
        }
    }

    // ------------------------------------------------------------------
    // FIFO carrier-channel tests. These exercise the WASM Carrier transport
    // directly without needing a full runtime process.
    // ------------------------------------------------------------------

    #[cfg(all(feature = "serde", not(target_os = "wasi")))]
    fn fifo_test_dir(label: &str) -> std::path::PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "elastos-guest-fifo-test-{}-{}-{}",
            std::process::id(),
            label,
            nanos
        ));
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    #[cfg(all(feature = "serde", not(target_os = "wasi")))]
    fn mkfifo_for_test(path: &std::path::Path) {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let cstr = CString::new(path.as_os_str().as_bytes()).expect("path has no NUL");
        let ret = unsafe { libc::mkfifo(cstr.as_ptr(), 0o600) };
        assert_eq!(
            ret,
            0,
            "mkfifo({}) failed: {}",
            path.display(),
            std::io::Error::last_os_error()
        );
    }

    /// Opens both FIFOs RDWR on the host side to anchor them, so subsequent
    /// one-directional opens by `channel_from_fifos` don't block. Caller must
    /// keep the returned handles alive for the duration of the test.
    #[cfg(all(feature = "serde", not(target_os = "wasi")))]
    fn anchor_fifos(
        request_path: &std::path::Path,
        response_path: &std::path::Path,
    ) -> (std::fs::File, std::fs::File) {
        let req = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(request_path)
            .expect("anchor request");
        let resp = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(response_path)
            .expect("anchor response");
        (req, resp)
    }

    #[cfg(all(feature = "serde", not(target_os = "wasi")))]
    #[test]
    fn channel_from_fifos_rejects_missing_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("ELASTOS_CARRIER_FIFOS").ok();
        std::env::remove_var("ELASTOS_CARRIER_FIFOS");
        match RuntimeClient::channel_from_fifos() {
            Ok(_) => panic!("missing env must yield Err, not Ok"),
            Err(err) => assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput),
        }
        if let Some(v) = saved {
            std::env::set_var("ELASTOS_CARRIER_FIFOS", v);
        }
    }

    #[cfg(all(feature = "serde", not(target_os = "wasi")))]
    #[test]
    fn channel_from_fifos_rejects_malformed_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("ELASTOS_CARRIER_FIFOS").ok();

        for (label, value) in &[
            ("single-path", "/tmp/only-one"),
            ("three-paths", "/a,/b,/c"),
            ("empty-reader", ",/tmp/writer"),
            ("empty-writer", "/tmp/reader,"),
        ] {
            std::env::set_var("ELASTOS_CARRIER_FIFOS", value);
            match RuntimeClient::channel_from_fifos() {
                Ok(_) => panic!("{label}: malformed env must produce Err, got Ok"),
                Err(err) => assert_eq!(
                    err.kind(),
                    std::io::ErrorKind::InvalidInput,
                    "{label}: wrong error kind"
                ),
            }
        }

        if let Some(v) = saved {
            std::env::set_var("ELASTOS_CARRIER_FIFOS", v);
        } else {
            std::env::remove_var("ELASTOS_CARRIER_FIFOS");
        }
    }

    #[cfg(all(feature = "serde", not(target_os = "wasi")))]
    #[test]
    fn channel_from_fifos_round_trip() {
        use std::io::Read as _;

        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("ELASTOS_CARRIER_FIFOS").ok();

        let dir = fifo_test_dir("round-trip");
        let request_path = dir.join("request");
        let response_path = dir.join("response");
        mkfifo_for_test(&request_path);
        mkfifo_for_test(&response_path);

        // Anchor before opening one-directional. Keep alive until cleanup.
        let (mut host_req_rw, mut host_resp_rw) = anchor_fifos(&request_path, &response_path);

        // ELASTOS_CARRIER_FIFOS spec: "reader_path,writer_path" from the
        // capsule's perspective. reader = response (capsule reads SDK
        // responses); writer = request (capsule writes SDK requests).
        std::env::set_var(
            "ELASTOS_CARRIER_FIFOS",
            format!("{},{}", response_path.display(), request_path.display()),
        );

        let mut channel =
            RuntimeClient::channel_from_fifos().expect("channel_from_fifos must succeed");

        // Send a request through the channel and read it via the host's
        // RDWR anchor on the request FIFO. Tests that the writer side of
        // the channel is wired to the right FIFO.
        if let RuntimeBridgeChannel::FilePair { ref mut writer, .. } = channel {
            writeln!(writer, "{{\"id\":1,\"request\":{{\"type\":\"ping\"}}}}").unwrap();
            writer.flush().unwrap();
        } else {
            panic!("channel must be a FilePair");
        }

        let mut buf = String::new();
        let mut byte = [0u8; 1];
        loop {
            let n = host_req_rw.read(&mut byte).expect("host read");
            if n == 0 || byte[0] == b'\n' {
                if byte[0] == b'\n' {
                    buf.push('\n');
                }
                break;
            }
            buf.push(byte[0] as char);
        }
        assert_eq!(buf, "{\"id\":1,\"request\":{\"type\":\"ping\"}}\n");

        // Host writes a response; channel reader on the SDK side must see it.
        writeln!(
            host_resp_rw,
            "{{\"id\":1,\"response\":{{\"type\":\"pong\"}}}}"
        )
        .unwrap();
        host_resp_rw.flush().unwrap();

        if let RuntimeBridgeChannel::FilePair { ref mut reader, .. } = channel {
            let mut resp_buf = String::new();
            reader
                .read_line(&mut resp_buf)
                .expect("channel reader must read line");
            assert_eq!(resp_buf, "{\"id\":1,\"response\":{\"type\":\"pong\"}}\n");
        }

        // Drop the channel and the anchors before unlinking.
        drop(channel);
        drop(host_req_rw);
        drop(host_resp_rw);

        if let Some(v) = saved {
            std::env::set_var("ELASTOS_CARRIER_FIFOS", v);
        } else {
            std::env::remove_var("ELASTOS_CARRIER_FIFOS");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
