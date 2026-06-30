//! ElastOS Browser VM Runtime relay.
//!
//! This guest-side helper exposes the Unix Exit socket expected by
//! `browser-native-proxy-engine`, then forwards each stream to a host-owned
//! Runtime bridge. It does not perform DNS or open public TCP itself.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpStream};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

const CONFIG_ENV: &str = "ELASTOS_BROWSER_VM_RUNTIME_RELAY_CONFIG";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayConfig {
    schema: String,
    guest_relay_ipc_path: String,
    network_mode: NetworkMode,
    direct_network: bool,
    transport: HostTransportConfig,
    #[serde(default)]
    replace_existing_socket: bool,
    #[serde(default = "default_buffer_bytes")]
    buffer_bytes: usize,
    #[serde(default)]
    max_sessions: usize,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum NetworkMode {
    RuntimeNetOnly,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum HostTransportConfig {
    UnixSocket { path: String },
    TcpConnect { host: String, port: u16 },
    VsockConnect { cid: u32, port: u32 },
    VsockListen { port: u32 },
}

enum RuntimeTransport {
    UnixSocket { path: String },
    TcpConnect { addr: SocketAddr },
    VsockConnect { cid: u32, port: u32 },
    VsockListen { listener: Arc<Mutex<VsockListener>> },
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
    }
}

fn run_from_env(stdout: &mut dyn Write) -> Result<(), String> {
    let raw = std::env::var(CONFIG_ENV).map_err(|_| format!("{CONFIG_ENV} is required"))?;
    let config: RelayConfig =
        serde_json::from_str(&raw).map_err(|err| format!("{CONFIG_ENV} is invalid JSON: {err}"))?;
    run_relay(&config, stdout)
}

fn run_relay(config: &RelayConfig, stdout: &mut dyn Write) -> Result<(), String> {
    validate_config(config)?;
    let runtime_transport = RuntimeTransport::from_config(&config.transport)?;
    let guest_path = Path::new(&config.guest_relay_ipc_path);
    prepare_socket_path(guest_path, config.replace_existing_socket)?;
    let listener = UnixListener::bind(guest_path).map_err(|err| err.to_string())?;
    let _socket_guard = SocketFileGuard::new(guest_path);

    writeln!(
        stdout,
        "{}",
        json!({
            "schema": "elastos.browser.vm-runtime-relay.ready/v1",
            "guest_relay_ipc_path": config.guest_relay_ipc_path,
            "transport": transport_label(&config.transport),
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "buffer_bytes": config.buffer_bytes,
            "max_sessions": config.max_sessions,
        })
    )
    .map_err(|err| err.to_string())?;
    stdout.flush().map_err(|err| err.to_string())?;

    let runtime_transport = Arc::new(runtime_transport);
    let mut accepted = 0_usize;
    let mut workers = Vec::new();
    loop {
        if config.max_sessions > 0 && accepted >= config.max_sessions {
            break;
        }
        let (guest_stream, _) = listener.accept().map_err(|err| err.to_string())?;
        accepted += 1;
        let transport = Arc::clone(&runtime_transport);
        let buffer_bytes = config.buffer_bytes;
        let session_id = accepted;
        eprintln!("browser VM runtime relay accepted session {session_id}");
        workers.push(thread::spawn(move || {
            let host_stream = transport.connect()?;
            let (guest_to_runtime, runtime_to_guest) =
                forward_pair(DuplexStream::Unix(guest_stream), host_stream, buffer_bytes)?;
            eprintln!(
                "browser VM runtime relay session {session_id} guest_to_runtime={guest_to_runtime} runtime_to_guest={runtime_to_guest}"
            );
            Ok::<(), String>(())
        }));
    }

    for worker in workers {
        worker
            .join()
            .map_err(|_| "browser VM runtime relay worker panicked".to_string())??;
    }
    Ok(())
}

fn validate_config(config: &RelayConfig) -> Result<(), String> {
    if config.schema != "elastos.browser.vm-runtime-relay.config/v1" {
        return Err("unsupported browser VM runtime relay config schema".to_string());
    }
    validate_unix_socket_path("guest_relay_ipc_path", &config.guest_relay_ipc_path)?;
    if config.network_mode != NetworkMode::RuntimeNetOnly {
        return Err("browser VM runtime relay must be runtime_net_only".to_string());
    }
    if config.direct_network {
        return Err("browser VM runtime relay must not grant direct network".to_string());
    }
    match &config.transport {
        HostTransportConfig::UnixSocket { path } => {
            validate_unix_socket_path("transport.path", path)?
        }
        HostTransportConfig::TcpConnect { host, port } => {
            validate_tcp_target(host, *port)?;
        }
        HostTransportConfig::VsockConnect { cid, port } => {
            validate_vsock_target(*cid, *port)?;
        }
        HostTransportConfig::VsockListen { port } => {
            validate_vsock_port(*port)?;
        }
    }
    if config.buffer_bytes < 1024 || config.buffer_bytes > 1024 * 1024 {
        return Err("buffer_bytes must be between 1024 and 1048576".to_string());
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

fn validate_vsock_target(cid: u32, port: u32) -> Result<(), String> {
    if cid == 0 {
        return Err("vsock cid must be non-zero".to_string());
    }
    validate_vsock_port(port)
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
            "guest_relay_ipc_path already exists: {}",
            path.display()
        ));
    }
    if !metadata.file_type().is_socket() {
        return Err("guest_relay_ipc_path exists and is not a Unix socket".to_string());
    }
    fs::remove_file(path).map_err(|err| err.to_string())
}

