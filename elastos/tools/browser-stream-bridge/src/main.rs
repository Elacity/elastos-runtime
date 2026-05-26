//! ElastOS Browser Engine stream bridge.
//!
//! This helper is intentionally narrow: it accepts one browser-engine
//! connection on a private Unix socket and forwards bytes to a Runtime-owned
//! Unix stream socket. It never opens TCP sockets, performs DNS, or reaches the
//! host internet.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;

const CONFIG_ENV: &str = "ELASTOS_BROWSER_STREAM_BRIDGE_CONFIG";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BridgeConfig {
    schema: String,
    stream_id: String,
    target: String,
    adapter_ipc_path: String,
    runtime_stream_path: String,
    network_mode: NetworkMode,
    direct_network: bool,
    #[serde(default)]
    replace_existing_socket: bool,
    #[serde(default = "default_buffer_bytes")]
    buffer_bytes: usize,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum NetworkMode {
    RuntimeNetOnly,
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
    let config: BridgeConfig =
        serde_json::from_str(&raw).map_err(|err| format!("{CONFIG_ENV} is invalid JSON: {err}"))?;
    run_bridge(&config, stdout)
}

fn run_bridge(config: &BridgeConfig, stdout: &mut dyn Write) -> Result<(), String> {
    validate_config(config)?;
    let adapter_path = Path::new(&config.adapter_ipc_path);
    let runtime_path = Path::new(&config.runtime_stream_path);
    prepare_adapter_socket_path(adapter_path, config.replace_existing_socket)?;
    let listener = UnixListener::bind(adapter_path).map_err(|err| err.to_string())?;
    let _socket_guard = SocketFileGuard::new(adapter_path);

    writeln!(
        stdout,
        "{}",
        json!({
            "schema": "elastos.browser.stream-bridge.ready/v1",
            "stream_id": config.stream_id,
            "target": config.target,
            "adapter_ipc_path": config.adapter_ipc_path,
            "runtime_stream_path": config.runtime_stream_path,
            "network_mode": "runtime_net_only",
            "direct_network": false,
        })
    )
    .map_err(|err| err.to_string())?;
    stdout.flush().map_err(|err| err.to_string())?;

    let (engine_stream, _) = listener.accept().map_err(|err| err.to_string())?;
    let runtime_stream = UnixStream::connect(runtime_path).map_err(|err| err.to_string())?;
    forward_pair(engine_stream, runtime_stream, config.buffer_bytes)
}

fn validate_config(config: &BridgeConfig) -> Result<(), String> {
    if config.schema != "elastos.browser.stream-bridge.config/v1" {
        return Err("unsupported browser stream bridge config schema".to_string());
    }
    if !is_safe_id(&config.stream_id) {
        return Err("stream_id must be a safe identifier".to_string());
    }
    if !config.target.starts_with("tls://") && !config.target.starts_with("tcp://") {
        return Err("target must use tls or tcp".to_string());
    }
    if config.network_mode != NetworkMode::RuntimeNetOnly {
        return Err("browser stream bridge must be runtime_net_only".to_string());
    }
    if config.direct_network {
        return Err("browser stream bridge must not grant direct network".to_string());
    }
    validate_unix_socket_path("adapter_ipc_path", &config.adapter_ipc_path)?;
    validate_unix_socket_path("runtime_stream_path", &config.runtime_stream_path)?;
    if config.adapter_ipc_path == config.runtime_stream_path {
        return Err("adapter_ipc_path and runtime_stream_path must be distinct".to_string());
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

fn prepare_adapter_socket_path(path: &Path, replace_existing_socket: bool) -> Result<(), String> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if !replace_existing_socket {
        return Err("adapter_ipc_path already exists".to_string());
    }
    if !metadata.file_type().is_socket() {
        return Err("adapter_ipc_path exists and is not a Unix socket".to_string());
    }
    fs::remove_file(path).map_err(|err| err.to_string())
}

fn forward_pair(
    engine: UnixStream,
    runtime: UnixStream,
    buffer_bytes: usize,
) -> Result<(), String> {
    let mut engine_to_runtime_in = engine.try_clone().map_err(|err| err.to_string())?;
    let mut runtime_to_engine_out = engine;
    let mut runtime_to_engine_in = runtime.try_clone().map_err(|err| err.to_string())?;
    let mut engine_to_runtime_out = runtime;

    let forward_to_runtime = thread::spawn(move || {
        copy_stream(
            &mut engine_to_runtime_in,
            &mut engine_to_runtime_out,
            buffer_bytes,
        )
    });
    let forward_to_engine = copy_stream(
        &mut runtime_to_engine_in,
        &mut runtime_to_engine_out,
        buffer_bytes,
    );
    let forward_to_runtime = forward_to_runtime
        .join()
        .map_err(|_| "browser stream bridge worker panicked".to_string())?;
    forward_to_engine.and(forward_to_runtime)
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

fn default_buffer_bytes() -> usize {
    16 * 1024
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
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    fn config(adapter_ipc_path: String, runtime_stream_path: String) -> BridgeConfig {
        BridgeConfig {
            schema: "elastos.browser.stream-bridge.config/v1".to_string(),
            stream_id: "stream:proof:test".to_string(),
            target: "tls://glidefinance.io:443".to_string(),
            adapter_ipc_path,
            runtime_stream_path,
            network_mode: NetworkMode::RuntimeNetOnly,
            direct_network: false,
            replace_existing_socket: false,
            buffer_bytes: 1024,
        }
    }

    fn temp_socket_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "elastos-browser-stream-bridge-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("temp socket dir");
        path
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
    fn validates_runtime_only_unix_bridge_config() {
        let dir = temp_socket_dir();
        let config = config(
            dir.join("adapter.sock").display().to_string(),
            dir.join("runtime.sock").display().to_string(),
        );
        assert!(validate_config(&config).is_ok());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_direct_network_or_bad_paths() {
        let dir = temp_socket_dir();
        let mut direct = config(
            dir.join("adapter.sock").display().to_string(),
            dir.join("runtime.sock").display().to_string(),
        );
        direct.direct_network = true;
        assert!(validate_config(&direct)
            .unwrap_err()
            .contains("direct network"));

        let mut bad_path = direct;
        bad_path.direct_network = false;
        bad_path.adapter_ipc_path = "tcp://127.0.0.1:9999".to_string();
        assert!(validate_config(&bad_path)
            .unwrap_err()
            .contains("absolute Unix socket path"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn forwards_bytes_between_private_unix_sockets() {
        let dir = temp_socket_dir();
        let adapter_path = dir.join("adapter.sock");
        let runtime_path = dir.join("runtime.sock");
        let runtime_listener = UnixListener::bind(&runtime_path).expect("runtime listener");
        let config = config(
            adapter_path.display().to_string(),
            runtime_path.display().to_string(),
        );

        let bridge_handle = thread::spawn(move || {
            let mut ready = Vec::new();
            run_bridge(&config, &mut ready).expect("bridge run");
            String::from_utf8(ready).expect("ready output")
        });

        wait_for_socket(&adapter_path);
        let runtime_handle = thread::spawn(move || {
            let (mut runtime, _) = runtime_listener.accept().expect("runtime accept");
            let mut request = [0_u8; 4];
            runtime.read_exact(&mut request).expect("runtime read");
            assert_eq!(&request, b"ping");
            runtime.write_all(b"pong").expect("runtime write");
        });

        let mut engine = UnixStream::connect(&adapter_path).expect("engine connect");
        engine.write_all(b"ping").expect("engine write");
        let mut response = [0_u8; 4];
        engine.read_exact(&mut response).expect("engine read");
        assert_eq!(&response, b"pong");
        drop(engine);

        runtime_handle.join().expect("runtime thread");
        let ready = bridge_handle.join().expect("bridge thread");
        assert!(ready.contains("elastos.browser.stream-bridge.ready/v1"));
        let _ = fs::remove_dir_all(dir);
    }
}
