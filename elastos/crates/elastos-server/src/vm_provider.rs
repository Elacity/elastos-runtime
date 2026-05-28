//! VM-backed provider bridge for supervisor-launched capsule providers.
//!
//! This adapter implements the runtime `Provider` trait and forwards raw JSON
//! requests over the Carrier-managed guest control network. Capsules are
//! expected to expose line-delimited JSON on the configured port.
//!
//! Two host→guest transports are supported:
//!
//! 1. **Linux flow (default).** Crosvm's TAP networking gives the host a
//!    routable IP for the guest, and the bridge dials it via TCP (or
//!    `AF_VSOCK` when `guest_host` parses as a numeric CID). Byte-identical
//!    to the pre-Phase-3 behaviour.
//! 2. **macOS flow (Phase 3 Day 6).** Apple's `Virtualization.framework`
//!    forbids `socket(AF_VSOCK, …)`; the only supported host→guest channel
//!    is `VZVirtioSocketDevice.connectToPort:`. The supervisor registers a
//!    [`MacVsockDial`] closure when launching a Vz VM, and the bridge
//!    uses it instead of opening an `AF_VSOCK` socket. The closure looks
//!    up the live [`crate::supervisor::RunningCapsule`] for the handle,
//!    downcasts to the `VzVm` backend, and calls `RunningVm::connect_vsock`
//!    (Phase 3 Day 5 primitive).

use std::future::Future;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::os::fd::{AsRawFd, OwnedFd};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use elastos_runtime::provider::{
    EntryType, Provider, ProviderError, ResourceAction, ResourceEntry, ResourceRequest,
    ResourceResponse,
};

/// Boxed-future closure that dials the guest's vsock listener on a given
/// port and returns an owned host-side fd. **Phase 3 Day 6.**
///
/// The supervisor builds one of these per Mac microVM provider route at
/// launch time. The closure captures a `Weak` reference to the running
/// map plus the capsule handle, so the lookup is lazy and a torn-down
/// VM cleanly surfaces `io::ErrorKind::NotConnected`.
pub type MacVsockDial = Arc<
    dyn Fn(u32) -> Pin<Box<dyn Future<Output = std::io::Result<OwnedFd>> + Send>> + Send + Sync,
>;

struct VmIo {
    reader: BufReader<Box<dyn Read + Send>>,
    writer: Box<dyn Write + Send>,
    raw_fd: i32, // For poll() — the underlying socket fd
}

const VM_PROVIDER_DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(15);
const VM_PROVIDER_LAUNCH_READ_TIMEOUT: Duration = Duration::from_secs(300);
#[cfg(test)]
const VM_PROVIDER_CONNECT_ATTEMPTS: usize = 3;
#[cfg(not(test))]
const VM_PROVIDER_CONNECT_ATTEMPTS: usize = 150;

struct VmRawBridge {
    guest_host: String,
    guest_port: u16,
    init_config: serde_json::Value,
    io: Mutex<Option<VmIo>>,
    /// Phase 3 Day 6: when present, the bridge bypasses `socket(AF_VSOCK,…)`
    /// and instead calls this closure to obtain the host-side fd. Required
    /// for macOS, where `AF_VSOCK` is not exposed to userspace.
    mac_vsock_dialer: Option<MacVsockDial>,
}

impl VmRawBridge {
    fn new(guest_host: String, guest_port: u16, init_config: serde_json::Value) -> Self {
        Self {
            guest_host,
            guest_port,
            init_config,
            io: Mutex::new(None),
            mac_vsock_dialer: None,
        }
    }