fn transport_label(transport: &HostTransportConfig) -> &'static str {
    match transport {
        HostTransportConfig::UnixSocket { .. } => "unix_socket",
        HostTransportConfig::TcpConnect { .. } => "tcp_connect",
        HostTransportConfig::VsockConnect { .. } => "vsock_connect",
        HostTransportConfig::VsockListen { .. } => "vsock_listen",
    }
}

fn default_buffer_bytes() -> usize {
    16 * 1024
}

impl RuntimeTransport {
    fn from_config(config: &HostTransportConfig) -> Result<Self, String> {
        match config {
            HostTransportConfig::UnixSocket { path } => Ok(Self::UnixSocket { path: path.clone() }),
            HostTransportConfig::TcpConnect { host, port } => Ok(Self::TcpConnect {
                addr: tcp_socket_addr(host, *port)?,
            }),
            HostTransportConfig::VsockConnect { cid, port } => Ok(Self::VsockConnect {
                cid: *cid,
                port: *port,
            }),
            HostTransportConfig::VsockListen { port } => Ok(Self::VsockListen {
                listener: Arc::new(Mutex::new(VsockListener::bind(*port)?)),
            }),
        }
    }

    fn connect(&self) -> Result<DuplexStream, String> {
        match self {
            RuntimeTransport::UnixSocket { path } => UnixStream::connect(path)
                .map(DuplexStream::Unix)
                .map_err(|err| format!("Runtime host bridge unavailable: {err}")),
            RuntimeTransport::TcpConnect { addr } => TcpStream::connect(addr)
                .map(DuplexStream::Tcp)
                .map_err(|err| format!("Runtime host TCP bridge unavailable at {addr}: {err}")),
            RuntimeTransport::VsockConnect { cid, port } => {
                connect_vsock(*cid, *port).map(DuplexStream::File)
            }
            RuntimeTransport::VsockListen { listener } => listener
                .lock()
                .map_err(|_| "vsock listener lock poisoned".to_string())?
                .accept()
                .map(DuplexStream::File),
        }
    }
}

