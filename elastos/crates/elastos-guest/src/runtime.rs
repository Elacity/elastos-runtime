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

/// Request from capsule to runtime
#[cfg(feature = "serde")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeRequest {
    /// List running capsules (shell only)
    ListCapsules,

    /// Launch a capsule (shell only)
    LaunchCapsule {
        cid: String,
        #[serde(default)]
        config: LaunchConfig,
    },

    /// Stop a capsule (shell only)
    StopCapsule { capsule_id: String },

    /// Grant capability to a capsule (shell only)
    GrantCapability {
        capsule_id: String,
        resource: String,
        action: String,
        #[serde(default)]
        constraints: CapabilityConstraints,
    },

    /// Revoke a capability (shell only)
    RevokeCapability { token_id: String },

    /// Send message to another capsule
    SendMessage {
        to: String,
        payload: Vec<u8>,
        #[serde(default)]
        reply_to: Option<String>,
    },

    /// Receive pending messages
    ReceiveMessages,

    /// Fetch content by elastos:// URI
    FetchContent {
        uri: String,
        #[serde(default)]
        token: Option<String>,
    },

    /// Request storage access (invokes capability)
    StorageRead { token: String, path: String },

    /// Request storage write (invokes capability)
    StorageWrite {
        token: String,
        path: String,
        content: Vec<u8>,
    },

    /// Request a capability token (capsule→shell, waits for approval)
    RequestCapability { resource: String, action: String },

    /// Call a provider operation (capsule→provider, via runtime routing)
    ProviderCall {
        scheme: String,
        op: String,
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

/// Response from runtime to capsule
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

    /// List of capsules
    CapsuleList { capsules: Vec<CapsuleListEntry> },

    /// Capsule launched
    CapsuleLaunched { capsule_id: String },

    /// Capability granted (shell granting to another capsule)
    CapabilityGranted { token_id: String },

    /// Capability token received (capsule requested, shell approved)
    CapabilityToken { token: String },

    /// Provider call result
    ProviderResult { result: serde_json::Value },

    /// Messages received
    Messages { messages: Vec<IncomingMessage> },

    /// Content fetched
    Content { data: Vec<u8> },

    /// Storage read result
    StorageData { data: Vec<u8> },

    /// Runtime info
    RuntimeInfo {
        version: String,
        capsule_count: usize,
    },

    /// Pong response
    Pong,
}

/// Configuration for launching a capsule
#[cfg(feature = "serde")]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LaunchConfig {
    #[serde(default)]
    pub env: Vec<(String, String)>,
    #[serde(default)]
    pub args: Vec<String>,
}

/// Constraints for capability grants
#[cfg(feature = "serde")]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilityConstraints {
    #[serde(default)]
    pub max_uses: Option<u32>,
    #[serde(default)]
    pub expiry_secs: Option<u64>,
    #[serde(default)]
    pub delegatable: bool,
}

/// Entry in capsule list
#[cfg(feature = "serde")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapsuleListEntry {
    pub id: String,
    pub name: String,
    pub status: String,
}

/// Incoming message from another capsule
#[cfg(feature = "serde")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessage {
    pub id: String,
    pub from: String,
    pub payload: Vec<u8>,
    pub timestamp: u64,
    #[serde(default)]
    pub reply_to: Option<String>,
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
enum CarrierChannel {
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
    /// WASM capsules running locally use this to reach the runtime's Carrier.
    Http { api_url: String, token: String },
}

/// Runtime client for capsules.
///
/// Communicates with the ElastOS runtime via Carrier. The transport is
/// detected automatically:
/// - `ELASTOS_CARRIER_FDS` set (e.g., "3,4") → use those fds (WASM bridge mode)
/// - `ELASTOS_CARRIER_PATH` set → use that file (e.g., /dev/hvc0 for microVM virtio-console)
/// - Otherwise → use stdin/stdout (standalone host-process mode, no bridge)
///
/// Capsule code doesn't change between substrates. Just use `RuntimeClient::new()`.
#[cfg(feature = "serde")]
pub struct RuntimeClient {
    next_id: RequestId,
    channel: CarrierChannel,
}

