//! VM-backed provider bridge for supervisor-launched capsule providers.
//!
//! This adapter implements the runtime `Provider` trait and forwards raw JSON
//! requests over the Carrier-managed guest control network. Capsules are
//! expected to expose line-delimited JSON on the configured port.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use elastos_runtime::provider::{
    EntryType, Provider, ProviderError, ResourceAction, ResourceEntry, ResourceRequest,
    ResourceResponse,
};

struct VmIo {
    reader: BufReader<Box<dyn Read + Send>>,
    writer: Box<dyn Write + Send>,
    raw_fd: i32, // For poll() — the underlying socket fd
}

const VM_PROVIDER_READ_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(test)]
const VM_PROVIDER_CONNECT_ATTEMPTS: usize = 3;
#[cfg(not(test))]
const VM_PROVIDER_CONNECT_ATTEMPTS: usize = 150;

struct VmRawBridge {
    guest_host: String,
    guest_port: u16,
    init_config: serde_json::Value,
    io: Mutex<Option<VmIo>>,
}

impl VmRawBridge {
    fn new(guest_host: String, guest_port: u16, init_config: serde_json::Value) -> Self {
        Self {
            guest_host,
            guest_port,
            init_config,
            io: Mutex::new(None),
        }
    }

    fn connect(&self) -> Result<VmIo, ProviderError> {
        let started = std::time::Instant::now();
        let mut last_err = None;
        for attempt in 0..VM_PROVIDER_CONNECT_ATTEMPTS {
            match self.try_connect_once() {
                Ok(io) => {
                    tracing::info!(
                        "tcp connect to guest {}:{} succeeded on attempt {} ({:.1}s)",
                        self.guest_host,
                        self.guest_port,
                        attempt + 1,
                        started.elapsed().as_secs_f64()
                    );
                    return Ok(io);
                }
                Err(err) => {
                    last_err = Some(err);
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }

        let msg = last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unknown error".to_string());
        Err(ProviderError::Provider(format!(
            "tcp connect to guest {}:{} failed after {:.1}s: {}",
            self.guest_host,
            self.guest_port,
            started.elapsed().as_secs_f64(),
            msg
        )))
    }

    fn try_connect_once(&self) -> Result<VmIo, ProviderError> {
        // Connect via vsock (guest_host is the vsock CID as a string)
        if let Ok(cid) = self.guest_host.parse::<u32>() {
            return self.try_vsock_connect(cid, self.guest_port as u32);
        }

        // Explicit local TCP compatibility path for host-native providers and
        // local tests. This must stay local-only; arbitrary remote TCP targets
        // would silently widen the trusted provider bridge.
        self.validate_local_tcp_compatibility_host()?;
        let addr = (self.guest_host.as_str(), self.guest_port)
            .to_socket_addrs()
            .map_err(|e| {
                ProviderError::Provider(format!("resolve guest provider address failed: {e}"))
            })?
            .next()
            .ok_or_else(|| {
                ProviderError::Provider("guest provider address resolved empty".into())
            })?;

        tracing::info!(
            "using local TCP compatibility transport to guest {}:{}",
            self.guest_host,
            self.guest_port
        );

        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
            .map_err(|e| ProviderError::Provider(format!("tcp connect attempt failed: {}", e)))?;
        stream
            .set_read_timeout(Some(VM_PROVIDER_READ_TIMEOUT))
            .map_err(|e| ProviderError::Provider(format!("tcp read timeout setup failed: {e}")))?;
        stream
            .set_write_timeout(Some(VM_PROVIDER_READ_TIMEOUT))
            .map_err(|e| ProviderError::Provider(format!("tcp write timeout setup failed: {e}")))?;

        let raw_fd = stream.as_raw_fd();
        let writer = stream
            .try_clone()
            .map_err(|e| ProviderError::Provider(format!("tcp clone failed: {e}")))?;
        let reader: BufReader<Box<dyn Read + Send>> = BufReader::new(Box::new(stream));
        let writer: Box<dyn Write + Send> = Box::new(writer);

        Ok(VmIo {
            reader,
            writer,
            raw_fd,
        })
    }

    fn validate_local_tcp_compatibility_host(&self) -> Result<(), ProviderError> {
        if self.guest_host.eq_ignore_ascii_case("localhost") {
            return Ok(());
        }

        let ip: std::net::IpAddr = self.guest_host.parse().map_err(|_| {
            ProviderError::Provider(format!(
                "tcp compatibility transport requires localhost or a local/private IP literal, got '{}'",
                self.guest_host
            ))
        })?;

        let allowed = match ip {
            std::net::IpAddr::V4(ipv4) => {
                ipv4.is_loopback() || ipv4.is_private() || ipv4.is_link_local()
            }
            std::net::IpAddr::V6(ipv6) => {
                ipv6.is_loopback() || ipv6.is_unique_local() || ipv6.is_unicast_link_local()
            }
        };

        if !allowed {
            return Err(ProviderError::Provider(format!(
                "tcp compatibility transport requires a local/private address, got '{}'",
                self.guest_host
            )));
        }

        Ok(())
    }

    fn try_vsock_connect(&self, cid: u32, port: u32) -> Result<VmIo, ProviderError> {
        use std::os::unix::io::FromRawFd;

        const AF_VSOCK: i32 = 40;
        const SOCK_STREAM: i32 = 1;

        #[repr(C)]
        struct SockaddrVm {
            svm_family: u16,
            svm_reserved1: u16,
            svm_port: u32,
            svm_cid: u32,
            svm_zero: [u8; 4],
        }

        unsafe {
            let fd = libc::socket(AF_VSOCK, SOCK_STREAM, 0);
            if fd < 0 {
                return Err(ProviderError::Provider(format!(
                    "vsock socket() failed: {}",
                    std::io::Error::last_os_error()
                )));
            }

            let addr = SockaddrVm {
                svm_family: AF_VSOCK as u16,
                svm_reserved1: 0,
                svm_port: port,
                svm_cid: cid,
                svm_zero: [0; 4],
            };

            let result = libc::connect(
                fd,
                &addr as *const SockaddrVm as *const libc::sockaddr,
                std::mem::size_of::<SockaddrVm>() as u32,
            );

            if result < 0 {
                let err = std::io::Error::last_os_error();
                libc::close(fd);
                return Err(ProviderError::Provider(format!(
                    "vsock connect to CID {}:{} failed: {}",
                    cid, port, err
                )));
            }

            let stream = std::fs::File::from_raw_fd(fd);
            let raw_fd = fd;
            let writer = stream
                .try_clone()
                .map_err(|e| ProviderError::Provider(format!("vsock clone failed: {e}")))?;
            let reader: BufReader<Box<dyn Read + Send>> = BufReader::new(Box::new(stream));
            let writer: Box<dyn Write + Send> = Box::new(writer);

            Ok(VmIo {
                reader,
                writer,
                raw_fd,
            })
        }
    }

    fn send_raw_blocking(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let mut guard = self
            .io
            .lock()
            .map_err(|_| ProviderError::Provider("vm bridge mutex poisoned".into()))?;

        if guard.is_some() {
            tracing::info!(
                "reusing persistent connection to guest {}:{} for: {}",
                self.guest_host,
                self.guest_port,
                serde_json::to_string(request).unwrap_or_default()
            );
        }

        if guard.is_none() {
            *guard = Some(self.connect()?);
            let io = guard
                .as_mut()
                .ok_or_else(|| ProviderError::Provider("vm bridge unavailable".into()))?;
            let init_req = serde_json::json!({
                "op": "init",
                "config": self.init_config.clone()
            });
            tracing::info!(
                "sending init to guest {}:{}: {}",
                self.guest_host,
                self.guest_port,
                serde_json::to_string(&init_req).unwrap_or_default()
            );
            let init_start = std::time::Instant::now();
            let init_resp = match Self::send_line_and_read_json(io, &init_req) {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::warn!(
                        "init exchange failed for guest {}:{} after {:.1}s: {}",
                        self.guest_host,
                        self.guest_port,
                        init_start.elapsed().as_secs_f64(),
                        e
                    );
                    *guard = None;
                    return Err(ProviderError::Provider(format!(
                        "provider VM init exchange failed: {e}"
                    )));
                }
            };
            tracing::info!(
                "init response from guest {}:{} in {:.1}s: {}",
                self.guest_host,
                self.guest_port,
                init_start.elapsed().as_secs_f64(),
                init_resp
            );
            let init_ok = init_resp
                .get("status")
                .and_then(|v| v.as_str())
                .map(|s| s == "ok")
                .unwrap_or(false);
            if !init_ok {
                *guard = None;
                return Err(ProviderError::Provider(format!(
                    "provider VM init failed: {}",
                    init_resp
                )));
            }
        }
        let io = guard
            .as_mut()
            .ok_or_else(|| ProviderError::Provider("vm bridge unavailable".into()))?;
        match Self::send_line_and_read_json(io, request) {
            Ok(v) => Ok(v),
            Err(e) => {
                *guard = None;
                Err(e)
            }
        }
    }

    fn send_line_and_read_json(
        io: &mut VmIo,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let payload = serde_json::to_string(request)
            .map_err(|e| ProviderError::Provider(format!("serialize request failed: {e}")))?;
        io.writer
            .write_all(payload.as_bytes())
            .map_err(|e| ProviderError::Provider(format!("tcp write failed: {e}")))?;
        io.writer
            .write_all(b"\n")
            .map_err(|e| ProviderError::Provider(format!("tcp newline write failed: {e}")))?;
        io.writer
            .flush()
            .map_err(|e| ProviderError::Provider(format!("tcp flush failed: {e}")))?;

        tracing::debug!(
            "tcp write complete ({} bytes), waiting for response...",
            payload.len() + 1
        );

        const MAX_LINE_LEN: usize = 1_048_576; // 1 MB
        let mut raw = Vec::new();
        for _ in 0..256 {
            Self::wait_for_readable(io, VM_PROVIDER_READ_TIMEOUT)?;
            raw.clear();
            // Bounded read: accumulate raw bytes until newline or EOF,
            // enforcing the size limit before allocating. UTF-8 decoding
            // happens only after the complete line is framed, avoiding
            // false failures from multibyte codepoints split across chunks.
            loop {
                let buf = io
                    .reader
                    .fill_buf()
                    .map_err(|e| ProviderError::Provider(format!("tcp read failed: {e}")))?;
                if buf.is_empty() {
                    if raw.is_empty() {
                        return Err(ProviderError::Provider(
                            "provider VM closed tcp connection".into(),
                        ));
                    }
                    break; // EOF mid-line — process what we have
                }
                let (chunk, found_nl) = match buf.iter().position(|&b| b == b'\n') {
                    Some(pos) => (&buf[..=pos], true),
                    None => (buf, false),
                };
                let chunk_len = chunk.len();
                if raw.len() + chunk_len > MAX_LINE_LEN {
                    return Err(ProviderError::Provider(format!(
                        "provider response line exceeds {} bytes",
                        MAX_LINE_LEN
                    )));
                }
                raw.extend_from_slice(chunk);
                io.reader.consume(chunk_len);
                if found_nl {
                    break;
                }
            }
            let line = std::str::from_utf8(&raw).map_err(|_| {
                ProviderError::Provider("provider response contains invalid UTF-8".into())
            })?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
                return Ok(v);
            }
        }

        Err(ProviderError::Provider(
            "did not receive JSON response from provider VM".into(),
        ))
    }

