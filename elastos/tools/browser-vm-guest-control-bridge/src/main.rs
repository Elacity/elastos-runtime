//! ElastOS Browser VM guest control bridge.
//!
//! This guest-side helper exposes the VM-local Browser control service to the
//! host substrate over an explicit VM transport. It does not open public
//! network egress and it does not interpret Browser control payloads.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

const CONFIG_ENV: &str = "ELASTOS_BROWSER_VM_CONTROL_BRIDGE_CONFIG";
const VM_LOG_DIR: &str = "/var/log/elastos";
const MAX_CONTROL_HTTP_HEADER_BYTES: usize = 64 * 1024;
const MAX_CONTROL_HTTP_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_CONTROL_HTTP_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_CONTROL_SOCKET_READY_TIMEOUT_MS: u64 = 60_000;
const DEFAULT_CONTROL_REQUEST_TIMEOUT_MS: u64 = 90_000;
const VM_LOG_NAMES: &[&str] = &[
    "browser-vm-init.log",
    "browser-vm-selkies-control.log",
    "browser-vm-xvfb.log",
    "browser-vm-native-proxy.log",
    "browser-vm-chromium.log",
    "browser-vm-selkies.log",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BridgeConfig {
    schema: String,
    guest_control_socket_path: String,
    network_mode: NetworkMode,
    direct_network: bool,
    transport: HostListenConfig,
    #[serde(default)]
    replace_existing_socket: bool,
    #[serde(default = "default_buffer_bytes")]
    buffer_bytes: usize,
    #[serde(default)]
    max_sessions: usize,
    #[serde(default = "default_control_socket_ready_timeout_ms")]
    control_socket_ready_timeout_ms: u64,
    #[serde(default = "default_control_request_timeout_ms")]
    control_request_timeout_ms: u64,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum NetworkMode {
    RuntimeNetOnly,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum HostListenConfig {
    UnixListen { path: String },
    TcpListen { host: String, port: u16 },
    VsockListen { port: u32 },
}

enum HostListener {
    Unix {
        listener: UnixListener,
        _guard: SocketFileGuard,
    },
    Tcp(TcpListener),
    Vsock(VsockListener),
}

enum DuplexStream {
    Unix(UnixStream),
    Tcp(TcpStream),
    File(File),
}

fn main() {
    match run_from_env(&mut io::stdout()) {
        Ok(()) => {}
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    };
}

fn run_from_env(stdout: &mut dyn Write) -> Result<(), String> {
    let raw = std::env::var(CONFIG_ENV).map_err(|_| format!("{CONFIG_ENV} is required"))?;
    let config: BridgeConfig =
        serde_json::from_str(&raw).map_err(|err| format!("{CONFIG_ENV} is invalid JSON: {err}"))?;
    run_bridge(&config, stdout)
}

fn run_bridge(config: &BridgeConfig, stdout: &mut dyn Write) -> Result<(), String> {
    validate_config(config)?;
    let listener = HostListener::bind(&config.transport, config.replace_existing_socket)?;

    writeln!(
        stdout,
        "{}",
        json!({
            "schema": "elastos.browser.vm-guest-control-bridge.ready/v1",
            "guest_control_socket_path": config.guest_control_socket_path,
            "transport": transport_label(&config.transport),
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "buffer_bytes": config.buffer_bytes,
            "max_sessions": config.max_sessions,
            "control_socket_ready_timeout_ms": config.control_socket_ready_timeout_ms,
            "control_request_timeout_ms": config.control_request_timeout_ms,
        })
    )
    .map_err(|err| err.to_string())?;
    stdout.flush().map_err(|err| err.to_string())?;

    let mut accepted = 0_usize;
    let mut workers = Vec::new();
    loop {
        if config.max_sessions > 0 && accepted >= config.max_sessions {
            break;
        }
        let host_stream = listener.accept()?;
        accepted += 1;
        let session_id = accepted;
        eprintln!("browser VM guest control bridge accepted session {session_id}");
        let guest_control_path = config.guest_control_socket_path.clone();
        let buffer_bytes = config.buffer_bytes;
        let control_socket_ready_timeout_ms = config.control_socket_ready_timeout_ms;
        let control_request_timeout = Duration::from_millis(config.control_request_timeout_ms);
        workers.push(thread::spawn(move || {
            let guest_stream = match connect_guest_control_socket(
                &guest_control_path,
                Duration::from_millis(control_socket_ready_timeout_ms),
            ) {
                Ok(stream) => {
                    eprintln!(
                        "browser VM guest control bridge session {session_id} connected to {guest_control_path}"
                    );
                    DuplexStream::Unix(stream)
                }
                Err(err) => {
                    let message = format!("VM-local Browser control service unavailable: {err}");
                    eprintln!("{message}");
                    return write_http_error_response(host_stream, &message);
                }
            };
            if let Err(err) =
                proxy_http_control_request(
                    session_id,
                    host_stream,
                    guest_stream,
                    buffer_bytes,
                    control_request_timeout,
                )
            {
                eprintln!("browser VM guest control bridge session {session_id} failed: {err}");
            }
            Ok(())
        }));
    }

    for worker in workers {
        worker
            .join()
            .map_err(|_| "browser VM guest control bridge worker panicked".to_string())??;
    }
    Ok(())
}

fn proxy_http_control_request(
    session_id: usize,
    host: DuplexStream,
    guest: DuplexStream,
    buffer_bytes: usize,
    request_timeout: Duration,
) -> Result<(), String> {
    let mut host = host;
    let mut guest = guest;
    match proxy_http_control_request_inner(
        session_id,
        &mut host,
        &mut guest,
        buffer_bytes,
        request_timeout,
    ) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = write_http_error_response(host, &err);
            Err(err)
        }
    }
}

fn proxy_http_control_request_inner(
    session_id: usize,
    host: &mut DuplexStream,
    guest: &mut DuplexStream,
    buffer_bytes: usize,
    request_timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + request_timeout;
    let request = read_one_http_request(host, buffer_bytes, deadline)?;
    eprintln!(
        "browser VM guest control bridge session {session_id} request_bytes={}",
        request.len()
    );
    guest.write_all(&request).map_err(|err| err.to_string())?;
    guest.flush().map_err(|err| err.to_string())?;

    let response = read_one_http_response(guest, buffer_bytes, deadline)?;
    eprintln!(
        "browser VM guest control bridge session {session_id} response_bytes={}",
        response.len()
    );
    host.write_all(&response).map_err(|err| err.to_string())?;
    host.flush().map_err(|err| err.to_string())?;
    Ok(())
}

fn read_one_http_request(
    stream: &mut DuplexStream,
    buffer_bytes: usize,
    deadline: Instant,
) -> Result<Vec<u8>, String> {
    let mut request = Vec::new();
    let mut buffer = vec![0_u8; buffer_bytes];
    let header_end = loop {
        if let Some(position) = find_header_end(&request) {
            break position;
        }
        if request.len() > MAX_CONTROL_HTTP_HEADER_BYTES {
            return Err("Browser VM control HTTP request headers are too large".to_string());
        }
        wait_for_readable(stream, deadline)
            .map_err(|err| format!("Browser VM host control HTTP request timed out: {err}"))?;
        let read = stream.read(&mut buffer).map_err(|err| err.to_string())?;
        if read == 0 {
            return Err("Browser VM control HTTP request closed before headers".to_string());
        }
        request.extend_from_slice(&buffer[..read]);
    };
    let content_length = parse_content_length(&request[..header_end])?;
    if content_length > MAX_CONTROL_HTTP_BODY_BYTES {
        return Err("Browser VM control HTTP request body is too large".to_string());
    }
    let total = header_end + 4 + content_length;
    while request.len() < total {
        wait_for_readable(stream, deadline)
            .map_err(|err| format!("Browser VM host control HTTP request timed out: {err}"))?;
        let read = stream.read(&mut buffer).map_err(|err| err.to_string())?;
        if read == 0 {
            return Err("Browser VM control HTTP request closed before body".to_string());
        }
        request.extend_from_slice(&buffer[..read]);
    }
    request.truncate(total);
    Ok(request)
}

fn read_one_http_response(
    stream: &mut DuplexStream,
    buffer_bytes: usize,
    deadline: Instant,
) -> Result<Vec<u8>, String> {
    let mut response = Vec::new();
    let mut buffer = vec![0_u8; buffer_bytes];
    let header_end = loop {
        if let Some(position) = find_header_end(&response) {
            break position;
        }
        if response.len() > MAX_CONTROL_HTTP_HEADER_BYTES {
            return Err("Browser VM control HTTP response headers are too large".to_string());
        }
        wait_for_readable(stream, deadline)
            .map_err(|err| format!("Browser VM guest control HTTP response timed out: {err}"))?;
        let read = stream.read(&mut buffer).map_err(|err| err.to_string())?;
        if read == 0 {
            return Err("Browser VM control HTTP response closed before headers".to_string());
        }
        response.extend_from_slice(&buffer[..read]);
    };
    let content_length = parse_content_length(&response[..header_end])?;
    if content_length > MAX_CONTROL_HTTP_RESPONSE_BYTES {
        return Err("Browser VM control HTTP response body is too large".to_string());
    }
    let total = header_end + 4 + content_length;
    while response.len() < total {
        wait_for_readable(stream, deadline)
            .map_err(|err| format!("Browser VM guest control HTTP response timed out: {err}"))?;
        let read = stream.read(&mut buffer).map_err(|err| err.to_string())?;
        if read == 0 {
            return Err("Browser VM control HTTP response closed before body".to_string());
        }
        response.extend_from_slice(&buffer[..read]);
    }
    response.truncate(total);
    Ok(response)
}

fn wait_for_readable(stream: &DuplexStream, deadline: Instant) -> Result<(), String> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("deadline elapsed".to_string());
        }
        let timeout_ms = remaining.as_millis().clamp(1, libc::c_int::MAX as u128) as libc::c_int;
        let mut poll_fd = libc::pollfd {
            fd: stream.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
        if result > 0 {
            return Ok(());
        }
        if result == 0 {
            return Err("deadline elapsed".to_string());
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(error.to_string());
    }
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_content_length(header: &[u8]) -> Result<usize, String> {
    let text = std::str::from_utf8(header)
        .map_err(|_| "Browser VM control HTTP request headers are not UTF-8".to_string())?;
    let mut content_length = 0_usize;
    for line in text.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value
                .trim()
                .parse::<usize>()
                .map_err(|_| "Browser VM control HTTP request Content-Length is invalid")?;
        }
    }
    Ok(content_length)
}

