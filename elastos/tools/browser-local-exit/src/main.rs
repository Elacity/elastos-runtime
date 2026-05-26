//! ElastOS Browser local Exit relay.
//!
//! This helper is the first server-side Exit daemon for the Browser path. It
//! listens on one private Unix socket, accepts typed Runtime relay-open
//! handshakes, dials allowlisted public TCP targets, and then forwards bytes.
//! Browser capsules, Browser Engine adapters, and web pages never see this
//! socket or direct host networking.

use serde::Deserialize;
use serde_json::json;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use url::Url;

const CONFIG_ENV: &str = "ELASTOS_BROWSER_LOCAL_EXIT_CONFIG";
const MAX_HANDSHAKE_BYTES: usize = 16 * 1024;
const MAX_PROXY_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalExitConfig {
    schema: String,
    relay_ipc_path: String,
    allowed_hosts: Vec<String>,
    #[serde(default = "default_allowed_schemes")]
    allowed_schemes: Vec<String>,
    #[serde(default = "default_allowed_ports")]
    allowed_ports: Vec<u16>,
    #[serde(default)]
    allow_private_targets: bool,
    #[serde(default)]
    replace_existing_socket: bool,
    #[serde(default = "default_connect_timeout_ms")]
    connect_timeout_ms: u64,
    #[serde(default = "default_buffer_bytes")]
    buffer_bytes: usize,
    #[serde(default = "default_address_family")]
    address_family: AddressFamilyPolicy,
    #[serde(default)]
    upstream_http_proxy: Option<UpstreamHttpProxyConfig>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AddressFamilyPolicy {
    System,
    PreferIpv4,
    PreferIpv6,
    Ipv4Only,
    Ipv6Only,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpstreamHttpProxyConfig {
    url: String,
    #[serde(default)]
    authorization_header: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayOpen {
    schema: String,
    stream_id: String,
    target: String,
    #[serde(default)]
    scheme: Option<String>,
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    principal_id: Option<String>,
    #[serde(default)]
    reason: Option<String>,
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
    let config: LocalExitConfig =
        serde_json::from_str(&raw).map_err(|err| format!("{CONFIG_ENV} is invalid JSON: {err}"))?;
    run_server(config, stdout)
}

fn run_server(config: LocalExitConfig, stdout: &mut dyn Write) -> Result<(), String> {
    validate_config(&config)?;
    let path = Path::new(&config.relay_ipc_path);
    prepare_socket_path(path, config.replace_existing_socket)?;
    let listener = UnixListener::bind(path).map_err(|err| err.to_string())?;
    let _socket_guard = SocketFileGuard::new(path);

    writeln!(
        stdout,
        "{}",
        json!({
            "schema": "elastos.browser.local-exit.ready/v1",
            "relay_ipc_path": config.relay_ipc_path,
            "allowed_hosts": config.allowed_hosts,
            "allowed_schemes": config.allowed_schemes,
            "allowed_ports": config.allowed_ports,
            "address_family": address_family_label(config.address_family),
            "network_mode": "runtime_net_only",
            "direct_network": false,
        })
    )
    .map_err(|err| err.to_string())?;
    stdout.flush().map_err(|err| err.to_string())?;

    for stream in listener.incoming() {
        let stream = stream.map_err(|err| err.to_string())?;
        let config = config.clone();
        thread::spawn(move || {
            if let Err(err) = handle_session(stream, &config) {
                eprintln!("browser-local-exit session failed: {err}");
            }
        });
    }
    Ok(())
}

fn handle_session(mut runtime_stream: UnixStream, config: &LocalExitConfig) -> Result<(), String> {
    validate_config(config)?;
    let open = read_relay_open(&mut runtime_stream)?;
    let target = validate_relay_open(&open, config)?;
    let remote = connect_target(&target, config)?;
    forward_pair(runtime_stream, remote, config.buffer_bytes)
}

fn read_relay_open(stream: &mut UnixStream) -> Result<RelayOpen, String> {
    let mut bytes = Vec::new();
    loop {
        let mut byte = [0_u8; 1];
        stream
            .read_exact(&mut byte)
            .map_err(|err| err.to_string())?;
        if byte[0] == b'\n' {
            break;
        }
        bytes.push(byte[0]);
        if bytes.len() > MAX_HANDSHAKE_BYTES {
            return Err("relay-open handshake is too large".to_string());
        }
    }
    serde_json::from_slice(&bytes).map_err(|err| format!("invalid relay-open handshake: {err}"))
}

fn validate_relay_open(open: &RelayOpen, config: &LocalExitConfig) -> Result<Url, String> {
    if open.schema != "elastos.exit.relay-open/v1" {
        return Err("unsupported relay-open schema".to_string());
    }
    if !is_safe_id(&open.stream_id) {
        return Err("stream_id must be a safe identifier".to_string());
    }
    let target = Url::parse(&open.target).map_err(|err| err.to_string())?;
    if !matches!(target.scheme(), "tcp" | "tls") {
        return Err("relay target must use tcp or tls".to_string());
    }
    if !config
        .allowed_schemes
        .iter()
        .any(|scheme| scheme == target.scheme())
    {
        return Err(format!(
            "scheme is not allowlisted for local exit: {}",
            target.scheme()
        ));
    }
    let host = target
        .host_str()
        .ok_or_else(|| "relay target requires a host".to_string())?;
    let port = target
        .port_or_known_default()
        .ok_or_else(|| "relay target requires a port".to_string())?;
    if !config.allowed_ports.contains(&port) {
        return Err(format!("port is not allowlisted for local exit: {port}"));
    }
    if let Some(host_hint) = open.host.as_deref() {
        if host_hint != host {
            return Err("relay host hint does not match target".to_string());
        }
    }
    if let Some(scheme_hint) = open.scheme.as_deref() {
        if scheme_hint != target.scheme() {
            return Err("relay scheme hint does not match target".to_string());
        }
    }
    if !host_allowed(host, &config.allowed_hosts) {
        return Err(format!("host is not allowlisted for local exit: {host}"));
    }
    validate_public_host(host, config.allow_private_targets)?;
    let _ = &open.principal_id;
    let _ = &open.reason;
    Ok(target)
}

fn connect_target(target: &Url, config: &LocalExitConfig) -> Result<TcpStream, String> {
    if let Some(proxy) = &config.upstream_http_proxy {
        return connect_target_via_http_proxy(target, config, proxy);
    }
    let host = target
        .host_str()
        .ok_or_else(|| "relay target requires a host".to_string())?;
    let port = target
        .port_or_known_default()
        .ok_or_else(|| "relay target requires a port".to_string())?;
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|err| format!("target DNS resolution failed: {err}"))?;
    let addrs = ordered_socket_addrs(addrs, config.address_family);
    let timeout = Duration::from_millis(config.connect_timeout_ms);
    let mut last_error = None;
    for addr in addrs {
        if let Err(err) = validate_public_socket_addr(addr, config.allow_private_targets) {
            last_error = Some(err);
            continue;
        }
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => {
                stream.set_nodelay(true).map_err(|err| err.to_string())?;
                return Ok(stream);
            }
            Err(err) => last_error = Some(err.to_string()),
        }
    }
    Err(last_error.unwrap_or_else(|| "target resolved to no usable addresses".to_string()))
}

fn connect_target_via_http_proxy(
    target: &Url,
    config: &LocalExitConfig,
    proxy: &UpstreamHttpProxyConfig,
) -> Result<TcpStream, String> {
    let target_host = target
        .host_str()
        .ok_or_else(|| "relay target requires a host".to_string())?;
    let target_port = target
        .port_or_known_default()
        .ok_or_else(|| "relay target requires a port".to_string())?;
    let proxy_url =
        Url::parse(&proxy.url).map_err(|err| format!("invalid upstream proxy URL: {err}"))?;
    if proxy_url.scheme() != "http" {
        return Err("upstream_http_proxy supports only http:// CONNECT proxies".to_string());
    }
    let proxy_host = proxy_url
        .host_str()
        .ok_or_else(|| "upstream proxy requires a host".to_string())?;
    let proxy_port = proxy_url
        .port_or_known_default()
        .ok_or_else(|| "upstream proxy requires a port".to_string())?;
    let mut proxy_stream = connect_proxy_socket(proxy_host, proxy_port, config)?;
    let authority = format!("{target_host}:{target_port}");
    let mut request = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: keep-alive\r\n"
    );
    if let Some(header) = proxy.authorization_header.as_deref() {
        request.push_str("Proxy-Authorization: ");
        request.push_str(header);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    proxy_stream
        .write_all(request.as_bytes())
        .map_err(|err| err.to_string())?;
    proxy_stream.flush().map_err(|err| err.to_string())?;
    let response = read_proxy_response_head(&mut proxy_stream)?;
    let status = response
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .unwrap_or_default();
    if status != "200" {
        return Err(format!("upstream proxy CONNECT failed: {status}"));
    }
    proxy_stream
        .set_nodelay(true)
        .map_err(|err| err.to_string())?;
    Ok(proxy_stream)
}

fn connect_proxy_socket(
    host: &str,
    port: u16,
    config: &LocalExitConfig,
) -> Result<TcpStream, String> {
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|err| format!("upstream proxy DNS resolution failed: {err}"))?;
    let addrs = ordered_socket_addrs(addrs, config.address_family);
    let timeout = Duration::from_millis(config.connect_timeout_ms);
    let mut last_error = None;
    for addr in addrs {
        if let Err(err) = validate_public_socket_addr(addr, config.allow_private_targets) {
            last_error = Some(err);
            continue;
        }
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(err) => last_error = Some(err.to_string()),
        }
    }
    Err(last_error.unwrap_or_else(|| "upstream proxy resolved to no usable addresses".to_string()))
}