    fn wait_for_readable(io: &VmIo, timeout: Duration) -> Result<(), ProviderError> {
        if !io.reader.buffer().is_empty() {
            tracing::trace!("provider reader has buffered data, skipping poll");
            return Ok(());
        }

        let fd = io.raw_fd;
        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;

        let rc = unsafe { libc::poll(&mut pollfd as *mut libc::pollfd, 1, timeout_ms) };
        if rc < 0 {
            return Err(ProviderError::Provider(format!(
                "provider poll failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        if rc == 0 {
            tracing::warn!("provider poll timed out after {}ms (fd={})", timeout_ms, fd);
            return Err(ProviderError::Provider(format!(
                "timed out waiting for provider VM response after {:?}",
                timeout
            )));
        }
        if (pollfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL)) != 0 {
            tracing::warn!(
                "provider poll unhealthy: rc={}, revents=0x{:x} (fd={})",
                rc,
                pollfd.revents,
                fd
            );
            return Err(ProviderError::Provider(format!(
                "provider VM socket became unhealthy (revents=0x{:x})",
                pollfd.revents
            )));
        }
        tracing::trace!(
            "provider poll ready: rc={}, revents=0x{:x}",
            rc,
            pollfd.revents
        );
        Ok(())
    }
}

/// Provider adapter for a supervisor-launched capsule VM.
pub struct VmCapsuleProvider {
    scheme: &'static str,
    bridge: Arc<VmRawBridge>,
}

impl VmCapsuleProvider {
    pub fn new(
        scheme: impl Into<String>,
        guest_host: String,
        guest_port: u16,
        init_config: serde_json::Value,
    ) -> Self {
        let scheme = scheme.into().to_ascii_lowercase();
        let scheme: &'static str = Box::leak(scheme.into_boxed_str());
        Self {
            scheme,
            bridge: Arc::new(VmRawBridge::new(guest_host, guest_port, init_config)),
        }
    }

    fn to_raw_request(request: &ResourceRequest) -> serde_json::Value {
        match request.action {
            ResourceAction::Read => serde_json::json!({
                "op": "read",
                "path": request.path,
                "token": "",
            }),
            ResourceAction::Write => serde_json::json!({
                "op": "write",
                "path": request.path,
                "token": "",
                "content": request.content.clone().unwrap_or_default(),
                "append": false,
            }),
            ResourceAction::Delete => serde_json::json!({
                "op": "delete",
                "path": request.path,
                "token": "",
                "recursive": request.recursive,
            }),
            ResourceAction::List => serde_json::json!({
                "op": "list",
                "path": request.path,
                "token": "",
            }),
            ResourceAction::Stat => serde_json::json!({
                "op": "stat",
                "path": request.path,
                "token": "",
            }),
            ResourceAction::Mkdir => serde_json::json!({
                "op": "mkdir",
                "path": request.path,
                "token": "",
                "parents": true,
            }),
            ResourceAction::Exists => serde_json::json!({
                "op": "exists",
                "path": request.path,
                "token": "",
            }),
        }
    }

    fn map_error_response(response: &serde_json::Value) -> ProviderError {
        let code = response
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("provider_error");
        let message = response
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown provider error");

        // Classify by code field only. Message content is not trusted for
        // error type classification — a VM could spoof error types via crafted
        // messages. Providers should use structured code fields.
        match code {
            "not_found" => ProviderError::NotFound(message.to_string()),
            "permission_denied" | "path_not_allowed" => {
                ProviderError::PermissionDenied(message.to_string())
            }
            _ => ProviderError::Provider(format!("[{}] {}", code, message)),
        }
    }

    fn to_resource_response(
        action: ResourceAction,
        response: serde_json::Value,
    ) -> Result<ResourceResponse, ProviderError> {
        let status = response
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("error");
        if status != "ok" {
            return Err(Self::map_error_response(&response));
        }

        let data = response.get("data").cloned();
        match action {
            ResourceAction::Read => {
                let data = data
                    .ok_or_else(|| ProviderError::Provider("read response missing data".into()))?;
                let content = data
                    .get("content")
                    .ok_or_else(|| {
                        ProviderError::Provider("read response missing 'content'".into())
                    })?
                    .as_array()
                    .ok_or_else(|| ProviderError::Provider("'content' is not an array".into()))?
                    .iter()
                    .map(|v| {
                        v.as_u64()
                            .filter(|&n| n <= 255)
                            .map(|n| n as u8)
                            .ok_or_else(|| {
                                ProviderError::Provider(
                                    "read response contains non-byte value in content array".into(),
                                )
                            })
                    })
                    .collect::<Result<Vec<u8>, _>>()?;
                Ok(ResourceResponse::Data(content))
            }
            ResourceAction::Write => {
                let bytes = data
                    .and_then(|d| d.get("bytes_written").and_then(|v| v.as_u64()))
                    .unwrap_or(0) as usize;
                Ok(ResourceResponse::Written { bytes })
            }
            ResourceAction::Delete => Ok(ResourceResponse::Deleted),
            ResourceAction::List => {
                let data = data
                    .ok_or_else(|| ProviderError::Provider("list response missing data".into()))?;
                let entries: Vec<serde_json::Value> = serde_json::from_value(data)
                    .map_err(|e| ProviderError::Provider(format!("parse list: {}", e)))?;
                let resource_entries = entries
                    .iter()
                    .map(|e| ResourceEntry {
                        name: e
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        is_directory: e.get("is_dir").and_then(|v| v.as_bool()).unwrap_or(false),
                        size: e.get("size").and_then(|v| v.as_u64()),
                        modified: None,
                    })
                    .collect();
                Ok(ResourceResponse::List(resource_entries))
            }
            ResourceAction::Stat => {
                let data = data
                    .ok_or_else(|| ProviderError::Provider("stat response missing data".into()))?;
                let is_dir = data
                    .get("is_dir")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let size = data.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                let modified = data.get("modified").and_then(|v| v.as_u64()).unwrap_or(0);
                let entry_type = if is_dir {
                    EntryType::Directory
                } else {
                    EntryType::File
                };
                Ok(ResourceResponse::Metadata {
                    size,
                    entry_type,
                    modified,
                })
            }
            ResourceAction::Mkdir => Ok(ResourceResponse::Created),
            ResourceAction::Exists => {
                let exists = data
                    .and_then(|d| d.get("exists").and_then(|v| v.as_bool()))
                    .unwrap_or(false);
                Ok(ResourceResponse::Exists(exists))
            }
        }
    }
}

#[async_trait::async_trait]
impl Provider for VmCapsuleProvider {
    async fn handle(&self, request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        let action = request.action;
        let raw_req = Self::to_raw_request(&request);
        let raw_resp = self.send_raw(&raw_req).await?;
        Self::to_resource_response(action, raw_resp)
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec![self.scheme]
    }

    fn name(&self) -> &'static str {
        "vm-capsule-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let bridge = Arc::clone(&self.bridge);
        let request = request.clone();
        tokio::task::spawn_blocking(move || bridge.send_raw_blocking(&request))
            .await
            .map_err(|e| ProviderError::Provider(format!("vm bridge task join failed: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_raw_request_read() {
        let req = ResourceRequest {
            uri: "localhost://Users/self/Documents/a.txt".into(),
            _scheme: "localhost".into(),
            path: "Users/self/Documents/a.txt".into(),
            _capsule_id: "capsule-1".into(),
            action: ResourceAction::Read,
            content: None,
            recursive: false,
        };
        let raw = VmCapsuleProvider::to_raw_request(&req);
        assert_eq!(raw.get("op").and_then(|v| v.as_str()), Some("read"));
        assert_eq!(
            raw.get("path").and_then(|v| v.as_str()),
            Some("Users/self/Documents/a.txt")
        );
    }

    #[test]
    fn test_to_resource_response_read_ok() {
        let response = serde_json::json!({
            "status": "ok",
            "data": { "content": [1, 2, 3] }
        });
        let mapped =
            VmCapsuleProvider::to_resource_response(ResourceAction::Read, response).unwrap();
        match mapped {
            ResourceResponse::Data(bytes) => assert_eq!(bytes, vec![1, 2, 3]),
            _ => panic!("expected data response"),
        }
    }

    #[test]
    fn test_to_resource_response_not_found_maps_error() {
        // Error classification uses the code field, not message content.
        let response = serde_json::json!({
            "status": "error",
            "code": "not_found",
            "message": "No such file or directory"
        });
        let mapped = VmCapsuleProvider::to_resource_response(ResourceAction::Read, response);
        assert!(matches!(mapped, Err(ProviderError::NotFound(_))));
    }

    #[test]
    fn test_to_resource_response_unknown_code_is_generic() {
        // Unknown code should NOT be classified as NotFound even if message
        // contains "not found" — prevents spoofing via crafted messages.
        let response = serde_json::json!({
            "status": "error",
            "code": "read_failed",
            "message": "No such file or directory"
        });
        let mapped = VmCapsuleProvider::to_resource_response(ResourceAction::Read, response);
        assert!(matches!(mapped, Err(ProviderError::Provider(_))));
    }

    #[test]
    fn test_init_failure_clears_guard() {
        let bridge = VmRawBridge::new("127.0.0.1".into(), 1, serde_json::json!({}));

        let err1 = bridge.send_raw_blocking(&serde_json::json!({"op": "ping"}));
        assert!(err1.is_err());
        assert!(
            bridge.io.lock().unwrap().is_none(),
            "guard must be None after connect failure"
        );

        let err2 = bridge.send_raw_blocking(&serde_json::json!({"op": "ping"}));
        assert!(err2.is_err());
        assert!(
            bridge.io.lock().unwrap().is_none(),
            "guard must remain None after repeated connect failure"
        );
    }

    #[test]
    fn test_local_tcp_compatibility_host_accepts_local_targets() {
        for host in [
            "localhost",
            "127.0.0.1",
            "10.0.0.5",
            "192.168.4.7",
            "169.254.1.2",
        ] {
            let bridge = VmRawBridge::new(host.into(), 4100, serde_json::json!({}));
            assert!(
                bridge.validate_local_tcp_compatibility_host().is_ok(),
                "{host}"
            );
        }
    }

    #[test]
    fn test_local_tcp_compatibility_host_rejects_non_local_targets() {
        for host in ["example.com", "8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            let bridge = VmRawBridge::new(host.into(), 4100, serde_json::json!({}));
            assert!(
                bridge.validate_local_tcp_compatibility_host().is_err(),
                "{host}"
            );
        }
    }
}
