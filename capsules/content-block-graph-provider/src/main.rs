//! ElastOS Content Block Graph Provider Capsule
//!
//! Runtime-only ABI for arbitrary block/IPLD DAG repair. The provider uses the
//! local Kubo coordination file maintained by `ipfs-provider` and exchanges
//! CAR bytes through a typed graph envelope; app capsules never receive raw
//! Kubo API authority.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PROVIDER_ID: &str = "content-block-graph-provider";
const GRAPH_SCHEMA: &str = "elastos.content.block-graph/v1";
const GRAPH_ENCODING: &str = "base64-car";
const DEFAULT_MAX_GRAPH_BYTES: usize = 64 * 1024 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const LARGE_HTTP_TIMEOUT: Duration = Duration::from_secs(300);
const PROVIDER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_COORD_BYTES: u64 = 64 * 1024;

#[derive(Debug, Serialize, Deserialize)]
struct CoordFile {
    kubo_pid: u32,
    api_port: u16,
    gateway_port: u16,
    started_at: u64,
    last_used: u64,
    /// Bearer secret Kubo's API requires (`API.Authorizations`).
    api_secret: String,
}

/// Kubo API base URL and the Authorization header value it requires.
struct KuboApi {
    url: String,
    authorization: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Init {
        #[serde(default)]
        config: Value,
    },
    ExportGraph(GraphRequest),
    ImportGraph(ImportGraphRequest),
    Status {
        #[serde(default)]
        _runtime_invocation: Option<Value>,
        #[serde(default)]
        _runtime_transfer: Option<Value>,
    },
    Shutdown,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct GraphRequest {
    cid: String,
    #[serde(default)]
    schema: Option<String>,
    #[serde(default)]
    repair_graph_kind: Option<String>,
    #[serde(default)]
    availability_requirements: Value,
    #[serde(default)]
    policy: Option<String>,
    #[serde(default)]
    object_did: Option<String>,
    #[serde(default)]
    publisher_did: Option<String>,
    #[serde(default)]
    _runtime_invocation: Option<Value>,
    #[serde(default)]
    _runtime_transfer: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct ImportGraphRequest {
    cid: String,
    graph: Value,
    #[serde(default)]
    availability_policy: Option<String>,
    #[serde(default)]
    availability_requirements: Value,
    #[serde(default)]
    ensure_failure: Option<String>,
    #[serde(default)]
    object_did: Option<String>,
    #[serde(default)]
    publisher_did: Option<String>,
    #[serde(default)]
    _runtime_invocation: Option<Value>,
    #[serde(default)]
    _runtime_transfer: Option<Value>,
}

struct ContentBlockGraphProvider {
    data_dir: PathBuf,
    max_graph_bytes: usize,
}

impl Default for ContentBlockGraphProvider {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            max_graph_bytes: DEFAULT_MAX_GRAPH_BYTES,
        }
    }
}

impl ContentBlockGraphProvider {
    fn handle(&mut self, request: Request) -> Value {
        match request {
            Request::Init { config } => self.init(config),
            Request::ExportGraph(request) => self.export_graph(request),
            Request::ImportGraph(request) => self.import_graph(request),
            Request::Status { .. } => ok(json!({
                "provider": PROVIDER_ID,
                "version": PROVIDER_VERSION,
                "schema": GRAPH_SCHEMA,
                "backend": self.backend_status(),
                "operations": ["export_graph", "import_graph", "status"],
                "status": if self.kubo_api().is_ok() {
                    "ready"
                } else {
                    "backend_not_configured"
                }
            })),
            Request::Shutdown => ok(json!({ "provider": PROVIDER_ID })),
        }
    }

