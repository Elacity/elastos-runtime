//! ElastOS native browser proxy engine wrapper.
//!
//! This helper is the product-shaped native engine process for Chromium/CEF-like
//! browsers. It runs inside the browser engine network sandbox, starts a
//! loopback HTTP proxy for the browser, and opens every outbound stream through
//! the Runtime Exit relay Unix socket. It never dials public TCP itself.

use serde::Deserialize;
use serde_json::json;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use url::Url;

const CONFIG_ENV: &str = "ELASTOS_BROWSER_NATIVE_PROXY_ENGINE_CONFIG";
const URL_ENV: &str = "ELASTOS_BROWSER_ENGINE_URL";
const STREAM_ID_ENV: &str = "ELASTOS_BROWSER_ENGINE_STREAM_ID";
const RELAY_IPC_ENV: &str = "ELASTOS_BROWSER_ENGINE_RELAY_IPC";
const MAX_HEADER_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeProxyEngineConfig {
    schema: String,
    browser_program: String,
    #[serde(default)]
    browser_args: Vec<String>,
    #[serde(default)]
    relay_ipc_path: Option<String>,
    #[serde(default = "default_startup_grace_ms")]
    startup_grace_ms: u64,
    #[serde(default = "default_buffer_bytes")]
    buffer_bytes: usize,
}

#[derive(Debug, Clone)]
struct ProxyConfig {
    relay_ipc_path: String,
    buffer_bytes: usize,
}

#[derive(Debug)]
struct ParsedProxyRequest {
    target: String,
    host: String,
    scheme: &'static str,
    rewritten_head: Option<Vec<u8>>,
}

fn main() {
    match run(&mut io::stdout()) {
        Ok(()) => {}
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}

fn run(stdout: &mut dyn Write) -> Result<(), String> {
    let raw = std::env::var(CONFIG_ENV).map_err(|_| format!("{CONFIG_ENV} is required"))?;
    let config: NativeProxyEngineConfig =
        serde_json::from_str(&raw).map_err(|err| format!("{CONFIG_ENV} is invalid JSON: {err}"))?;
    validate_config(&config)?;

    let url = std::env::var(URL_ENV).map_err(|_| format!("{URL_ENV} is required"))?;
    let stream_id =
        std::env::var(STREAM_ID_ENV).map_err(|_| format!("{STREAM_ID_ENV} is required"))?;
    let relay_ipc_path = resolve_relay_ipc_path(&config)?;
    let proxy_url = start_proxy(ProxyConfig {
        relay_ipc_path: relay_ipc_path.clone(),
        buffer_bytes: config.buffer_bytes,
    })?;

    let mut child = Command::new(&config.browser_program)
        .args(expand_args(
            &config.browser_args,
            &url,
            &proxy_url,
            &stream_id,
        ))
        .env("ELASTOS_BROWSER_PROXY_URL", &proxy_url)
        .env("http_proxy", &proxy_url)
        .env("https_proxy", &proxy_url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("native browser launch failed: {err}"))?;

    writeln!(
        stdout,
        "{}",
        json!({
            "schema": "elastos.browser.native-proxy-engine.ready/v1",
            "proxy_url": proxy_url,
            "stream_id": stream_id,
            "relay_ipc_path": relay_ipc_path,
            "browser_pid": child.id(),
            "network_mode": "runtime_net_only",
            "direct_network": false,
        })
    )
    .map_err(|err| err.to_string())?;
    stdout.flush().map_err(|err| err.to_string())?;

    if config.startup_grace_ms > 0 {
        thread::sleep(Duration::from_millis(config.startup_grace_ms));
        if let Some(status) = child.try_wait().map_err(|err| err.to_string())? {
            return Err(format!("native browser exited during startup: {status}"));
        }
    }

    let status = child.wait().map_err(|err| err.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("native browser exited with status {status}"))
    }
}

fn validate_config(config: &NativeProxyEngineConfig) -> Result<(), String> {
    if config.schema != "elastos.browser.native-proxy-engine.config/v1" {
        return Err("unsupported native proxy engine config schema".to_string());
    }
    if config.browser_program.is_empty() || !config.browser_program.starts_with('/') {
        return Err("browser_program must be absolute".to_string());
    }
    if config.browser_program.bytes().any(|byte| byte == b'\0') {
        return Err("browser_program must not contain NUL".to_string());
    }
    if config
        .browser_args
        .iter()
        .any(|arg| arg.bytes().any(|byte| byte == b'\0'))
    {
        return Err("browser_args must not contain NUL".to_string());
    }
    if let Some(relay_ipc_path) = &config.relay_ipc_path {
        validate_unix_socket_path("relay_ipc_path", relay_ipc_path)?;
    }
    if config.startup_grace_ms > 30_000 {
        return Err("startup_grace_ms must be <= 30000".to_string());
    }
    if config.buffer_bytes < 1024 || config.buffer_bytes > 1024 * 1024 {
        return Err("buffer_bytes must be between 1024 and 1048576".to_string());
    }
    Ok(())
}