fn validate_config(config: &BridgeConfig) -> Result<(), String> {
    if config.schema != "elastos.browser.vm-guest-control-bridge.config/v1" {
        return Err("unsupported browser VM guest control bridge config schema".to_string());
    }
    validate_unix_socket_path(
        "guest_control_socket_path",
        &config.guest_control_socket_path,
    )?;
    if config.network_mode != NetworkMode::RuntimeNetOnly {
        return Err("browser VM guest control bridge must be runtime_net_only".to_string());
    }
    if config.direct_network {
        return Err("browser VM guest control bridge must not grant direct network".to_string());
    }
    match &config.transport {
        HostListenConfig::UnixListen { path } => {
            validate_unix_socket_path("transport.path", path)?;
        }
        HostListenConfig::TcpListen { host, port } => {
            validate_tcp_listener(host, *port)?;
        }
        HostListenConfig::VsockListen { port } => {
            validate_vsock_port(*port)?;
        }
    }
    if config.buffer_bytes < 1024 || config.buffer_bytes > 1024 * 1024 {
        return Err("buffer_bytes must be between 1024 and 1048576".to_string());
    }
    if config.control_socket_ready_timeout_ms > 600_000 {
        return Err("control_socket_ready_timeout_ms must be at most 600000".to_string());
    }
    if config.control_request_timeout_ms > 600_000 {
        return Err("control_request_timeout_ms must be at most 600000".to_string());
    }
    Ok(())
}