    fn init(&mut self, config: Value) -> Value {
        let extra = config
            .get("extra")
            .filter(|value| !value.is_null())
            .unwrap_or(&config);
        if let Some(base_path) = config
            .get("base_path")
            .and_then(Value::as_str)
            .or_else(|| extra.get("data_dir").and_then(Value::as_str))
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let path = PathBuf::from(base_path);
            if !path.is_absolute() {
                return error(
                    "invalid_config",
                    "content block graph data_dir must be absolute",
                );
            }
            self.data_dir = path;
        }
        if let Some(max_graph_bytes) = extra.get("max_graph_bytes").and_then(Value::as_u64) {
            let max_graph_bytes = usize::try_from(max_graph_bytes).unwrap_or(usize::MAX);
            if max_graph_bytes == 0 || max_graph_bytes > DEFAULT_MAX_GRAPH_BYTES {
                return error(
                    "invalid_config",
                    "max_graph_bytes must be between 1 and 67108864",
                );
            }
            self.max_graph_bytes = max_graph_bytes;
        }

        ok(json!({
            "provider": PROVIDER_ID,
            "protocol_version": "1.0",
            "version": PROVIDER_VERSION,
            "schema": GRAPH_SCHEMA,
            "backend": self.backend_status(),
            "status": if self.kubo_api().is_ok() {
                "ready"
            } else {
                "backend_not_configured"
            }
        }))
    }

    fn export_graph(&self, request: GraphRequest) -> Value {
        if let Err(message) = validate_cid(&request.cid) {
            return error("invalid_request", &message);
        }
        if let Some(schema) = request.schema.as_deref() {
            if schema != GRAPH_SCHEMA {
                return error(
                    "unsupported_schema",
                    "export_graph requires block graph schema v1",
                );
            }
        }
        if request
            .repair_graph_kind
            .as_deref()
            .is_some_and(|kind| !matches!(kind, "ipld_dag" | "block_dag" | "dag" | "arbitrary_dag"))
        {
            return error(
                "unsupported_repair_graph",
                "export_graph only handles arbitrary block/IPLD DAG repair",
            );
        }
        let car = match self.kubo_dag_export(&request.cid) {
            Ok(car) => car,
            Err(message) => return error("export_failed", &message),
        };
        if car.len() > self.max_graph_bytes {
            return error(
                "graph_too_large",
                "exported graph exceeds content-block-graph-provider max_graph_bytes",
            );
        }
        ok(json!({
            "graph": {
                "schema": GRAPH_SCHEMA,
                "root_cid": request.cid,
                "kind": "ipld_dag",
                "encoding": GRAPH_ENCODING,
                "car": BASE64.encode(&car),
                "bytes": car.len(),
                "backend": "kubo_dag_car",
                "exported_at": now_unix_secs(),
                "max_graph_bytes": self.max_graph_bytes,
            }
        }))
    }

    fn import_graph(&self, request: ImportGraphRequest) -> Value {
        if let Err(message) = validate_cid(&request.cid) {
            return error("invalid_request", &message);
        }
        if request.graph.get("schema").and_then(Value::as_str) != Some(GRAPH_SCHEMA) {
            return error(
                "unsupported_schema",
                "import_graph requires block graph schema v1",
            );
        }
        if request.graph.get("root_cid").and_then(Value::as_str) != Some(request.cid.as_str()) {
            return error("cid_mismatch", "graph root_cid must match import cid");
        }
        if request.graph.get("encoding").and_then(Value::as_str) != Some(GRAPH_ENCODING) {
            return error(
                "unsupported_encoding",
                "import_graph requires base64-car encoding",
            );
        }
        let car = match request.graph.get("car").and_then(Value::as_str) {
            Some(car) => match BASE64.decode(car) {
                Ok(bytes) => bytes,
                Err(err) => {
                    return error("invalid_car", &format!("invalid graph CAR base64: {err}"))
                }
            },
            None => return error("invalid_graph", "import_graph requires graph.car"),
        };
        if car.is_empty() {
            return error("invalid_graph", "import_graph requires non-empty CAR bytes");
        }
        if car.len() > self.max_graph_bytes {
            return error(
                "graph_too_large",
                "import graph exceeds content-block-graph-provider max_graph_bytes",
            );
        }
        if let Err(message) = self.kubo_dag_import_and_pin(&request.cid, &car) {
            return error("import_failed", &message);
        }
        ok(json!({
            "cid": request.cid,
            "availability": {
                "status": "local_pinned",
                "provider": PROVIDER_ID,
                "policy": request
                    .availability_policy
                    .unwrap_or_else(|| "carrier_block_graph_import".to_string()),
                "replicas": 1,
                "peer_selection": {
                    "mode": "single_local",
                    "live_multi_peer_proof": false
                },
                "quota": {
                    "policy": "provider_local",
                    "enforced": false
                },
                "repair_worker": {
                    "scheduled": false,
                    "status": "healthy",
                    "worker": PROVIDER_ID
                },
                "repair_graph": {
                    "schema": "elastos.content.repair-graph/v1",
                    "policy": "carrier_provider_block_graph_repair",
                    "requested_kind": "ipld_dag",
                    "status": "block_graph_provider_imported",
                    "backend": "kubo_dag_car"
                }
            },
            "import": {
                "schema": GRAPH_SCHEMA,
                "verified_cid": true,
                "bytes": car.len(),
                "backend": "kubo_dag_import"
            }
        }))
    }

    fn backend_status(&self) -> Value {
        match read_coord_file(&self.data_dir) {
            Some(coord) => json!({
                "kind": "kubo_coord",
                "configured": true,
                "coord_file": coord_file_path(&self.data_dir),
                "kubo_pid": coord.kubo_pid,
                "api_port": coord.api_port,
                "gateway_port": coord.gateway_port,
                "started_at": coord.started_at,
                "last_used": coord.last_used,
                "max_graph_bytes": self.max_graph_bytes,
            }),
            None => json!({
                "kind": "kubo_coord",
                "configured": false,
                "coord_file": coord_file_path(&self.data_dir),
                "max_graph_bytes": self.max_graph_bytes,
            }),
        }
    }

    fn kubo_api(&self) -> Result<KuboApi, String> {
        let coord = read_coord_file(&self.data_dir).ok_or_else(|| {
            format!(
                "Kubo coord file missing at {}; start ipfs-provider before block graph repair",
                coord_file_path(&self.data_dir).display()
            )
        })?;
        if coord.api_port == 0 {
            return Err("Kubo coord file has no API port".to_string());
        }
        Ok(KuboApi {
            url: format!("http://127.0.0.1:{}", coord.api_port),
            authorization: format!("Bearer {}", coord.api_secret),
        })
    }

    fn kubo_dag_export(&self, cid: &str) -> Result<Vec<u8>, String> {
        let api = self.kubo_api()?;
        let url = format!("{}/api/v0/dag/export?arg={}", api.url, cid);
        let response = ureq::post(&url)
            .set("Authorization", &api.authorization)
            .timeout(LARGE_HTTP_TIMEOUT)
            .call()
            .map_err(|err| format!("kubo dag export failed: {err}"))?;
        if response.status() != 200 {
            return Err(format!(
                "kubo dag export returned HTTP {}",
                response.status()
            ));
        }
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take((self.max_graph_bytes as u64).saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|err| format!("kubo dag export read failed: {err}"))?;
        update_coord_last_used(&self.data_dir);
        Ok(bytes)
    }

    fn kubo_dag_import_and_pin(&self, cid: &str, car: &[u8]) -> Result<(), String> {
        let api = self.kubo_api()?;
        let boundary = format!("----elastos-block-graph-{}", now_unix_secs());
        let mut body = Vec::new();
        write!(body, "--{boundary}\r\n").unwrap();
        write!(
            body,
            "Content-Disposition: form-data; name=\"file\"; filename=\"graph.car\"\r\n"
        )
        .unwrap();
        write!(body, "Content-Type: application/vnd.ipld.car\r\n\r\n").unwrap();
        body.extend_from_slice(car);
        write!(body, "\r\n--{boundary}--\r\n").unwrap();

        let import_url = format!("{}/api/v0/dag/import?pin-roots=true", api.url);
        let response = ureq::post(&import_url)
            .set("Authorization", &api.authorization)
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .timeout(LARGE_HTTP_TIMEOUT)
            .send_bytes(&body)
            .map_err(|err| format!("kubo dag import failed: {err}"))?;
        if response.status() != 200 {
            return Err(format!(
                "kubo dag import returned HTTP {}",
                response.status()
            ));
        }

        let pin_url = format!("{}/api/v0/pin/add?arg={cid}", api.url);
        let pin_response = ureq::post(&pin_url)
            .set("Authorization", &api.authorization)
            .timeout(HTTP_TIMEOUT)
            .call()
            .map_err(|err| format!("kubo pin after dag import failed: {err}"))?;
        if pin_response.status() != 200 {
            return Err(format!(
                "kubo pin after dag import returned HTTP {}",
                pin_response.status()
            ));
        }
        update_coord_last_used(&self.data_dir);
        Ok(())
    }
}