fn read_proxy_response_head(stream: &mut TcpStream) -> Result<String, String> {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        stream
            .read_exact(&mut byte)
            .map_err(|err| err.to_string())?;
        head.push(byte[0]);
        if head.len() > MAX_PROXY_RESPONSE_BYTES {
            return Err("upstream proxy response header is too large".to_string());
        }
    }
    String::from_utf8(head).map_err(|_| "upstream proxy response is not UTF-8".to_string())
}

fn validate_config(config: &LocalExitConfig) -> Result<(), String> {
    if config.schema != "elastos.browser.local-exit.config/v1" {
        return Err("unsupported local exit config schema".to_string());
    }
    validate_unix_socket_path("relay_ipc_path", &config.relay_ipc_path)?;
    if config.allowed_hosts.is_empty() {
        return Err("allowed_hosts must not be empty".to_string());
    }
    for host in &config.allowed_hosts {
        validate_allowed_host(host)?;
    }
    if config.allowed_schemes.is_empty() {
        return Err("allowed_schemes must not be empty".to_string());
    }
    for scheme in &config.allowed_schemes {
        if !matches!(scheme.as_str(), "tcp" | "tls") {
            return Err("allowed_schemes may contain only tcp or tls".to_string());
        }
    }
    if config.allowed_ports.is_empty() {
        return Err("allowed_ports must not be empty".to_string());
    }
    for port in &config.allowed_ports {
        if *port == 0 {
            return Err("allowed_ports must contain TCP ports between 1 and 65535".to_string());
        }
    }
    if config.connect_timeout_ms == 0 || config.connect_timeout_ms > 30_000 {
        return Err("connect_timeout_ms must be between 1 and 30000".to_string());
    }
    if config.buffer_bytes < 1024 || config.buffer_bytes > 1024 * 1024 {
        return Err("buffer_bytes must be between 1024 and 1048576".to_string());
    }
    if let Some(proxy) = &config.upstream_http_proxy {
        validate_upstream_http_proxy(proxy)?;
    }
    Ok(())
}