#[cfg(feature = "serde")]
impl RuntimeClient {
    /// Create a new runtime client.
    ///
    /// Detects the Carrier channel automatically:
    /// 1. `ELASTOS_CARRIER_FDS=read_fd,write_fd` → dedicated fd pair (WASM bridge, in-process)
    /// 2. `ELASTOS_CARRIER_PATH=/dev/hvc0` → file-based (microVM virtio-console device)
    /// 3. `ELASTOS_API` + `ELASTOS_TOKEN` → HTTP API to running runtime (attached mode)
    /// 4. Otherwise → stdin/stdout (standalone host-process mode)
    pub fn new() -> Self {
        let channel = if std::env::var_os("ELASTOS_CARRIER_FDS").is_some() {
            Self::channel_from_fds()
                .unwrap_or_else(|e| panic!("ELASTOS_CARRIER_FDS is set but invalid: {e}"))
        } else if let Ok(path) = std::env::var("ELASTOS_CARRIER_PATH") {
            // MicroVM: use one kept-open serial fd for Carrier. Avoid BufReader
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
                        CarrierChannel::Serial { file }
                    }
                    #[cfg(target_os = "wasi")]
                    {
                        let _ = file;
                        CarrierChannel::Stdio
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
                CarrierChannel::Http { api_url, token }
            } else {
                CarrierChannel::Stdio
            }
        } else {
            CarrierChannel::Stdio
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
    /// Maps RuntimeRequest variants to POST /api/provider/:scheme/:op calls.
    #[cfg(feature = "serde")]
    fn http_call(
        _id: RequestId,
        request: &RuntimeRequest,
        api_url: &str,
        token: &str,
    ) -> io::Result<RuntimeResponse> {
        let (path, body, cap_token) = match request {
            RuntimeRequest::RequestCapability { resource, action } => (
                "/api/capability/request".to_string(),
                serde_json::json!({"resource": resource, "action": action}),
                None,
            ),
            RuntimeRequest::ProviderCall {
                scheme,
                op,
                body,
                token: cap_token,
            } => (
                format!("/api/provider/{}/{}", scheme, op),
                body.clone(),
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
            _ => {
                return Ok(RuntimeResponse::Error {
                    code: "not_supported".to_string(),
                    message: "operation not supported in attached-runtime mode".to_string(),
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
            RuntimeRequest::ProviderCall { .. } => {
                Ok(RuntimeResponse::ProviderResult { result: resp_json })
            }
            _ => Ok(RuntimeResponse::Ok {
                data: Some(resp_json),
            }),
        }
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

    /// Try to open a Carrier channel from ELASTOS_CARRIER_FDS env var.
    ///
    /// Format: "read_fd,write_fd" (e.g., "3,4").
    /// Used by the WASM bridge: the runtime inserts pipe endpoints at these fds
    /// in the WASI context, keeping stdin/stdout free for user I/O.
    fn channel_from_fds() -> io::Result<CarrierChannel> {
        let fds_str = std::env::var("ELASTOS_CARRIER_FDS")
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let parts: Vec<&str> = fds_str.split(',').collect();
        if parts.len() != 2 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected ELASTOS_CARRIER_FDS=read_fd,write_fd",
            ));
        }
        let read_fd: i32 = parts[0].trim().parse().map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidInput, format!("invalid read fd: {e}"))
        })?;
        let write_fd: i32 = parts[1].trim().parse().map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid write fd: {e}"),
            )
        })?;

        // Safety: the runtime guarantees these fds are valid pipe endpoints
        // inserted into the WASI context before the capsule starts.
        #[cfg(target_os = "wasi")]
        {
            use std::os::wasi::io::FromRawFd;
            let reader = unsafe { std::fs::File::from_raw_fd(read_fd) };
            let writer = unsafe { std::fs::File::from_raw_fd(write_fd) };
            Ok(CarrierChannel::FilePair {
                reader: io::BufReader::new(reader),
                writer,
            })
        }

        #[cfg(not(target_os = "wasi"))]
        {
            use std::os::unix::io::FromRawFd;
            let reader = unsafe { std::fs::File::from_raw_fd(read_fd) };
            let writer = unsafe { std::fs::File::from_raw_fd(write_fd) };
            Ok(CarrierChannel::FilePair {
                reader: io::BufReader::new(reader),
                writer,
            })
        }
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
                            "carrier channel closed",
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
            CarrierChannel::Stdio => {
                let mut stdout = io::stdout().lock();
                writeln!(stdout, "{}", json)?;
                stdout.flush()?;

                let stdin = io::stdin();
                let mut line = String::new();
                stdin.lock().read_line(&mut line)?;
                line
            }
            #[cfg(not(target_os = "wasi"))]
            CarrierChannel::Serial { file } => {
                Self::serial_write_line(file, &json)?;
                Self::read_unbuffered_line(file)?
            }
            CarrierChannel::FilePair { reader, writer } => {
                writeln!(writer, "{}", json)?;
                writer.flush()?;

                let mut line = String::new();
                reader.read_line(&mut line)?;
                line
            }
            CarrierChannel::Http { api_url, token } => {
                // Translate SDK request into HTTP API call to the running runtime.
                return Self::http_call(id, &envelope.request, api_url, token);
            }
        };

        let resp_envelope: ResponseEnvelope = serde_json::from_str(&line)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

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
        if !matches!(self.channel, CarrierChannel::Stdio) {
            return self.call(request);
        }

        let id = self.next_id;
        self.next_id += 1;

        let envelope = RequestEnvelope { id, request };
        let json = serde_json::to_string(&envelope)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        // Send request via the detected channel
        match &mut self.channel {
            CarrierChannel::Stdio => {
                let mut stdout = io::stdout().lock();
                writeln!(stdout, "{}", json)?;
                stdout.flush()?;
            }
            #[cfg(not(target_os = "wasi"))]
            CarrierChannel::Serial { .. }
            | CarrierChannel::FilePair { .. }
            | CarrierChannel::Http { .. } => unreachable!(),
            #[cfg(target_os = "wasi")]
            CarrierChannel::FilePair { .. } | CarrierChannel::Http { .. } => unreachable!(),
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

        let resp_envelope: ResponseEnvelope = serde_json::from_str(&line)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

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

    /// List running capsules
    pub fn list_capsules(&mut self) -> io::Result<Vec<CapsuleListEntry>> {
        match self.call(RuntimeRequest::ListCapsules)? {
            RuntimeResponse::CapsuleList { capsules } => Ok(capsules),
            RuntimeResponse::Error { code, message } => {
                Err(io::Error::other(format!("{}: {}", code, message)))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response",
            )),
        }
    }

    /// Launch a capsule
    pub fn launch_capsule(&mut self, cid: &str, config: LaunchConfig) -> io::Result<String> {
        match self.call(RuntimeRequest::LaunchCapsule {
            cid: cid.to_string(),
            config,
        })? {
            RuntimeResponse::CapsuleLaunched { capsule_id } => Ok(capsule_id),
            RuntimeResponse::Error { code, message } => {
                Err(io::Error::other(format!("{}: {}", code, message)))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response",
            )),
        }
    }

    /// Stop a capsule
    pub fn stop_capsule(&mut self, capsule_id: &str) -> io::Result<()> {
        match self.call(RuntimeRequest::StopCapsule {
            capsule_id: capsule_id.to_string(),
        })? {
            RuntimeResponse::Ok { .. } => Ok(()),
            RuntimeResponse::Error { code, message } => {
                Err(io::Error::other(format!("{}: {}", code, message)))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response",
            )),
        }
    }

    /// Grant capability to a capsule
    pub fn grant_capability(
        &mut self,
        capsule_id: &str,
        resource: &str,
        action: &str,
        constraints: CapabilityConstraints,
    ) -> io::Result<String> {
        match self.call(RuntimeRequest::GrantCapability {
            capsule_id: capsule_id.to_string(),
            resource: resource.to_string(),
            action: action.to_string(),
            constraints,
        })? {
            RuntimeResponse::CapabilityGranted { token_id } => Ok(token_id),
            RuntimeResponse::Error { code, message } => {
                Err(io::Error::other(format!("{}: {}", code, message)))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response",
            )),
        }
    }

    /// Revoke a capability
    pub fn revoke_capability(&mut self, token_id: &str) -> io::Result<()> {
        match self.call(RuntimeRequest::RevokeCapability {
            token_id: token_id.to_string(),
        })? {
            RuntimeResponse::Ok { .. } => Ok(()),
            RuntimeResponse::Error { code, message } => {
                Err(io::Error::other(format!("{}: {}", code, message)))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response",
            )),
        }
    }

    /// Send message to another capsule
    pub fn send_message(&mut self, to: &str, payload: Vec<u8>) -> io::Result<()> {
        match self.call(RuntimeRequest::SendMessage {
            to: to.to_string(),
            payload,
            reply_to: None,
        })? {
            RuntimeResponse::Ok { .. } => Ok(()),
            RuntimeResponse::Error { code, message } => {
                Err(io::Error::other(format!("{}: {}", code, message)))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response",
            )),
        }
    }

    /// Receive pending messages
    pub fn receive_messages(&mut self) -> io::Result<Vec<IncomingMessage>> {
        match self.call(RuntimeRequest::ReceiveMessages)? {
            RuntimeResponse::Messages { messages } => Ok(messages),
            RuntimeResponse::Error { code, message } => {
                Err(io::Error::other(format!("{}: {}", code, message)))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response",
            )),
        }
    }

    /// Fetch content by URI
    pub fn fetch_content(&mut self, uri: &str) -> io::Result<Vec<u8>> {
        match self.call(RuntimeRequest::FetchContent {
            uri: uri.to_string(),
            token: None,
        })? {
            RuntimeResponse::Content { data } => Ok(data),
            RuntimeResponse::Error { code, message } => {
                Err(io::Error::other(format!("{}: {}", code, message)))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response",
            )),
        }
    }

    /// Fetch content by URI with an explicit read capability token.
    pub fn fetch_content_with_token(&mut self, uri: &str, token: &str) -> io::Result<Vec<u8>> {
        match self.call(RuntimeRequest::FetchContent {
            uri: uri.to_string(),
            token: Some(token.to_string()),
        })? {
            RuntimeResponse::Content { data } => Ok(data),
            RuntimeResponse::Error { code, message } => {
                Err(io::Error::other(format!("{}: {}", code, message)))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response",
            )),
        }
    }

    /// Read from storage
    pub fn storage_read(&mut self, token: &str, path: &str) -> io::Result<Vec<u8>> {
        match self.call(RuntimeRequest::StorageRead {
            token: token.to_string(),
            path: path.to_string(),
        })? {
            RuntimeResponse::StorageData { data } => Ok(data),
            RuntimeResponse::Error { code, message } => {
                Err(io::Error::other(format!("{}: {}", code, message)))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response",
            )),
        }
    }

    /// Write to storage
    pub fn storage_write(&mut self, token: &str, path: &str, content: Vec<u8>) -> io::Result<()> {
        match self.call(RuntimeRequest::StorageWrite {
            token: token.to_string(),
            path: path.to_string(),
            content,
        })? {
            RuntimeResponse::Ok { .. } => Ok(()),
            RuntimeResponse::Error { code, message } => {
                Err(io::Error::other(format!("{}: {}", code, message)))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response",
            )),
        }
    }

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

    /// Call a provider operation via the runtime.
    /// The runtime routes the call to the appropriate provider (e.g., peer, did, storage).
    pub fn provider_call(
        &mut self,
        scheme: &str,
        op: &str,
        body: &serde_json::Value,
        token: &str,
    ) -> io::Result<serde_json::Value> {
        match self.call(RuntimeRequest::ProviderCall {
            scheme: scheme.to_string(),
            op: op.to_string(),
            body: body.clone(),
            token: token.to_string(),
        })? {
            RuntimeResponse::ProviderResult { result } => Ok(result),
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

    #[cfg(feature = "serde")]
    #[test]
    fn test_request_serialization() {
        let req = RuntimeRequest::ListCapsules;
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("list_capsules"));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_response_serialization() {
        let resp = RuntimeResponse::CapsuleList {
            capsules: vec![CapsuleListEntry {
                id: "cap-1".to_string(),
                name: "test".to_string(),
                status: "running".to_string(),
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("capsule_list"));
        assert!(json.contains("cap-1"));
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
    fn test_runtime_client_two_calls_over_tty_serial() {
        let _guard = ENV_LOCK.lock().unwrap();
        let token_payload = "tok-1".repeat(256);

        unsafe {
            let mut master_fd = -1;
            let mut slave_fd = -1;
            let mut name = [0i8; 128];

            let rc = libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                name.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
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

                // Request 2: provider_call(get_did)
                let _ = reader.read_line(&mut line).unwrap();
                seen.push(line.clone());
                if line.contains("\"provider_call\"") {
                    assert!(
                        line.contains("\"scheme\":\"did\""),
                        "unexpected provider scheme: {line}"
                    );
                    assert!(
                        line.contains("\"op\":\"get_did\""),
                        "unexpected provider op: {line}"
                    );
                    master
                        .write_all(
                            br#"{"id":2,"response":{"type":"provider_result","result":{"data":{"did":"did:key:zTest"}}}}"#,
                        )
                        .unwrap();
                    master.write_all(b"\n").unwrap();
                    master.flush().unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }

                seen
            });

            let old_fds = std::env::var("ELASTOS_CARRIER_FDS").ok();
            let old_path = std::env::var("ELASTOS_CARRIER_PATH").ok();
            std::env::remove_var("ELASTOS_CARRIER_FDS");
            std::env::set_var("ELASTOS_CARRIER_PATH", &slave_path);

            let result = {
                let mut client = RuntimeClient::new();
                let token = client
                    .request_capability("elastos://did/*", "execute")
                    .unwrap();
                assert_eq!(token, token_payload);

                client.provider_call("did", "get_did", &serde_json::json!({}), &token)
            };

            if let Some(value) = old_fds {
                std::env::set_var("ELASTOS_CARRIER_FDS", value);
            } else {
                std::env::remove_var("ELASTOS_CARRIER_FDS");
            }
            if let Some(value) = old_path {
                std::env::set_var("ELASTOS_CARRIER_PATH", value);
            } else {
                std::env::remove_var("ELASTOS_CARRIER_PATH");
            }

            let seen = bridge.join().unwrap();
            assert!(
                result.is_ok(),
                "provider_call failed: {:?}; bridge saw: {:?}",
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
}