fn resolve_relay_ipc_path(config: &NativeProxyEngineConfig) -> Result<String, String> {
    let relay_ipc_path = config
        .relay_ipc_path
        .clone()
        .or_else(|| std::env::var(RELAY_IPC_ENV).ok())
        .ok_or_else(|| {
            "relay_ipc_path or ELASTOS_BROWSER_ENGINE_RELAY_IPC is required".to_string()
        })?;
    validate_unix_socket_path("relay_ipc_path", &relay_ipc_path)?;
    Ok(relay_ipc_path)
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

fn expand_args(args: &[String], url: &str, proxy_url: &str, stream_id: &str) -> Vec<String> {
    args.iter()
        .map(|arg| {
            arg.replace("{url}", url)
                .replace("{proxy_url}", proxy_url)
                .replace("{stream_id}", stream_id)
        })
        .collect()
}

fn start_proxy(config: ProxyConfig) -> Result<String, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|err| err.to_string())?;
    let addr = listener.local_addr().map_err(|err| err.to_string())?;
    let proxy_url = format!("http://{addr}");
    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let config = config.clone();
                    thread::spawn(move || {
                        let _ = handle_proxy_connection(stream, &config);
                    });
                }
                Err(_) => break,
            }
        }
    });
    Ok(proxy_url)
}

fn handle_proxy_connection(mut client: TcpStream, config: &ProxyConfig) -> Result<(), String> {
    let head = read_http_head(&mut client)?;
    let request = parse_proxy_request(&head)?;
    let mut relay = UnixStream::connect(Path::new(&config.relay_ipc_path))
        .map_err(|err| format!("Runtime Exit relay unavailable: {err}"))?;
    let mut open = serde_json::to_vec(&json!({
        "schema": "elastos.exit.relay-open/v1",
        "stream_id": format!("stream:native-proxy:{}", stable_hash(&request.target)),
        "target": request.target,
        "scheme": request.scheme,
        "host": request.host,
        "reason": "Native browser proxy request",
    }))
    .map_err(|err| err.to_string())?;
    open.push(b'\n');
    relay.write_all(&open).map_err(|err| err.to_string())?;

    if let Some(rewritten_head) = request.rewritten_head {
        relay
            .write_all(&rewritten_head)
            .map_err(|err| err.to_string())?;
    } else {
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .map_err(|err| err.to_string())?;
    }
    forward_pair(client, relay, config.buffer_bytes)
}

fn read_http_head(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        stream
            .read_exact(&mut byte)
            .map_err(|err| err.to_string())?;
        head.push(byte[0]);
        if head.len() > MAX_HEADER_BYTES {
            return Err("browser proxy request header is too large".to_string());
        }
    }
    Ok(head)
}

fn parse_proxy_request(head: &[u8]) -> Result<ParsedProxyRequest, String> {
    let text = std::str::from_utf8(head).map_err(|_| "browser proxy request is not UTF-8")?;
    let mut lines = text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| "browser proxy request is empty".to_string())?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| "browser proxy request is missing method".to_string())?;
    let target = parts
        .next()
        .ok_or_else(|| "browser proxy request is missing target".to_string())?;
    let version = parts
        .next()
        .ok_or_else(|| "browser proxy request is missing version".to_string())?;
    if parts.next().is_some() || !version.starts_with("HTTP/") {
        return Err("browser proxy request line is invalid".to_string());
    }

    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = parse_authority(target, Some(443))?;
        return Ok(ParsedProxyRequest {
            target: format!("tls://{host}:{port}"),
            host,
            scheme: "tls",
            rewritten_head: None,
        });
    }

    let url = Url::parse(target).map_err(|err| format!("browser proxy URL is invalid: {err}"))?;
    if url.scheme() != "http" {
        return Err("plain browser proxy requests must use absolute http URLs".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "browser proxy URL requires a host".to_string())?
        .to_string();
    let port = url.port_or_known_default().unwrap_or(80);
    let mut path = url.path().to_string();
    if path.is_empty() {
        path.push('/');
    }
    if let Some(query) = url.query() {
        path.push('?');
        path.push_str(query);
    }
    let mut rewritten = Vec::new();
    write!(&mut rewritten, "{method} {path} {version}\r\n").map_err(|err| err.to_string())?;
    for line in lines {
        if line.is_empty() {
            break;
        }
        let lower = line
            .split_once(':')
            .map(|(name, _)| name.trim().to_ascii_lowercase())
            .unwrap_or_default();
        if matches!(
            lower.as_str(),
            "proxy-connection" | "proxy-authorization" | "proxy-authenticate"
        ) {
            continue;
        }
        write!(&mut rewritten, "{line}\r\n").map_err(|err| err.to_string())?;
    }
    rewritten.extend_from_slice(b"\r\n");
    Ok(ParsedProxyRequest {
        target: format!("tcp://{host}:{port}"),
        host,
        scheme: "tcp",
        rewritten_head: Some(rewritten),
    })
}