fn validate_upstream_http_proxy(proxy: &UpstreamHttpProxyConfig) -> Result<(), String> {
    let url = Url::parse(&proxy.url).map_err(|err| format!("invalid upstream proxy URL: {err}"))?;
    if url.scheme() != "http" {
        return Err("upstream_http_proxy supports only http:// CONNECT proxies".to_string());
    }
    if url.host_str().is_none() || url.port_or_known_default().is_none() {
        return Err("upstream_http_proxy requires host and port".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(
            "upstream proxy credentials must be supplied through authorization_header".to_string(),
        );
    }
    if let Some(header) = proxy.authorization_header.as_deref() {
        if header.is_empty()
            || header
                .bytes()
                .any(|byte| matches!(byte, b'\r' | b'\n' | b'\0'))
        {
            return Err("upstream proxy authorization_header is invalid".to_string());
        }
    }
    Ok(())
}

fn ordered_socket_addrs<I>(addrs: I, policy: AddressFamilyPolicy) -> Vec<SocketAddr>
where
    I: IntoIterator<Item = SocketAddr>,
{
    let addrs: Vec<SocketAddr> = addrs.into_iter().collect();
    match policy {
        AddressFamilyPolicy::System => addrs,
        AddressFamilyPolicy::Ipv4Only => addrs.into_iter().filter(SocketAddr::is_ipv4).collect(),
        AddressFamilyPolicy::Ipv6Only => addrs.into_iter().filter(SocketAddr::is_ipv6).collect(),
        AddressFamilyPolicy::PreferIpv4 => {
            let (mut preferred, other): (Vec<_>, Vec<_>) =
                addrs.into_iter().partition(SocketAddr::is_ipv4);
            preferred.extend(other);
            preferred
        }
        AddressFamilyPolicy::PreferIpv6 => {
            let (mut preferred, other): (Vec<_>, Vec<_>) =
                addrs.into_iter().partition(SocketAddr::is_ipv6);
            preferred.extend(other);
            preferred
        }
    }
}

fn address_family_label(policy: AddressFamilyPolicy) -> &'static str {
    match policy {
        AddressFamilyPolicy::System => "system",
        AddressFamilyPolicy::PreferIpv4 => "prefer_ipv4",
        AddressFamilyPolicy::PreferIpv6 => "prefer_ipv6",
        AddressFamilyPolicy::Ipv4Only => "ipv4_only",
        AddressFamilyPolicy::Ipv6Only => "ipv6_only",
    }
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

fn validate_allowed_host(host: &str) -> Result<(), String> {
    let host = host.trim();
    if host.is_empty() {
        return Err("allowed host must not be empty".to_string());
    }
    if host == "*" {
        return Ok(());
    }
    let host = host.strip_prefix("*.").unwrap_or(host);
    validate_public_host_shape(host)
}

fn validate_public_host(host: &str, allow_private: bool) -> Result<(), String> {
    validate_public_host_shape(host)?;
    if allow_private {
        return Ok(());
    }
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        validate_public_ip(ip).map_err(|_| format!("private IP blocked: {host}"))?;
    }
    Ok(())
}