fn validate_cid(cid: &str) -> Result<(), String> {
    let value = cid.trim();
    if value.is_empty() {
        return Err("cid must not be empty".to_string());
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err("cid contains unsupported characters".to_string());
    }
    Ok(())
}

fn ok(data: Value) -> Value {
    json!({
        "status": "ok",
        "data": data,
    })
}

fn error(code: &str, message: &str) -> Value {
    json!({
        "status": "error",
        "code": code,
        "message": message,
    })
}

fn default_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ELASTOS_DATA_DIR") {
        PathBuf::from(dir)
    } else if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        PathBuf::from(dir).join("elastos")
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".local/share/elastos")
    } else {
        PathBuf::from("/tmp/elastos")
    }
}

fn coord_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ipfs-coords.json")
}

// ipfs-coords.json carries the Kubo API secret. ipfs-provider writes it as a
// new owner-only file renamed into place; it is trusted only when it is a
// regular file owned by this user with mode 0600.
fn read_coord_file(data_dir: &Path) -> Option<CoordFile> {
    #[cfg(unix)]
    let owner = unsafe { libc::geteuid() };
    #[cfg(not(unix))]
    let owner = 0;
    read_private_coord(&coord_file_path(data_dir), owner)
}

fn read_private_coord(path: &Path, owner: u32) -> Option<CoordFile> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() {
        return None;
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let private =
            |m: &fs::Metadata| m.is_file() && m.mode() & 0o777 == 0o600 && m.uid() == owner;
        if !private(&metadata) {
            return None;
        }
        let file = options.custom_flags(libc::O_NOFOLLOW).open(path).ok()?;
        if !private(&file.metadata().ok()?) {
            return None;
        }
        file
    };
    #[cfg(not(unix))]
    let file = {
        let _ = owner;
        options.open(path).ok()?
    };
    let mut data = Vec::new();
    file.take(MAX_COORD_BYTES + 1).read_to_end(&mut data).ok()?;
    if data.len() as u64 > MAX_COORD_BYTES {
        return None;
    }
    let coord: CoordFile = serde_json::from_slice(&data).ok()?;
    let secret_ok = coord.api_secret.len() == 64
        && coord
            .api_secret
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    secret_ok.then_some(coord)
}