    /// Phase 3 Day 6 — Mac construction.
    ///
    /// `guest_host` is still passed for log parity with the Linux flow
    /// (it shows up in error messages and traces), but the actual dial
    /// goes through the supplied closure rather than an `AF_VSOCK`
    /// socket. The Linux side keeps using [`VmRawBridge::new`].
    fn new_with_vsock_dialer(
        guest_host: String,
        guest_port: u16,
        init_config: serde_json::Value,
        dialer: MacVsockDial,
    ) -> Self {
        Self {
            guest_host,
            guest_port,
            init_config,
            io: Mutex::new(None),
            mac_vsock_dialer: Some(dialer),
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
        // Phase 3 Day 6: prefer the Mac vsock dialer when set. We
        // route by closure presence rather than `cfg!(target_os)` so
        // tests on either platform can inject a fake dialer to
        // exercise the bridge end-to-end without touching the kernel.
        if let Some(dialer) = self.mac_vsock_dialer.clone() {
            return self.try_mac_vsock_dial(dialer, self.guest_port as u32);
        }

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
            .set_read_timeout(Some(VM_PROVIDER_DEFAULT_READ_TIMEOUT))
            .map_err(|e| ProviderError::Provider(format!("tcp read timeout setup failed: {e}")))?;
        stream
            .set_write_timeout(Some(VM_PROVIDER_DEFAULT_READ_TIMEOUT))
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

    /// Phase 3 Day 6 — Mac transport.
    ///
    /// Drive the supervisor-provided dialer (which ultimately calls
    /// `VZVirtioSocketDevice.connectToPort:`) and wrap the resulting
    /// fd in the same blocking `VmIo` shape used by the Linux
    /// `AF_VSOCK` and TCP paths.
    ///
    /// We are called from inside `tokio::task::spawn_blocking`
    /// (`Provider::send_raw` → `block_in_place` semantics) so it is
    /// safe to drive a `Future` to completion with
    /// `Handle::block_on`. Doing so on a runtime worker thread would
    /// panic; doing it on a blocking-pool thread is the standard
    /// idiom for "sync API that needs async I/O".
    fn try_mac_vsock_dial(&self, dialer: MacVsockDial, port: u32) -> Result<VmIo, ProviderError> {
        use std::os::unix::io::FromRawFd;

        let owned_fd = tokio::runtime::Handle::current()
            .block_on(dialer(port))
            .map_err(|e| {
                ProviderError::Provider(format!(
                    "mac vsock dial to '{}' port {} failed: {}",
                    self.guest_host, port, e
                ))
            })?;

        // Mirror the AF_VSOCK arm: re-wrap the fd into a `File`,
        // clone it for the writer half, and stash the raw fd for
        // later `poll()` use in `wait_for_response`.
        let raw_fd = owned_fd.as_raw_fd();
        // SAFETY: `owned_fd` is the sole owner of this fd, returned
        // from `RunningVm::connect_vsock` (which itself `dup`s the
        // Vz-managed connection fd). Converting to `File` transfers
        // ownership; we immediately `try_clone` for the writer, and
        // the original `OwnedFd` is forgotten via `into_raw_fd`.
        let stream = unsafe {
            use std::os::fd::IntoRawFd;
            std::fs::File::from_raw_fd(owned_fd.into_raw_fd())
        };
        let writer = stream
            .try_clone()
            .map_err(|e| ProviderError::Provider(format!("mac vsock clone failed: {e}")))?;
        let reader: BufReader<Box<dyn Read + Send>> = BufReader::new(Box::new(stream));
        let writer: Box<dyn Write + Send> = Box::new(writer);

        Ok(VmIo {
            reader,
            writer,
            raw_fd,
        })
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
            let init_resp = match Self::send_line_and_read_json(
                io,
                &init_req,
                VM_PROVIDER_DEFAULT_READ_TIMEOUT,
            ) {
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
        let read_timeout = Self::read_timeout_for_request(request);
        match Self::send_line_and_read_json(io, request, read_timeout) {
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
        read_timeout: Duration,
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
            Self::wait_for_readable(io, read_timeout)?;
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

    fn read_timeout_for_request(request: &serde_json::Value) -> Duration {
        match request.get("op").and_then(|value| value.as_str()) {
            Some("launch") => VM_PROVIDER_LAUNCH_READ_TIMEOUT,
            _ => VM_PROVIDER_DEFAULT_READ_TIMEOUT,
        }
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

    /// Phase 3 Day 6 — macOS constructor.
    ///
    /// Identical to [`Self::new`] except the underlying bridge will
    /// dial through `dialer` rather than `socket(AF_VSOCK,…)`. Used
    /// only by the supervisor's `start_capsule_vm_macos` path; the
    /// Linux launch path continues to use [`Self::new`].
    pub fn new_with_vsock_dialer(
        scheme: impl Into<String>,
        guest_host: String,
        guest_port: u16,
        init_config: serde_json::Value,
        dialer: MacVsockDial,
    ) -> Self {
        let scheme = scheme.into().to_ascii_lowercase();
        let scheme: &'static str = Box::leak(scheme.into_boxed_str());
        Self {
            scheme,
            bridge: Arc::new(VmRawBridge::new_with_vsock_dialer(
                guest_host,
                guest_port,
                init_config,
                dialer,
            )),
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
            uri: "localhost://MyWebSite/Documents/a.txt".into(),
            _scheme: "localhost".into(),
            path: "MyWebSite/Documents/a.txt".into(),
            _capsule_id: "capsule-1".into(),
            action: ResourceAction::Read,
            content: None,
            recursive: false,
        };
        let raw = VmCapsuleProvider::to_raw_request(&req);
        assert_eq!(raw.get("op").and_then(|v| v.as_str()), Some("read"));
        assert_eq!(
            raw.get("path").and_then(|v| v.as_str()),
            Some("MyWebSite/Documents/a.txt")
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

    // -----------------------------------------------------------------
    // Phase 3 Day 6: MacVsockDial integration tests.
    // -----------------------------------------------------------------
    //
    // These tests exercise `VmRawBridge::try_mac_vsock_dial` without
    // touching the kernel's `AF_VSOCK` path. We inject a mock dialer
    // that hands over one end of a socketpair, and a "fake guest"
    // thread services the other end with a stock line-delimited JSON
    // protocol. The bridge cannot tell the difference between this
    // and a real `VZVirtioSocketDevice.connectToPort:` connection —
    // both surface as a single `OwnedFd`.

    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream as StdUnixStream;
    use std::sync::Mutex as StdMutex;

    /// Build a socketpair and return the two halves as owned fds.
    fn socketpair_owned_fds() -> (OwnedFd, OwnedFd) {
        let (a, b) = StdUnixStream::pair().expect("socketpair");
        a.set_nonblocking(false).unwrap();
        b.set_nonblocking(false).unwrap();
        (a.into(), b.into())
    }

    /// Build a `MacVsockDial` that hands out the fd held in `slot`
    /// on its first call and fails with `NotConnected` afterwards
    /// (mirrors a torn-down VM). We use a `Mutex<Option<OwnedFd>>`
    /// because the dialer's `Fn` signature does not allow direct
    /// moves out of captured state.
    fn one_shot_dialer(slot: Arc<StdMutex<Option<OwnedFd>>>) -> MacVsockDial {
        Arc::new(move |_port: u32| {
            let slot = slot.clone();
            Box::pin(async move {
                let mut guard = slot.lock().unwrap();
                guard.take().ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        "dialer slot drained — vm gone",
                    )
                })
            })
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn vm_capsule_provider_uses_mac_dialer_when_present() {
        // Host end is what the dialer hands to the bridge; guest end
        // is what the fake guest thread reads/writes.
        let (host_fd, guest_fd) = socketpair_owned_fds();
        let slot = Arc::new(StdMutex::new(Some(host_fd)));
        let dialer = one_shot_dialer(slot);

        // Fake guest: handle the bridge's init handshake first
        // (`{"op":"init", "config": {…}}`), then service the actual
        // request. Mirrors what a real provider capsule does inside
        // its vsock listener loop on the guest side.
        //
        // After the response we block on a final read so the
        // socket stays open until the bridge has actually parsed
        // the response. Without this the bridge's `poll()` races
        // the guest's drop and observes `POLLIN | POLLHUP` (0x11),
        // which it treats as unhealthy and fails the request.
        let guest_handle = std::thread::spawn(move || {
            use std::io::{BufRead as _, BufReader, Read as _, Write as _};
            let stream: StdUnixStream = guest_fd.into();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;

            let mut init_line = String::new();
            reader.read_line(&mut init_line).expect("guest read init");
            let init_parsed: serde_json::Value =
                serde_json::from_str(&init_line).expect("guest parse init");
            assert_eq!(
                init_parsed.get("op").and_then(|v| v.as_str()),
                Some("init"),
                "first frame must be init"
            );
            writer
                .write_all(b"{\"status\":\"ok\"}\n")
                .expect("guest init ack");

            let mut req_line = String::new();
            reader.read_line(&mut req_line).expect("guest read req");
            let req_parsed: serde_json::Value =
                serde_json::from_str(&req_line).expect("guest parse req");
            assert_eq!(req_parsed.get("op").and_then(|v| v.as_str()), Some("read"));
            writer
                .write_all(b"{\"status\":\"ok\",\"data\":{\"content\":[104,105]}}\n")
                .expect("guest write");

            // Stay alive until the bridge closes its half; this
            // prevents the bridge's poll() from observing POLLHUP
            // mid-response.
            let mut sink = [0u8; 64];
            let _ = reader.get_mut().read(&mut sink);
        });

        // Build the provider with the dialer; run send_raw on a
        // blocking thread (the bridge expects to be invoked from
        // `spawn_blocking`-equivalent context, exactly like the
        // Linux production path).
        let provider = VmCapsuleProvider::new_with_vsock_dialer(
            "localhost",
            "handle-test".into(),
            7000,
            serde_json::json!({}),
            dialer,
        );
        let bridge = provider.bridge.clone();
        let response = tokio::task::spawn_blocking(move || {
            bridge.send_raw_blocking(&serde_json::json!({
                "op": "read",
                "path": "anything",
                "token": ""
            }))
        })
        .await
        .expect("blocking task")
        .expect("bridge call");

        assert_eq!(response.get("status").and_then(|v| v.as_str()), Some("ok"));
        let content = response
            .get("data")
            .and_then(|d| d.get("content"))
            .and_then(|c| c.as_array())
            .expect("content array");
        assert_eq!(content.len(), 2);

        // Drop the provider so the bridge's writer half closes,
        // letting the guest's parking read return cleanly.
        drop(provider);
        guest_handle.join().expect("guest thread");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn vm_capsule_provider_propagates_dialer_errors() {
        // Empty slot — dialer's first call returns NotConnected.
        // This is the shape `start_capsule_vm_macos`'s dialer
        // produces when the running map no longer contains the
        // capsule handle (torn-down VM, supervisor reaped).
        let slot = Arc::new(StdMutex::new(None::<OwnedFd>));
        let dialer = one_shot_dialer(slot);

        let bridge = Arc::new(VmRawBridge::new_with_vsock_dialer(
            "handle-missing".into(),
            7000,
            serde_json::json!({}),
            dialer,
        ));
        let bridge_call = bridge.clone();
        let err = tokio::task::spawn_blocking(move || {
            bridge_call.send_raw_blocking(&serde_json::json!({"op":"ping"}))
        })
        .await
        .expect("blocking task")
        .expect_err("expected dialer error");

        // The connect() retry loop wraps the dialer's error in its
        // own message; we only require that the original payload
        // ("vm gone") shows up somewhere in the chain.
        let msg = err.to_string();
        assert!(
            msg.contains("vm gone") || msg.contains("dialer"),
            "expected dialer error to surface, got: {msg}"
        );

        // Bridge state must remain clean after a failed dial — the
        // io guard must not be set so a subsequent send retries the
        // dialer rather than reusing a half-built `VmIo`.
        assert!(bridge.io.lock().unwrap().is_none());
    }

    // ---------------------------------------------------------------
    // Phase 4 Day 3 — cross-VM RPC dispatch under N consumers × M
    // providers.
    //
    // Audit finding (documented in PHASE_4_DAY_3_NOTES.md): the host
    // bridge has NO request-id allocator. Pairing is by strict order
    // over a `Mutex<Option<VmIo>>`-protected single connection — N
    // concurrent callers against ONE `VmCapsuleProvider` serialize at
    // the Mutex, but N concurrent callers against M providers proceed
    // in parallel (M independent Mutexes / connections). This test
    // proves both halves: per-provider serialization is race-free
    // (no cross-talk, no lost responses, no spurious responses) and
    // cross-provider dispatch is fully concurrent.
    // ---------------------------------------------------------------

    /// Spawn a synthetic provider-VM thread on the guest end of a
    /// socketpair. It handles the bridge's `init` handshake, then
    /// echoes back each request with a per-provider marker and the
    /// original `nonce` so the consumer can prove its OWN request
    /// is what came back.
    fn spawn_synthetic_provider_vm(
        guest_fd: OwnedFd,
        provider_marker: &'static str,
        served_count: Arc<std::sync::atomic::AtomicUsize>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            use std::io::{BufRead as _, BufReader, Read as _, Write as _};
            let stream: StdUnixStream = guest_fd.into();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;

            // Init handshake — bridge sends `{"op":"init","config":…}` first.
            let mut init_line = String::new();
            if reader.read_line(&mut init_line).is_err() {
                return;
            }
            let _: serde_json::Value = match serde_json::from_str(&init_line) {
                Ok(v) => v,
                Err(_) => return,
            };
            if writer.write_all(b"{\"status\":\"ok\"}\n").is_err() {
                return;
            }

            // Service loop — read JSON line, echo back with marker.
            loop {
                let mut req_line = String::new();
                match reader.read_line(&mut req_line) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
                let parsed: serde_json::Value = match serde_json::from_str(&req_line) {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let nonce = parsed.get("nonce").and_then(|v| v.as_u64()).unwrap_or(0);
                let response = format!(
                    "{{\"status\":\"ok\",\"data\":{{\"provider\":\"{}\",\"nonce\":{}}}}}\n",
                    provider_marker, nonce
                );
                if writer.write_all(response.as_bytes()).is_err() {
                    break;
                }
                served_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }

            // Drain anything else the bridge writes after we exit
            // so the bridge's `flush` does not error.
            let mut sink = [0u8; 64];
            let _ = reader.get_mut().read(&mut sink);
        })
    }

    /// Two synthetic provider VMs + three concurrent consumer
    /// tasks each issuing 20 RPCs (10 per provider). Total: 60
    /// RPCs against the shared `Arc<VmCapsuleProvider>` pair.
    /// Each consumer's per-RPC response MUST carry its OWN nonce
    /// (strict-order pairing through the per-provider Mutex)
    /// and the provider's OWN marker (no cross-provider mixup).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cross_vm_rpc_dispatch_isolates_per_provider_under_n_consumers() {
        let (host_a, guest_a) = socketpair_owned_fds();
        let (host_b, guest_b) = socketpair_owned_fds();

        let served_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let served_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let guest_thread_a = spawn_synthetic_provider_vm(guest_a, "alpha", served_a.clone());
        let guest_thread_b = spawn_synthetic_provider_vm(guest_b, "bravo", served_b.clone());

        // One-shot dialers — each bridge will dial its provider
        // exactly once at first request, the slot drains after.
        let slot_a = Arc::new(StdMutex::new(Some(host_a)));
        let slot_b = Arc::new(StdMutex::new(Some(host_b)));
        let dialer_a = one_shot_dialer(slot_a);
        let dialer_b = one_shot_dialer(slot_b);

        let provider_a = Arc::new(VmCapsuleProvider::new_with_vsock_dialer(
            "alpha-provider",
            "vm-stress-a".into(),
            7000,
            serde_json::json!({}),
            dialer_a,
        ));
        let provider_b = Arc::new(VmCapsuleProvider::new_with_vsock_dialer(
            "bravo-provider",
            "vm-stress-b".into(),
            7000,
            serde_json::json!({}),
            dialer_b,
        ));

        // Three consumer tasks, each issues 10 RPCs to alpha then
        // 10 to bravo, interleaved (alpha, bravo, alpha, bravo …)
        // so the per-provider Mutex sees genuinely interleaved
        // contention.
        const CONSUMERS: usize = 3;
        const PER_PROVIDER_PER_CONSUMER: usize = 10;
        let mut set = tokio::task::JoinSet::new();
        for consumer_idx in 0..CONSUMERS {
            let provider_a = Arc::clone(&provider_a);
            let provider_b = Arc::clone(&provider_b);
            set.spawn(async move {
                let mut results = Vec::with_capacity(2 * PER_PROVIDER_PER_CONSUMER);
                for iteration in 0..PER_PROVIDER_PER_CONSUMER {
                    let nonce_a = (consumer_idx as u64) * 1_000_000 + iteration as u64;
                    let nonce_b = nonce_a + 500_000;

                    let req_a = serde_json::json!({
                        "op": "ping",
                        "nonce": nonce_a,
                    });
                    let req_b = serde_json::json!({
                        "op": "ping",
                        "nonce": nonce_b,
                    });

                    let resp_a = provider_a
                        .send_raw(&req_a)
                        .await
                        .unwrap_or_else(|e| panic!("alpha send_raw {nonce_a}: {e}"));
                    let resp_b = provider_b
                        .send_raw(&req_b)
                        .await
                        .unwrap_or_else(|e| panic!("bravo send_raw {nonce_b}: {e}"));

                    results.push(("alpha", nonce_a, resp_a));
                    results.push(("bravo", nonce_b, resp_b));
                }
                results
            });
        }

        let mut all = Vec::new();
        while let Some(joined) = set.join_next().await {
            all.extend(joined.expect("consumer task must not panic"));
        }

        assert_eq!(
            all.len(),
            CONSUMERS * 2 * PER_PROVIDER_PER_CONSUMER,
            "expected 60 round-trips total"
        );

        for (expected_provider, expected_nonce, response) in &all {
            let data = response
                .get("data")
                .unwrap_or_else(|| panic!("response missing data: {response}"));
            assert_eq!(
                data.get("provider").and_then(|v| v.as_str()),
                Some(*expected_provider),
                "response routed through the wrong provider: nonce={expected_nonce} resp={response}"
            );
            assert_eq!(
                data.get("nonce").and_then(|v| v.as_u64()),
                Some(*expected_nonce),
                "nonce mismatch — pairing broke. expected provider={expected_provider} nonce={expected_nonce} resp={response}"
            );
        }

        // Each provider must have served exactly half the calls.
        assert_eq!(
            served_a.load(std::sync::atomic::Ordering::Relaxed),
            CONSUMERS * PER_PROVIDER_PER_CONSUMER,
            "provider alpha must have served 30 requests"
        );
        assert_eq!(
            served_b.load(std::sync::atomic::Ordering::Relaxed),
            CONSUMERS * PER_PROVIDER_PER_CONSUMER,
            "provider bravo must have served 30 requests"
        );

        // Drop providers so the bridges close their writer halves
        // and the guest threads exit cleanly.
        drop(provider_a);
        drop(provider_b);
        guest_thread_a.join().expect("alpha guest thread");
        guest_thread_b.join().expect("bravo guest thread");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mac_vsock_dialer_takes_priority_over_af_vsock_path() {
        // Defensive: the dialer-bearing bridge must NOT fall through
        // to the AF_VSOCK socket path. We verify this by pointing
        // `guest_host` at a value that would otherwise parse as a
        // numeric CID (`42`) — the dialer must short-circuit and
        // produce its own error, NOT one mentioning
        // `vsock connect to CID 42:…`.
        let slot = Arc::new(StdMutex::new(None::<OwnedFd>));
        let dialer = one_shot_dialer(slot);

        let bridge =
            VmRawBridge::new_with_vsock_dialer("42".into(), 7000, serde_json::json!({}), dialer);
        let result = tokio::task::spawn_blocking(move || bridge.try_connect_once())
            .await
            .unwrap();
        let err = match result {
            Ok(_) => panic!("dialer returned no fd; expected error"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(
            !msg.contains("vsock connect to CID"),
            "AF_VSOCK fallback fired despite Mac dialer being set: {msg}"
        );
        assert!(
            msg.contains("mac vsock dial"),
            "expected the mac dialer's error wrapper, got: {msg}"
        );
    }
}