fn validate_unix_socket_path(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || !value.starts_with('/') {
        return Err(format!("{label} must be an absolute Unix socket path"));
    }
    if value
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte == b'\0')
    {
        return Err(format!("{label} must not contain whitespace or NUL"));
    }
    Ok(())
}

fn validate_vsock_port(port: u32) -> Result<(), String> {
    if port == 0 {
        return Err("vsock port must be non-zero".to_string());
    }
    Ok(())
}

fn prepare_socket_path(path: &Path, replace_existing_socket: bool) -> Result<(), String> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if !replace_existing_socket {
        return Err(format!(
            "control bridge socket already exists: {}",
            path.display()
        ));
    }
    if !metadata.file_type().is_socket() {
        return Err("control bridge socket path exists and is not a Unix socket".to_string());
    }
    fs::remove_file(path).map_err(|err| err.to_string())
}

fn transport_label(transport: &HostListenConfig) -> &'static str {
    match transport {
        HostListenConfig::UnixListen { .. } => "unix_listen",
        HostListenConfig::TcpListen { .. } => "tcp_listen",
        HostListenConfig::VsockListen { .. } => "vsock_listen",
    }
}

fn default_buffer_bytes() -> usize {
    16 * 1024
}

fn default_control_socket_ready_timeout_ms() -> u64 {
    DEFAULT_CONTROL_SOCKET_READY_TIMEOUT_MS
}