fn validate_public_host_shape(host: &str) -> Result<(), String> {
    let host = host.trim().trim_matches(['[', ']']);
    if host.is_empty() {
        return Err("host must not be empty".to_string());
    }
    if host
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'\\' | b'\0'))
    {
        return Err(format!("invalid host: {host}"));
    }
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") || lower.ends_with(".local") {
        return Err(format!("private host blocked: {host}"));
    }
    Ok(())
}

fn validate_public_socket_addr(addr: SocketAddr, allow_private: bool) -> Result<(), String> {
    if allow_private {
        return Ok(());
    }
    validate_public_ip(addr.ip()).map_err(|_| format!("private resolved IP blocked: {addr}"))
}

fn validate_public_ip(ip: IpAddr) -> Result<(), ()> {
    match ip {
        IpAddr::V4(ip) => {
            if ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
            {
                Err(())
            } else {
                Ok(())
            }
        }
        IpAddr::V6(ip) => {
            if ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
            {
                Err(())
            } else {
                Ok(())
            }
        }
    }
}

fn host_allowed(host: &str, allowed_hosts: &[String]) -> bool {
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    allowed_hosts.iter().any(|allowed| {
        let allowed = allowed.to_ascii_lowercase();
        if allowed == "*" {
            return true;
        }
        if let Some(suffix) = allowed.strip_prefix("*.") {
            host.ends_with(&format!(".{suffix}"))
        } else {
            host == allowed
        }
    })
}

fn prepare_socket_path(path: &Path, replace_existing_socket: bool) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if !replace_existing_socket {
        return Err("relay_ipc_path already exists".to_string());
    }
    if !metadata.file_type().is_socket() {
        return Err("relay_ipc_path exists and is not a Unix socket".to_string());
    }
    fs::remove_file(path).map_err(|err| err.to_string())
}

fn forward_pair(runtime: UnixStream, remote: TcpStream, buffer_bytes: usize) -> Result<(), String> {
    let mut runtime_to_remote_in = runtime.try_clone().map_err(|err| err.to_string())?;
    let mut remote_to_runtime_out = runtime;
    let mut remote_to_runtime_in = remote.try_clone().map_err(|err| err.to_string())?;
    let mut runtime_to_remote_out = remote;

    let forward_to_remote = thread::spawn(move || {
        copy_stream(
            &mut runtime_to_remote_in,
            &mut runtime_to_remote_out,
            buffer_bytes,
        )
    });
    let forward_to_runtime = copy_stream(
        &mut remote_to_runtime_in,
        &mut remote_to_runtime_out,
        buffer_bytes,
    );
    let forward_to_remote = forward_to_remote
        .join()
        .map_err(|_| "browser local exit worker panicked".to_string())?;
    forward_to_runtime.and(forward_to_remote)
}