impl DuplexStream {
    fn try_clone(&self) -> Result<Self, String> {
        match self {
            DuplexStream::Unix(stream) => stream
                .try_clone()
                .map(DuplexStream::Unix)
                .map_err(|err| err.to_string()),
            DuplexStream::Tcp(stream) => stream
                .try_clone()
                .map(DuplexStream::Tcp)
                .map_err(|err| err.to_string()),
            DuplexStream::File(file) => file
                .try_clone()
                .map(DuplexStream::File)
                .map_err(|err| err.to_string()),
        }
    }

    fn shutdown_write(&self) {
        match self {
            DuplexStream::Unix(stream) => {
                let _ = stream.shutdown(Shutdown::Write);
            }
            DuplexStream::Tcp(stream) => {
                let _ = stream.shutdown(Shutdown::Write);
            }
            DuplexStream::File(file) => {
                let _ = shutdown_file_write(file);
            }
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

fn validate_tcp_target(host: &str, port: u16) -> Result<(), String> {
    tcp_socket_addr(host, port).map(|_| ())
}

fn forward_pair(
    guest: DuplexStream,
    runtime: DuplexStream,
    buffer_bytes: usize,
) -> Result<(u64, u64), String> {
    let mut guest_to_runtime_in = guest.try_clone()?;
    let mut runtime_to_guest_out = guest;
    let mut runtime_to_guest_in = runtime.try_clone()?;
    let mut guest_to_runtime_out = runtime;

    let forward_to_runtime = thread::spawn(move || {
        let result = copy_stream(
            &mut guest_to_runtime_in,
            &mut guest_to_runtime_out,
            buffer_bytes,
        );
        guest_to_runtime_out.shutdown_write();
        result
    });
    let forward_to_guest = copy_stream(
        &mut runtime_to_guest_in,
        &mut runtime_to_guest_out,
        buffer_bytes,
    );
    runtime_to_guest_out.shutdown_write();
    let forward_to_runtime = forward_to_runtime
        .join()
        .map_err(|_| "browser VM runtime relay copy worker panicked".to_string())?;
    Ok((forward_to_runtime?, forward_to_guest?))
}

fn shutdown_file_write(file: &File) -> Result<(), String> {
    let result = unsafe { libc::shutdown(file.as_raw_fd(), libc::SHUT_WR) };
    if result < 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    Ok(())
}

fn copy_stream<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    buffer_bytes: usize,
) -> Result<u64, String> {
    let mut buffer = vec![0_u8; buffer_bytes];
    let mut total = 0_u64;
    loop {
        let read = reader.read(&mut buffer).map_err(|err| err.to_string())?;
        if read == 0 {
            return Ok(total);
        }
        total += read as u64;
        writer
            .write_all(&buffer[..read])
            .map_err(|err| err.to_string())?;
        writer.flush().map_err(|err| err.to_string())?;
    }
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
fn connect_vsock(cid: u32, port: u32) -> Result<File, String> {
    unsafe {
        let fd = libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0);
        if fd < 0 {
            return Err(format!(
                "vsock socket() failed: {}",
                io::Error::last_os_error()
            ));
        }
        let addr = sockaddr_vm(cid, port);
        let result = libc::connect(
            fd,
            &addr as *const SockAddrVm as *const libc::sockaddr,
            std::mem::size_of::<SockAddrVm>() as libc::socklen_t,
        );
        if result < 0 {
            let err = io::Error::last_os_error();
            libc::close(fd);
            return Err(format!("vsock connect to {cid}:{port} failed: {err}"));
        }
        Ok(File::from_raw_fd(fd))
    }
}

#[cfg(not(target_os = "linux"))]
fn connect_vsock(_cid: u32, _port: u32) -> Result<File, String> {
    Err("vsock transport is available only in the Linux guest target".to_string())
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
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    fn temp_socket_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "elastos-browser-vm-runtime-relay-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("temp socket dir");
        path
    }

    fn config(guest_relay_ipc_path: String, host_path: String) -> RelayConfig {
        RelayConfig {
            schema: "elastos.browser.vm-runtime-relay.config/v1".to_string(),
            guest_relay_ipc_path,
            network_mode: NetworkMode::RuntimeNetOnly,
            direct_network: false,
            transport: HostTransportConfig::UnixSocket { path: host_path },
            replace_existing_socket: false,
            buffer_bytes: 1024,
            max_sessions: 1,
        }
    }

    fn wait_for_socket(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if path.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for socket {}", path.display());
    }

    #[test]
    fn validates_runtime_only_config() {
        let dir = temp_socket_dir();
        let relay = config(
            dir.join("guest.sock").display().to_string(),
            dir.join("host.sock").display().to_string(),
        );
        assert!(validate_config(&relay).is_ok());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_direct_network_or_bad_paths() {
        let dir = temp_socket_dir();
        let mut relay = config(
            dir.join("guest.sock").display().to_string(),
            dir.join("host.sock").display().to_string(),
        );
        relay.direct_network = true;
        assert!(validate_config(&relay)
            .unwrap_err()
            .contains("direct network"));

        let mut bad_path = relay;
        bad_path.direct_network = false;
        bad_path.guest_relay_ipc_path = "relative.sock".to_string();
        assert!(validate_config(&bad_path)
            .unwrap_err()
            .contains("absolute Unix socket path"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn validates_vsock_contract_without_opening_vsock() {
        let dir = temp_socket_dir();
        let mut relay = config(
            dir.join("guest.sock").display().to_string(),
            dir.join("host.sock").display().to_string(),
        );
        relay.transport = HostTransportConfig::VsockListen { port: 19091 };
        assert!(validate_config(&relay).is_ok());
        relay.transport = HostTransportConfig::VsockConnect {
            cid: 2,
            port: 19091,
        };
        assert!(validate_config(&relay).is_ok());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn validates_private_tcp_target_without_opening_tcp() {
        let dir = temp_socket_dir();
        let mut relay = config(
            dir.join("guest.sock").display().to_string(),
            dir.join("host.sock").display().to_string(),
        );
        relay.transport = HostTransportConfig::TcpConnect {
            host: "192.168.253.1".to_string(),
            port: 19091,
        };
        assert!(validate_config(&relay).is_ok());
        relay.transport = HostTransportConfig::TcpConnect {
            host: "runtime-host.local".to_string(),
            port: 19091,
        };
        assert!(validate_config(&relay)
            .unwrap_err()
            .contains("literal IP address"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn forwards_bytes_between_guest_unix_and_host_unix_bridge() {
        let dir = temp_socket_dir();
        let guest_path = dir.join("guest.sock");
        let host_path = dir.join("host.sock");
        let host_listener = UnixListener::bind(&host_path).expect("host listener");
        let relay_config = config(
            guest_path.display().to_string(),
            host_path.display().to_string(),
        );

        let relay_handle = thread::spawn(move || {
            let mut ready = Vec::new();
            run_relay(&relay_config, &mut ready).expect("relay run");
            String::from_utf8(ready).expect("ready output")
        });

        wait_for_socket(&guest_path);
        let host_handle = thread::spawn(move || {
            let (mut host, _) = host_listener.accept().expect("host accept");
            let mut request = [0_u8; 4];
            host.read_exact(&mut request).expect("host read");
            assert_eq!(&request, b"ping");
            host.write_all(b"pong").expect("host write");
        });

        let mut guest = UnixStream::connect(&guest_path).expect("guest connect");
        guest.write_all(b"ping").expect("guest write");
        let mut response = [0_u8; 4];
        guest.read_exact(&mut response).expect("guest read");
        assert_eq!(&response, b"pong");
        drop(guest);

        host_handle.join().expect("host thread");
        let ready = relay_handle.join().expect("relay thread");
        assert!(ready.contains("elastos.browser.vm-runtime-relay.ready/v1"));
        assert!(ready.contains("runtime_net_only"));
        let _ = fs::remove_dir_all(dir);
    }
}