fn default_control_request_timeout_ms() -> u64 {
    DEFAULT_CONTROL_REQUEST_TIMEOUT_MS
}

fn connect_guest_control_socket(path: &str, timeout: Duration) -> Result<UnixStream, io::Error> {
    let deadline = Instant::now() + timeout;
    loop {
        match UnixStream::connect(path) {
            Ok(stream) => return Ok(stream),
            Err(err) => {
                if Instant::now() >= deadline {
                    return Err(err);
                }
            }
        }
        thread::sleep(
            Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

impl HostListener {
    fn bind(config: &HostListenConfig, replace_existing_socket: bool) -> Result<Self, String> {
        match config {
            HostListenConfig::UnixListen { path } => {
                let path = Path::new(path);
                prepare_socket_path(path, replace_existing_socket)?;
                let listener = UnixListener::bind(path).map_err(|err| err.to_string())?;
                Ok(Self::Unix {
                    listener,
                    _guard: SocketFileGuard::new(path),
                })
            }
            HostListenConfig::TcpListen { host, port } => {
                let addr = tcp_socket_addr(host, *port)?;
                let listener = TcpListener::bind(addr).map_err(|err| err.to_string())?;
                Ok(Self::Tcp(listener))
            }
            HostListenConfig::VsockListen { port } => VsockListener::bind(*port).map(Self::Vsock),
        }
    }

    fn accept(&self) -> Result<DuplexStream, String> {
        match self {
            HostListener::Unix { listener, .. } => listener
                .accept()
                .map(|(stream, _)| DuplexStream::Unix(stream))
                .map_err(|err| err.to_string()),
            HostListener::Tcp(listener) => listener
                .accept()
                .map(|(stream, _)| DuplexStream::Tcp(stream))
                .map_err(|err| err.to_string()),
            HostListener::Vsock(listener) => listener.accept().map(DuplexStream::File),
        }
    }
}

impl Read for DuplexStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            DuplexStream::Unix(stream) => stream.read(buf),
            DuplexStream::Tcp(stream) => stream.read(buf),
            DuplexStream::File(file) => file.read(buf),
        }
    }
}

impl AsRawFd for DuplexStream {
    fn as_raw_fd(&self) -> RawFd {
        match self {
            DuplexStream::Unix(stream) => stream.as_raw_fd(),
            DuplexStream::Tcp(stream) => stream.as_raw_fd(),
            DuplexStream::File(file) => file.as_raw_fd(),
        }
    }
}