/// Writes a new owner-only file beside `path`, syncs it and renames it over
/// `path`. Neither the temporary name nor an existing link is followed.
fn write_private_file_atomically(path: &Path, data: &[u8]) -> io::Result<()> {
    let refused = |message: &str| io::Error::new(io::ErrorKind::InvalidInput, message.to_owned());
    let parent = path
        .parent()
        .ok_or_else(|| refused("private file path has no parent"))?;
    if !fs::symlink_metadata(parent)?.file_type().is_dir() {
        return Err(refused("private file parent must be a real directory"));
    }
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_file() {
            return Err(refused("private file target must be a regular file"));
        }
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| refused("private file name is invalid"))?;
    let mut nonce = [0u8; 16];
    getrandom::getrandom(&mut nonce)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
    let nonce: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let temporary = parent.join(format!(".{name}.{nonce}.tmp"));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(data)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn update_coord_last_used(data_dir: &Path) {
    let Some(mut coord) = read_coord_file(data_dir) else {
        return;
    };
    coord.last_used = now_unix_secs();
    if let Ok(json) = serde_json::to_vec_pretty(&coord) {
        let _ = write_private_file_atomically(&coord_file_path(data_dir), &json);
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn main() {
    eprintln!("{PROVIDER_ID}: starting v{PROVIDER_VERSION}");
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut provider = ContentBlockGraphProvider::default();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                eprintln!("{PROVIDER_ID} read error: {err}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }

        let request = match serde_json::from_str::<Request>(&line) {
            Ok(request) => request,
            Err(err) => {
                let response = error("invalid_request", &err.to_string());
                writeln!(stdout, "{}", serde_json::to_string(&response).unwrap()).unwrap();
                stdout.flush().unwrap();
                continue;
            }
        };
        let is_shutdown = matches!(request, Request::Shutdown);
        let response = provider.handle(request);
        writeln!(stdout, "{}", serde_json::to_string(&response).unwrap()).unwrap();
        stdout.flush().unwrap();
        if is_shutdown {
            break;
        }
    }

    eprintln!("{PROVIDER_ID}: exiting");
}
#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "elastos-block-graph-provider-test-{name}-{}",
            now_unix_secs()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn cid_validation_rejects_empty_and_path_like_values() {
        assert!(
            validate_cid("bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi").is_ok()
        );
        assert!(validate_cid("").is_err());
        assert!(validate_cid("../secret").is_err());
        assert!(validate_cid("bafy cid").is_err());
    }

    const FIXTURE_SECRET: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn write_fixture_coord(data_dir: &Path, api_port: u16) {
        let coord = CoordFile {
            kubo_pid: std::process::id(),
            api_port,
            gateway_port: 0,
            started_at: 1,
            last_used: 1,
            api_secret: FIXTURE_SECRET.to_string(),
        };
        write_private_file_atomically(
            &coord_file_path(data_dir),
            &serde_json::to_vec(&coord).unwrap(),
        )
        .unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn coord_reader_trusts_only_private_current_coordinates() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let data_dir = temp_dir("private-coord");
        let path = coord_file_path(&data_dir);
        let previous = unsafe { libc::umask(0o022) };
        write_fixture_coord(&data_dir, 5001);
        unsafe { libc::umask(previous) };
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            read_coord_file(&data_dir).unwrap().api_secret,
            FIXTURE_SECRET
        );
        // Another owner's file is not trusted even when it is private.
        let owner = unsafe { libc::geteuid() };
        assert!(read_private_coord(&path, owner + 1).is_none());

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_coord_file(&data_dir).is_none());
        // A refresh never rewrites coordinates it does not trust.
        update_coord_last_used(&data_dir);
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o644);

        let target = data_dir.join("elsewhere.json");
        fs::rename(&path, &target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(read_coord_file(&data_dir).is_none());
        assert!(write_private_file_atomically(&path, b"{}").is_err());
        fs::remove_file(&path).unwrap();

        // Coordinates from before the API secret existed name no usable backend.
        write_private_file_atomically(
            &path,
            br#"{"kubo_pid":1,"api_port":5001,"gateway_port":0,"started_at":1,"last_used":1}"#,
        )
        .unwrap();
        assert!(read_coord_file(&data_dir).is_none());
        let _ = fs::remove_dir_all(&data_dir);
    }

    #[test]
    #[cfg(unix)]
    fn export_graph_sends_the_kubo_api_secret_and_keeps_it_on_refresh() {
        use std::io::{BufRead as _, BufReader};
        use std::os::unix::fs::MetadataExt;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(_) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(err) => panic!("export never reached the Kubo API fixture: {err}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            let mut headers = Vec::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                headers.push(line.trim_end().to_ascii_lowercase());
            }
            let authorized = headers.contains(&format!("authorization: bearer {FIXTURE_SECRET}"));
            let response: &[u8] = if authorized {
                b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\ncar"
            } else {
                b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            };
            (&socket).write_all(response).unwrap();
            headers
        });
        let data_dir = temp_dir("export-auth");
        write_fixture_coord(&data_dir, port);
        let provider = ContentBlockGraphProvider {
            data_dir: data_dir.clone(),
            ..Default::default()
        };
        let response = provider.export_graph(GraphRequest {
            cid: "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi".to_string(),
            schema: Some(GRAPH_SCHEMA.to_string()),
            repair_graph_kind: Some("ipld_dag".to_string()),
            availability_requirements: Value::Null,
            policy: None,
            object_did: None,
            publisher_did: None,
            _runtime_invocation: None,
            _runtime_transfer: None,
        });
        let headers = server.join().unwrap();
        assert_eq!(response["status"], "ok", "{response} {headers:?}");
        assert_eq!(response["data"]["graph"]["car"], BASE64.encode(b"car"));
        let coord = read_coord_file(&data_dir).unwrap();
        assert!(coord.last_used > 1);
        assert_eq!(coord.api_secret, FIXTURE_SECRET);
        assert_eq!(
            fs::metadata(coord_file_path(&data_dir)).unwrap().mode() & 0o777,
            0o600
        );
        let _ = fs::remove_dir_all(&data_dir);
    }

    #[test]
    fn status_reports_missing_kubo_coord_as_not_configured() {
        let mut provider = ContentBlockGraphProvider::default();
        let data_dir = temp_dir("missing-coord");
        let response = provider.handle(Request::Init {
            config: json!({
                "base_path": data_dir,
            }),
        });
        assert_eq!(response["status"], "ok");

        let response = provider.handle(Request::Status {
            _runtime_invocation: None,
            _runtime_transfer: None,
        });
        assert_eq!(response["data"]["status"], "backend_not_configured");
        assert_eq!(response["data"]["backend"]["configured"], false);
    }

    #[test]
    fn export_graph_fails_closed_without_kubo_coord() {
        let provider = ContentBlockGraphProvider {
            data_dir: temp_dir("export-no-coord"),
            ..Default::default()
        };
        let response = provider.export_graph(GraphRequest {
            cid: "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi".to_string(),
            schema: Some(GRAPH_SCHEMA.to_string()),
            repair_graph_kind: Some("ipld_dag".to_string()),
            availability_requirements: Value::Null,
            policy: None,
            object_did: None,
            publisher_did: None,
            _runtime_invocation: None,
            _runtime_transfer: None,
        });

        assert_eq!(response["status"], "error");
        assert_eq!(response["code"], "export_failed");
        assert!(response["message"]
            .as_str()
            .unwrap()
            .contains("Kubo coord file missing"));
    }

    #[test]
    fn import_graph_rejects_wrong_schema_and_root() {
        let provider = ContentBlockGraphProvider {
            data_dir: temp_dir("import-validate"),
            ..Default::default()
        };
        let cid = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi".to_string();
        let response = provider.import_graph(ImportGraphRequest {
            cid: cid.clone(),
            graph: json!({
                "schema": "wrong",
                "root_cid": cid,
            }),
            availability_policy: None,
            availability_requirements: Value::Null,
            ensure_failure: None,
            object_did: None,
            publisher_did: None,
            _runtime_invocation: None,
            _runtime_transfer: None,
        });
        assert_eq!(response["code"], "unsupported_schema");

        let response = provider.import_graph(ImportGraphRequest {
            cid,
            graph: json!({
                "schema": GRAPH_SCHEMA,
                "root_cid": "bafywrong",
                "encoding": GRAPH_ENCODING,
                "car": BASE64.encode(b"car"),
            }),
            availability_policy: None,
            availability_requirements: Value::Null,
            ensure_failure: None,
            object_did: None,
            publisher_did: None,
            _runtime_invocation: None,
            _runtime_transfer: None,
        });
        assert_eq!(response["code"], "cid_mismatch");
    }
}