fn parse_authority(value: &str, default_port: Option<u16>) -> Result<(String, u16), String> {
    if value.is_empty() || value.contains(char::is_whitespace) {
        return Err("browser proxy authority is invalid".to_string());
    }
    let (host, port) = if let Some((host, port)) = value.rsplit_once(':') {
        let port = port
            .parse::<u16>()
            .map_err(|_| "browser proxy authority port is invalid".to_string())?;
        (host.to_string(), port)
    } else {
        let Some(port) = default_port else {
            return Err("browser proxy authority requires a port".to_string());
        };
        (value.to_string(), port)
    };
    if host.is_empty() || host.bytes().any(|byte| byte == b'\0') {
        return Err("browser proxy authority host is invalid".to_string());
    }
    Ok((host, port))
}

fn forward_pair(client: TcpStream, relay: UnixStream, buffer_bytes: usize) -> Result<(), String> {
    let mut client_to_relay_in = client.try_clone().map_err(|err| err.to_string())?;
    let mut relay_to_client_out = client;
    let mut relay_to_client_in = relay.try_clone().map_err(|err| err.to_string())?;
    let mut client_to_relay_out = relay;

    let to_relay = thread::spawn(move || {
        let result = copy_stream(
            &mut client_to_relay_in,
            &mut client_to_relay_out,
            buffer_bytes,
        );
        let _ = client_to_relay_out.shutdown(Shutdown::Write);
        result
    });
    let to_client = copy_stream(
        &mut relay_to_client_in,
        &mut relay_to_client_out,
        buffer_bytes,
    );
    let _ = relay_to_client_out.shutdown(Shutdown::Write);
    to_client.and(
        to_relay
            .join()
            .map_err(|_| "native proxy worker panicked".to_string())?,
    )
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

fn stable_hash(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn default_startup_grace_ms() -> u64 {
    500
}

fn default_buffer_bytes() -> usize {
    64 * 1024
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_socket_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "elastos-native-proxy-engine-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        dir.join(name)
    }

    #[test]
    fn parses_connect_as_tls_relay_target() {
        let parsed = parse_proxy_request(
            b"CONNECT glidefinance.io:443 HTTP/1.1\r\nHost: glidefinance.io:443\r\n\r\n",
        )
        .expect("connect request");
        assert_eq!(parsed.target, "tls://glidefinance.io:443");
        assert_eq!(parsed.host, "glidefinance.io");
        assert_eq!(parsed.scheme, "tls");
        assert!(parsed.rewritten_head.is_none());
    }

    #[test]
    fn rewrites_absolute_http_requests_for_relay() {
        let parsed = parse_proxy_request(
            b"GET http://example.com/path?q=1 HTTP/1.1\r\nHost: example.com\r\nProxy-Connection: keep-alive\r\n\r\n",
        )
        .expect("http request");
        assert_eq!(parsed.target, "tcp://example.com:80");
        let head = String::from_utf8(parsed.rewritten_head.unwrap()).unwrap();
        assert!(head.starts_with("GET /path?q=1 HTTP/1.1\r\n"));
        assert!(head.contains("Host: example.com\r\n"));
        assert!(!head.to_ascii_lowercase().contains("proxy-connection"));
    }

    #[test]
    fn proxies_connect_bytes_through_runtime_exit_relay() {
        let relay_path = temp_socket_path("relay.sock");
        let listener = UnixListener::bind(&relay_path).expect("relay listener");
        let proxy_url = start_proxy(ProxyConfig {
            relay_ipc_path: relay_path.to_string_lossy().to_string(),
            buffer_bytes: 1024,
        })
        .expect("proxy");

        let relay = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("relay accept");
            let mut line = Vec::new();
            let mut byte = [0_u8; 1];
            while byte[0] != b'\n' {
                stream.read_exact(&mut byte).expect("relay read open");
                line.push(byte[0]);
            }
            let open: serde_json::Value = serde_json::from_slice(&line).expect("relay open");
            assert_eq!(open["target"], "tls://example.com:443");
            let mut payload = [0_u8; 4];
            stream.read_exact(&mut payload).expect("relay payload");
            assert_eq!(&payload, b"ping");
            stream.write_all(b"pong").expect("relay response");
        });

        let addr = proxy_url.strip_prefix("http://").unwrap();
        let mut client = TcpStream::connect(addr).expect("proxy connect");
        client
            .write_all(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n")
            .expect("connect write");
        let mut response = [0_u8; 39];
        client.read_exact(&mut response).expect("connect response");
        assert!(String::from_utf8_lossy(&response).contains("200 Connection Established"));
        client.write_all(b"ping").expect("tunnel write");
        let mut pong = [0_u8; 4];
        client.read_exact(&mut pong).expect("tunnel read");
        assert_eq!(&pong, b"pong");
        relay.join().expect("relay thread");
    }
}