impl Write for DuplexStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            DuplexStream::Unix(stream) => stream.write(buf),
            DuplexStream::Tcp(stream) => stream.write(buf),
            DuplexStream::File(file) => file.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            DuplexStream::Unix(stream) => stream.flush(),
            DuplexStream::Tcp(stream) => stream.flush(),
            DuplexStream::File(file) => file.flush(),
        }
    }
}

fn tcp_socket_addr(host: &str, port: u16) -> Result<SocketAddr, String> {
    let ip: IpAddr = host
        .parse()
        .map_err(|_| "tcp host must be a literal IP address".to_string())?;
    if ip.is_unspecified() {
        return Err("tcp host must not be unspecified".to_string());
    }
    if port == 0 {
        return Err("tcp port must be non-zero".to_string());
    }
    Ok(SocketAddr::new(ip, port))
}

fn validate_tcp_listener(host: &str, port: u16) -> Result<(), String> {
    tcp_socket_addr(host, port).map(|_| ())
}

fn write_http_error_response(mut stream: DuplexStream, message: &str) -> Result<(), String> {
    let body = serde_json::to_vec(&json!({
        "schema": "elastos.browser.vm-guest-control-bridge.error/v1",
        "error": message,
        "logs": read_vm_log_tails(),
    }))
    .map_err(|err| err.to_string())?;
    write!(
        stream,
        "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .map_err(|err| err.to_string())?;
    stream.write_all(&body).map_err(|err| err.to_string())?;
    stream.flush().map_err(|err| err.to_string())
}

fn read_vm_log_tails() -> Value {
    let mut logs = serde_json::Map::new();
    for name in VM_LOG_NAMES {
        let path = Path::new(VM_LOG_DIR).join(name);
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let start = bytes.len().saturating_sub(12 * 1024);
        let tail = String::from_utf8_lossy(&bytes[start..]).to_string();
        logs.insert((*name).to_string(), json!(tail));
    }
    Value::Object(logs)
}

struct SocketFileGuard {
    path: PathBuf,
}

impl SocketFileGuard {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }
}

impl Drop for SocketFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

struct VsockListener {
    fd: RawFd,
}

impl VsockListener {
    fn bind(port: u32) -> Result<Self, String> {
        bind_vsock(port).map(|fd| Self { fd })
    }

    fn accept(&self) -> Result<File, String> {
        accept_vsock(self.fd)
    }
}

impl Drop for VsockListener {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct SockAddrVm {
    svm_family: libc::sa_family_t,
    svm_reserved1: libc::c_ushort,
    svm_port: libc::c_uint,
    svm_cid: libc::c_uint,
    svm_zero: [u8; 4],
}

#[cfg(target_os = "linux")]
fn sockaddr_vm(cid: u32, port: u32) -> SockAddrVm {
    SockAddrVm {
        svm_family: libc::AF_VSOCK as libc::sa_family_t,
        svm_reserved1: 0,
        svm_port: port,
        svm_cid: cid,
        svm_zero: [0; 4],
    }
}

#[cfg(target_os = "linux")]
fn bind_vsock(port: u32) -> Result<RawFd, String> {
    unsafe {
        let fd = libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0);
        if fd < 0 {
            return Err(format!(
                "vsock socket() failed: {}",
                io::Error::last_os_error()
            ));
        }
        let addr = sockaddr_vm(libc::VMADDR_CID_ANY, port);
        let bind_result = libc::bind(
            fd,
            &addr as *const SockAddrVm as *const libc::sockaddr,
            std::mem::size_of::<SockAddrVm>() as libc::socklen_t,
        );
        if bind_result < 0 {
            let err = io::Error::last_os_error();
            libc::close(fd);
            return Err(format!("vsock bind on port {port} failed: {err}"));
        }
        if libc::listen(fd, 128) < 0 {
            let err = io::Error::last_os_error();
            libc::close(fd);
            return Err(format!("vsock listen on port {port} failed: {err}"));
        }
        Ok(fd)
    }
}

#[cfg(not(target_os = "linux"))]
fn bind_vsock(_port: u32) -> Result<RawFd, String> {
    Err("vsock transport is available only in the Linux guest target".to_string())
}