fn copy_stream<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    buffer_bytes: usize,
) -> Result<(), String> {
    let mut buffer = vec![0_u8; buffer_bytes];
    loop {
        let read = reader.read(&mut buffer).map_err(|err| err.to_string())?;
        if read == 0 {
            return Ok(());
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|err| err.to_string())?;
        writer.flush().map_err(|err| err.to_string())?;
    }
}

fn is_safe_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
}

fn default_connect_timeout_ms() -> u64 {
    5_000
}

fn default_buffer_bytes() -> usize {
    16 * 1024
}

fn default_address_family() -> AddressFamilyPolicy {
    AddressFamilyPolicy::PreferIpv4
}

fn default_allowed_schemes() -> Vec<String> {
    vec!["tcp".to_string(), "tls".to_string()]
}

fn default_allowed_ports() -> Vec<u16> {
    vec![80, 443]
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_socket_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "elastos-browser-local-exit-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp socket dir");
        dir.join(name)
    }

    fn config(relay_ipc_path: String, allowed_hosts: Vec<String>) -> LocalExitConfig {
        LocalExitConfig {
            schema: "elastos.browser.local-exit.config/v1".to_string(),
            relay_ipc_path,
            allowed_hosts,
            allowed_schemes: default_allowed_schemes(),
            allowed_ports: default_allowed_ports(),
            allow_private_targets: false,
            replace_existing_socket: false,
            connect_timeout_ms: 500,
            buffer_bytes: 1024,
            address_family: default_address_family(),
            upstream_http_proxy: None,
        }
    }

    fn relay_open(target: &str, host: &str) -> String {
        let scheme = target
            .split_once("://")
            .map(|(scheme, _)| scheme)
            .unwrap_or("tcp");
        json!({
            "schema": "elastos.exit.relay-open/v1",
            "stream_id": "stream:test:proof",
            "target": target,
            "scheme": scheme,
            "host": host,
            "principal_id": "person:local:test",
            "reason": "test local exit"
        })
        .to_string()
            + "\n"
    }

    #[test]
    fn rejects_unallowlisted_target() {
        let config = config(
            temp_socket_path("relay.sock").to_string_lossy().to_string(),
            vec!["glidefinance.io".to_string()],
        );
        let open: RelayOpen =
            serde_json::from_str(relay_open("tcp://example.com:443", "example.com").trim())
                .unwrap();

        assert!(validate_relay_open(&open, &config)
            .unwrap_err()
            .contains("not allowlisted"));
    }

    #[test]
    fn rejects_private_target_by_default() {
        let config = config(
            temp_socket_path("relay.sock").to_string_lossy().to_string(),
            vec!["127.0.0.1".to_string()],
        );
        let open: RelayOpen =
            serde_json::from_str(relay_open("tcp://127.0.0.1:80", "127.0.0.1").trim()).unwrap();

        assert!(validate_relay_open(&open, &config)
            .unwrap_err()
            .contains("private"));
    }

    #[test]
    fn wildcard_allows_public_target_but_not_private_target() {
        let config = config(
            temp_socket_path("relay.sock").to_string_lossy().to_string(),
            vec!["*".to_string()],
        );
        let public: RelayOpen =
            serde_json::from_str(relay_open("tcp://whatismyip.com:443", "whatismyip.com").trim())
                .unwrap();
        validate_relay_open(&public, &config).unwrap();

        let private: RelayOpen =
            serde_json::from_str(relay_open("tcp://127.0.0.1:80", "127.0.0.1").trim()).unwrap();
        assert!(validate_relay_open(&private, &config)
            .unwrap_err()
            .contains("private"));
    }

    #[test]
    fn address_family_policy_prefers_ipv4_without_dropping_ipv6() {
        let addrs = vec![
            "[2001:4860:4860::8888]:443".parse::<SocketAddr>().unwrap(),
            "8.8.8.8:443".parse::<SocketAddr>().unwrap(),
            "1.1.1.1:443".parse::<SocketAddr>().unwrap(),
        ];

        let ordered = ordered_socket_addrs(addrs, AddressFamilyPolicy::PreferIpv4);

        assert!(ordered[0].is_ipv4());
        assert!(ordered[1].is_ipv4());
        assert!(ordered[2].is_ipv6());
    }

    #[test]
    fn address_family_policy_can_require_ipv4_only() {
        let addrs = vec![
            "[2001:4860:4860::8888]:443".parse::<SocketAddr>().unwrap(),
            "8.8.8.8:443".parse::<SocketAddr>().unwrap(),
        ];

        let ordered = ordered_socket_addrs(addrs, AddressFamilyPolicy::Ipv4Only);

        assert_eq!(ordered.len(), 1);
        assert!(ordered[0].is_ipv4());
    }

    #[test]
    fn policy_can_limit_schemes_and_ports() {
        let mut config = config(
            temp_socket_path("relay.sock").to_string_lossy().to_string(),
            vec!["example.com".to_string()],
        );
        config.allowed_schemes = vec!["tls".to_string()];
        config.allowed_ports = vec![443];

        let allowed: RelayOpen =
            serde_json::from_str(relay_open("tls://example.com:443", "example.com").trim())
                .unwrap();
        validate_relay_open(&allowed, &config).unwrap();

        let blocked_scheme: RelayOpen =
            serde_json::from_str(relay_open("tcp://example.com:443", "example.com").trim())
                .unwrap();
        assert!(validate_relay_open(&blocked_scheme, &config)
            .unwrap_err()
            .contains("scheme is not allowlisted"));

        let blocked_port: RelayOpen =
            serde_json::from_str(relay_open("tls://example.com:8443", "example.com").trim())
                .unwrap();
        assert!(validate_relay_open(&blocked_port, &config)
            .unwrap_err()
            .contains("port is not allowlisted"));
    }

    #[test]
    fn can_forward_to_allowed_private_target_when_operator_enables_it() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let tcp_task = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4];
            stream.read_exact(&mut request).unwrap();
            assert_eq!(&request, b"ping");
            stream.write_all(b"pong").unwrap();
        });

        let mut config = config(
            temp_socket_path("relay.sock").to_string_lossy().to_string(),
            vec!["127.0.0.1".to_string()],
        );
        config.allow_private_targets = true;
        config.allowed_ports = vec![addr.port()];
        let (mut client, server) = UnixStream::pair().unwrap();
        let exit_task = thread::spawn(move || handle_session(server, &config));

        client
            .write_all(relay_open(&format!("tcp://{addr}"), "127.0.0.1").as_bytes())
            .unwrap();
        client.write_all(b"ping").unwrap();
        let mut response = [0_u8; 4];
        client.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"pong");
        drop(client);

        exit_task.join().unwrap().unwrap();
        tcp_task.join().unwrap();
    }

    #[test]
    fn can_forward_through_operator_http_connect_proxy() {
        let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let proxy_task = thread::spawn(move || {
            let (mut stream, _) = proxy.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0_u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            let head = String::from_utf8(head).unwrap();
            assert!(head.starts_with("CONNECT example.com:443 HTTP/1.1"));
            assert!(head.contains("Proxy-Authorization: Basic test-token"));
            stream
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .unwrap();
            let mut request = [0_u8; 4];
            stream.read_exact(&mut request).unwrap();
            assert_eq!(&request, b"ping");
            stream.write_all(b"pong").unwrap();
        });

        let mut config = config(
            temp_socket_path("relay.sock").to_string_lossy().to_string(),
            vec!["example.com".to_string()],
        );
        config.allow_private_targets = true;
        config.upstream_http_proxy = Some(UpstreamHttpProxyConfig {
            url: format!("http://{proxy_addr}"),
            authorization_header: Some("Basic test-token".to_string()),
        });
        let (mut client, server) = UnixStream::pair().unwrap();
        let exit_task = thread::spawn(move || handle_session(server, &config));

        client
            .write_all(relay_open("tls://example.com:443", "example.com").as_bytes())
            .unwrap();
        client.write_all(b"ping").unwrap();
        let mut response = [0_u8; 4];
        client.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"pong");
        drop(client);

        exit_task.join().unwrap().unwrap();
        proxy_task.join().unwrap();
    }

    #[test]
    fn rejects_invalid_upstream_proxy_config() {
        let bad_scheme = UpstreamHttpProxyConfig {
            url: "socks5://proxy.example:1080".to_string(),
            authorization_header: None,
        };
        assert!(validate_upstream_http_proxy(&bad_scheme)
            .unwrap_err()
            .contains("only http"));

        let bad_header = UpstreamHttpProxyConfig {
            url: "http://proxy.example:8080".to_string(),
            authorization_header: Some("Basic ok\r\nInjected: no".to_string()),
        };
        assert!(validate_upstream_http_proxy(&bad_header)
            .unwrap_err()
            .contains("authorization_header"));
    }
}