#[cfg(target_os = "linux")]
fn accept_vsock(listener_fd: RawFd) -> Result<File, String> {
    unsafe {
        let fd = libc::accept(listener_fd, std::ptr::null_mut(), std::ptr::null_mut());
        if fd < 0 {
            return Err(format!(
                "vsock accept failed: {}",
                io::Error::last_os_error()
            ));
        }
        Ok(File::from_raw_fd(fd))
    }
}

#[cfg(not(target_os = "linux"))]
fn accept_vsock(_listener_fd: RawFd) -> Result<File, String> {
    Err("vsock transport is available only in the Linux guest target".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(transport: HostListenConfig) -> BridgeConfig {
        BridgeConfig {
            schema: "elastos.browser.vm-guest-control-bridge.config/v1".to_string(),
            guest_control_socket_path: "/tmp/elastos-browser-control.sock".to_string(),
            network_mode: NetworkMode::RuntimeNetOnly,
            direct_network: false,
            transport,
            replace_existing_socket: false,
            buffer_bytes: 1024,
            max_sessions: 1,
            control_socket_ready_timeout_ms: DEFAULT_CONTROL_SOCKET_READY_TIMEOUT_MS,
            control_request_timeout_ms: DEFAULT_CONTROL_REQUEST_TIMEOUT_MS,
        }
    }

    #[test]
    fn validates_private_tcp_listener_without_binding() {
        let bridge = config(HostListenConfig::TcpListen {
            host: "192.168.253.2".to_string(),
            port: 19092,
        });
        assert!(validate_config(&bridge).is_ok());
    }

    #[test]
    fn rejects_dns_or_unspecified_private_tcp_listener() {
        let bridge = config(HostListenConfig::TcpListen {
            host: "browser-vm.local".to_string(),
            port: 19092,
        });
        assert!(validate_config(&bridge)
            .unwrap_err()
            .contains("literal IP address"));

        let bridge = config(HostListenConfig::TcpListen {
            host: "0.0.0.0".to_string(),
            port: 19092,
        });
        assert!(validate_config(&bridge)
            .unwrap_err()
            .contains("must not be unspecified"));
    }

    #[test]
    fn stalled_guest_control_response_returns_http_error() {
        let (mut host_client, host_bridge) = UnixStream::pair().expect("host pair");
        let (guest_bridge, _silent_guest_control) = UnixStream::pair().expect("guest pair");
        let request = b"POST /pages HTTP/1.1\r\nHost: browser-vm\r\nContent-Length: 2\r\n\r\n{}";
        host_client.write_all(request).expect("write request");

        let err = proxy_http_control_request(
            1,
            DuplexStream::Unix(host_bridge),
            DuplexStream::Unix(guest_bridge),
            1024,
            Duration::from_millis(10),
        )
        .unwrap_err();
        assert!(err.contains("Browser VM guest control HTTP response timed out"));

        let mut response = String::new();
        host_client
            .read_to_string(&mut response)
            .expect("read error response");
        assert!(response.starts_with("HTTP/1.1 503 Service Unavailable"));
        assert!(response.contains("Browser VM guest control HTTP response timed out"));
    }

    #[test]
    fn stalled_host_control_request_returns_http_error() {
        let (mut host_client, host_bridge) = UnixStream::pair().expect("host pair");
        let (guest_bridge, _guest_control) = UnixStream::pair().expect("guest pair");

        let err = proxy_http_control_request(
            1,
            DuplexStream::Unix(host_bridge),
            DuplexStream::Unix(guest_bridge),
            1024,
            Duration::from_millis(10),
        )
        .unwrap_err();
        assert!(err.contains("Browser VM host control HTTP request timed out"));

        let mut response = String::new();
        host_client
            .read_to_string(&mut response)
            .expect("read error response");
        assert!(response.starts_with("HTTP/1.1 503 Service Unavailable"));
        assert!(response.contains("Browser VM host control HTTP request timed out"));
    }
}
