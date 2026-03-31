//! ElastOS ipfs-provider Capsule
//!
//! Manages a Kubo daemon subprocess for IPFS operations.
//! Kubo is persistent across CLI invocations (shared via coord file).
//! Wire protocol: line-delimited JSON over stdin/stdout.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HEALTH_POLL_INTERVAL: Duration = Duration::from_secs(2);
const KUBO_STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
const IDLE_TIMEOUT_SECS: u64 = 600; // 10 minutes
const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(60);
const LOCKFILE_POLL_INTERVAL: Duration = Duration::from_millis(500);
const LOCKFILE_POLL_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const LARGE_HTTP_TIMEOUT: Duration = Duration::from_secs(300);

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

// ── Protocol types ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Init {
        #[serde(default)]
        config: serde_json::Value,
    },
    AddBytes {
        data: String, // base64
        #[serde(default = "default_filename")]
        filename: String,
        #[serde(default = "default_true")]
        pin: bool,
    },
    AddPath {
        path: String, // absolute filesystem path
        #[serde(default = "default_true")]
        pin: bool,
    },
    AddDirectory {
        files: Vec<DirFile>,
        #[serde(default = "default_true")]
        pin: bool,
    },
    Cat {
        cid: String,
        #[serde(default)]
        path: Option<String>,
    },
    CatToPath {
        cid: String,
        #[serde(default)]
        path: Option<String>,
        dest: String,
    },
    GetBytes {
        cid: String,
        #[serde(default)]
        path: Option<String>,
    },
    Ls {
        cid: String,
    },
    DownloadDirectory {
        cid: String,
        dest: String,
    },
    Pin {
        cid: String,
    },
    Unpin {
        cid: String,
    },
    Health,
    Status,
    Shutdown,
}

#[derive(Debug, Deserialize)]
struct DirFile {
    path: String,
    data: String, // base64
}

fn default_filename() -> String {
    "file".to_string()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Response {
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
    },
    Error {
        code: String,
        message: String,
    },
}

impl Response {
    fn ok(data: serde_json::Value) -> Self {
        Response::Ok { data: Some(data) }
    }

    fn ok_empty() -> Self {
        Response::Ok { data: None }
    }

    fn error(code: &str, message: &str) -> Self {
        Response::Error {
            code: code.to_string(),
            message: message.to_string(),
        }
    }
}

// ── State machine ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum KuboState {
    Cold,
    Starting,
    Ready,
    Error,
}

// ── Coord file ──────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct CoordFile {
    kubo_pid: u32,
    api_port: u16,
    gateway_port: u16,
    started_at: u64,
    last_used: u64,
}

// ── Provider ────────────────────────────────────────────────────────

struct IpfsProvider {
    state: KuboState,
    api_port: u16,
    gateway_port: u16,
    kubo_binary: Option<PathBuf>,
    kubo_child: Option<Child>,
    data_dir: PathBuf,
    repo_dir: PathBuf,
}

impl IpfsProvider {
    fn new() -> Self {
        let data_dir = data_dir();
        let repo_dir = data_dir.join("ipfs-repo");
        Self {
            state: KuboState::Cold,
            api_port: 0,
            gateway_port: 0,
            kubo_binary: None,
            kubo_child: None,
            data_dir,
            repo_dir,
        }
    }

    fn api_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.api_port)
    }

    fn root_cid(arg: &str) -> &str {
        arg.split('/').next().unwrap_or(arg)
    }

    fn kubo_cat_bytes(&mut self, arg: &str, timeout: Duration) -> Result<Vec<u8>, String> {
        let url = format!("{}/api/v0/cat?arg={}", self.api_url(), arg);
        match ureq::post(&url).timeout(timeout).call() {
            Ok(resp) if resp.status() == 200 => {
                let mut bytes = Vec::new();
                resp.into_reader()
                    .read_to_end(&mut bytes)
                    .map_err(|e| format!("kubo cat -> {}", e))?;
                update_coord_last_used(&self.data_dir);
                Ok(bytes)
            }
            Ok(resp) => Err(format!("kubo cat -> HTTP {} for {}", resp.status(), arg)),
            Err(e) => Err(format!("kubo cat -> {} for {}", e, arg)),
        }
    }

    fn kubo_prefetch_cid(&mut self, cid: &str) -> Result<(), String> {
        let url = format!("{}/api/v0/pin/add?arg={}", self.api_url(), cid);
        match ureq::post(&url).timeout(LARGE_HTTP_TIMEOUT).call() {
            Ok(resp) if resp.status() == 200 => {
                update_coord_last_used(&self.data_dir);
                Ok(())
            }
            Ok(resp) => Err(format!("kubo pin -> HTTP {} for {}", resp.status(), cid)),
            Err(e) => Err(format!("kubo pin -> {} for {}", e, cid)),
        }
    }

    fn fetch_bytes(&mut self, arg: &str) -> Result<Vec<u8>, String> {
        let mut failures = Vec::new();

        if self.state == KuboState::Ready || self.ensure_kubo().is_ok() {
            match self.kubo_cat_bytes(arg, LARGE_HTTP_TIMEOUT) {
                Ok(bytes) => return Ok(bytes),
                Err(err) => failures.push(err),
            }

            let root_cid = Self::root_cid(arg);
            match self.kubo_prefetch_cid(root_cid) {
                Ok(()) => match self.kubo_cat_bytes(arg, LARGE_HTTP_TIMEOUT) {
                    Ok(bytes) => return Ok(bytes),
                    Err(err) => failures.push(err),
                },
                Err(err) => failures.push(err),
            }
        }

        match self.fetch_from_local_gateway_with_timeout(arg, LARGE_HTTP_TIMEOUT) {
            Ok(bytes) => Ok(bytes),
            Err(err) => {
                failures.push(err);
                Err(failures.join("; "))
            }
        }
    }

    fn handle(&mut self, req: Request) -> Response {
        match req {
            Request::Init { config } => self.init(config),
            Request::AddBytes {
                data,
                filename,
                pin,
            } => self.add_bytes(&data, &filename, pin),
            Request::AddPath { path, pin } => self.add_path(&path, pin),
            Request::AddDirectory { files, pin } => self.add_directory(files, pin),
            Request::Cat { cid, path } => self.cat(&cid, path.as_deref()),
            Request::CatToPath { cid, path, dest } => {
                self.cat_to_path(&cid, path.as_deref(), &dest)
            }
            Request::GetBytes { cid, path } => self.cat(&cid, path.as_deref()),
            Request::Ls { cid } => self.ls(&cid),
            Request::DownloadDirectory { cid, dest } => self.download_directory(&cid, &dest),
            Request::Pin { cid } => self.pin(&cid),
            Request::Unpin { cid } => self.unpin(&cid),
            Request::Health => self.health(),
            Request::Status => self.status(),
            Request::Shutdown => self.shutdown(),
        }
    }

    // ── Init ────────────────────────────────────────────────────────

    fn init(&mut self, config: serde_json::Value) -> Response {
        let extra = config.get("extra").unwrap_or(&config);

        if extra.get("gateways").is_some() || std::env::var("ELASTOS_IPFS_GATEWAYS").is_ok() {
            eprintln!("ipfs-provider: ignoring gateway override; provider is local-IPFS only");
        }

        // Find Kubo binary
        match find_kubo_binary() {
            Some(path) => {
                eprintln!("ipfs-provider: found kubo at {}", path.display());
                self.kubo_binary = Some(path);
            }
            None => {
                eprintln!("ipfs-provider: kubo not found. Run: elastos setup --with kubo");
            }
        }

        // Check coord file for running Kubo instance
        if let Some(coord) = read_coord_file(&self.data_dir) {
            if is_pid_alive(coord.kubo_pid) {
                eprintln!(
                    "ipfs-provider: reusing existing Kubo (pid={}, api={})",
                    coord.kubo_pid, coord.api_port
                );
                self.api_port = coord.api_port;
                self.gateway_port = coord.gateway_port;
                self.state = KuboState::Ready;
                update_coord_last_used(&self.data_dir);
            } else {
                eprintln!(
                    "ipfs-provider: stale coord file (pid {} dead), removing",
                    coord.kubo_pid
                );
                remove_coord_file(&self.data_dir);
            }
        }

        Response::ok(serde_json::json!({
            "provider": "ipfs-provider",
            "state": self.state,
        }))
    }

    // ── Ensure Kubo is running ──────────────────────────────────────

    fn ensure_kubo(&mut self) -> Result<(), String> {
        if self.state == KuboState::Ready {
            // Verify still alive
            if let Some(coord) = read_coord_file(&self.data_dir) {
                if is_pid_alive(coord.kubo_pid) {
                    update_coord_last_used(&self.data_dir);
                    return Ok(());
                }
                // PID died — remove stale coord and re-start
                eprintln!(
                    "ipfs-provider: Kubo pid {} died, restarting",
                    coord.kubo_pid
                );
                remove_coord_file(&self.data_dir);
            }
            self.state = KuboState::Cold;
        }

        if self.state == KuboState::Starting || self.state == KuboState::Error {
            self.state = KuboState::Cold;
        }

        // Ensure Kubo binary exists
        if self.kubo_binary.is_none() {
            return Err("kubo not found. Run: elastos setup --with kubo".to_string());
        }

        // Use lockfile protocol to safely start Kubo
        self.start_kubo_with_lock()
    }

    fn start_kubo_with_lock(&mut self) -> Result<(), String> {
        let lockfile_path = self.data_dir.join("ipfs-startup.lock");
        fs::create_dir_all(&self.data_dir)
            .map_err(|e| format!("Failed to create data dir: {}", e))?;

        let lockfile = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&lockfile_path)
            .map_err(|e| format!("Failed to open lockfile: {}", e))?;

        // Try exclusive lock (non-blocking)
        if try_flock_exclusive(&lockfile) {
            // We got the lock — check coord file again (another process may have finished)
            if let Some(coord) = read_coord_file(&self.data_dir) {
                if is_pid_alive(coord.kubo_pid) {
                    self.api_port = coord.api_port;
                    self.gateway_port = coord.gateway_port;
                    self.state = KuboState::Ready;
                    update_coord_last_used(&self.data_dir);
                    // Lock is released on drop
                    return Ok(());
                }
                remove_coord_file(&self.data_dir);
            }

            // Start Kubo (lock is released on drop)
            self.start_kubo()
        } else {
            // Another process is starting Kubo — poll coord file
            eprintln!("ipfs-provider: another process is starting Kubo, waiting...");
            let start = Instant::now();
            loop {
                std::thread::sleep(LOCKFILE_POLL_INTERVAL);
                if let Some(coord) = read_coord_file(&self.data_dir) {
                    if is_pid_alive(coord.kubo_pid) {
                        self.api_port = coord.api_port;
                        self.gateway_port = coord.gateway_port;
                        self.state = KuboState::Ready;
                        return Ok(());
                    }
                }
                if start.elapsed() > LOCKFILE_POLL_TIMEOUT {
                    return Err("Kubo startup timed out waiting for another process".to_string());
                }
            }
        }
    }

    fn start_kubo(&mut self) -> Result<(), String> {
        let binary = self.kubo_binary.as_ref().ok_or("Kubo binary not found")?;
        self.state = KuboState::Starting;

        // Init IPFS repo if absent
        if !self.repo_dir.join("config").is_file() {
            eprintln!(
                "ipfs-provider: initializing IPFS repo at {}",
                self.repo_dir.display()
            );
            let output = Command::new(binary)
                .args(["init"])
                .env("IPFS_PATH", &self.repo_dir)
                .output()
                .map_err(|e| format!("Failed to run kubo init: {}", e))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                // "already initialized" is not an error
                if !stderr.contains("already") {
                    self.state = KuboState::Error;
                    return Err(format!("kubo init failed: {}", stderr));
                }
            }
        }

        // Bind free ports
        let api_port = bind_free_port().map_err(|e| format!("Failed to bind API port: {}", e))?;
        let gw_port =
            bind_free_port().map_err(|e| format!("Failed to bind gateway port: {}", e))?;

        // Kubo v0.40.x does not support --gateway CLI flag, so set gateway in repo config.
        let gw_addr = format!("/ip4/127.0.0.1/tcp/{}", gw_port);
        let gw_cfg = Command::new(binary)
            .args(["config", "Addresses.Gateway", &gw_addr])
            .env("IPFS_PATH", &self.repo_dir)
            .output()
            .map_err(|e| format!("Failed to set Kubo gateway address: {}", e))?;
        if !gw_cfg.status.success() {
            let stderr = String::from_utf8_lossy(&gw_cfg.stderr);
            return Err(format!(
                "kubo config Addresses.Gateway failed: {}",
                stderr.trim()
            ));
        }

        // Start Kubo daemon
        let child = Command::new(binary)
            .args([
                "daemon",
                "--api",
                &format!("/ip4/127.0.0.1/tcp/{}", api_port),
                "--enable-gc",
            ])
            .env("IPFS_PATH", &self.repo_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                self.state = KuboState::Error;
                format!("Failed to spawn kubo: {}", e)
            })?;

        let pid = child.id();
        eprintln!(
            "ipfs-provider: spawned kubo pid={} api_port={}",
            pid, api_port
        );
        self.kubo_child = Some(child);
        self.api_port = api_port;
        self.gateway_port = gw_port;

        // Health poll until ready
        let health_url = format!("http://127.0.0.1:{}/api/v0/id", api_port);
        let start = Instant::now();
        loop {
            if start.elapsed() > KUBO_STARTUP_TIMEOUT {
                self.state = KuboState::Error;
                return Err(format!(
                    "Kubo health timeout after {}s",
                    KUBO_STARTUP_TIMEOUT.as_secs()
                ));
            }

            // Check if process died
            if let Some(ref mut child) = self.kubo_child {
                if let Ok(Some(status)) = child.try_wait() {
                    let mut stderr_tail = String::new();
                    if let Some(mut stderr) = child.stderr.take() {
                        let mut buf = String::new();
                        let _ = stderr.read_to_string(&mut buf);
                        if !buf.trim().is_empty() {
                            stderr_tail = format!(" stderr: {}", buf.trim());
                        }
                    }
                    self.state = KuboState::Error;
                    return Err(format!(
                        "Kubo exited during startup with status: {}{}",
                        status, stderr_tail
                    ));
                }
            }

            match ureq::post(&health_url)
                .timeout(Duration::from_secs(5))
                .call()
            {
                Ok(resp) if resp.status() == 200 => {
                    self.state = KuboState::Ready;
                    eprintln!(
                        "ipfs-provider: kubo ready (took {:.1}s)",
                        start.elapsed().as_secs_f64()
                    );
                    break;
                }
                _ => {}
            }

            std::thread::sleep(HEALTH_POLL_INTERVAL);
        }

        // Write coord file (atomic)
        let coord = CoordFile {
            kubo_pid: pid,
            api_port,
            gateway_port: gw_port,
            started_at: now_unix_secs(),
            last_used: now_unix_secs(),
        };
        write_coord_file(&self.data_dir, &coord);

        Ok(())
    }

    // ── Write ops ───────────────────────────────────────────────────

    fn add_bytes(&mut self, data_b64: &str, filename: &str, pin: bool) -> Response {
        if let Err(e) = self.ensure_kubo() {
            return Response::error("kubo_unavailable", &e);
        }

        let bytes = match BASE64.decode(data_b64) {
            Ok(b) => b,
            Err(e) => return Response::error("invalid_base64", &e.to_string()),
        };

        match self.kubo_add_bytes(&bytes, filename, pin) {
            Ok(cid) => Response::ok(serde_json::json!({ "cid": cid })),
            Err(e) => Response::error("add_failed", &e),
        }
    }

    fn add_path(&mut self, path: &str, pin: bool) -> Response {
        if let Err(e) = self.ensure_kubo() {
            return Response::error("kubo_unavailable", &e);
        }

        let file_path = Path::new(path);
        if !file_path.is_absolute() {
            return Response::error("invalid_path", "Path must be absolute");
        }
        if let Err(e) = validate_source_path(file_path) {
            return Response::error("path_not_allowed", &e);
        }
        if !file_path.exists() {
            return Response::error("not_found", &format!("File not found: {}", path));
        }

        match self.kubo_add_path(file_path, pin) {
            Ok(cid) => Response::ok(serde_json::json!({ "cid": cid })),
            Err(e) => Response::error("add_failed", &e),
        }
    }

    fn add_directory(&mut self, files: Vec<DirFile>, pin: bool) -> Response {
        if let Err(e) = self.ensure_kubo() {
            return Response::error("kubo_unavailable", &e);
        }

        match self.kubo_add_directory(files, pin) {
            Ok(cid) => Response::ok(serde_json::json!({ "cid": cid })),
            Err(e) => Response::error("add_failed", &e),
        }
    }

    // ── Read ops ────────────────────────────────────────────────────

    fn cat(&mut self, cid: &str, path: Option<&str>) -> Response {
        let arg = match path {
            Some(p) if !p.is_empty() => format!("{}/{}", cid, p.trim_start_matches('/')),
            _ => cid.to_string(),
        };

        match self.fetch_bytes(&arg) {
            Ok(bytes) => Response::ok(serde_json::json!({
                "data": BASE64.encode(&bytes)
            })),
            Err(e) => Response::error("cat_failed", &e),
        }
    }

    fn cat_to_path(&mut self, cid: &str, path: Option<&str>, dest: &str) -> Response {
        // Path safety validation
        let dest_path = Path::new(dest);
        if let Err(e) = validate_dest_path(dest_path) {
            return Response::error("invalid_dest", &e);
        }

        let arg = match path {
            Some(p) if !p.is_empty() => format!("{}/{}", cid, p.trim_start_matches('/')),
            _ => cid.to_string(),
        };

        match self.fetch_bytes(&arg) {
            Ok(bytes) => {
                if let Some(parent) = dest_path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                match fs::write(dest_path, &bytes) {
                    Ok(()) => Response::ok_empty(),
                    Err(e) => Response::error("write_failed", &e.to_string()),
                }
            }
            Err(e) => Response::error("cat_failed", &e),
        }
    }

    fn ls(&mut self, cid: &str) -> Response {
        // Try Kubo first
        if self.state == KuboState::Ready || self.ensure_kubo().is_ok() {
            let url = format!("{}/api/v0/ls?arg={}", self.api_url(), cid);
            if let Ok(resp) = ureq::post(&url).timeout(HTTP_TIMEOUT).call() {
                if resp.status() == 200 {
                    if let Ok(body) = resp.into_string() {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                            let mut entries = Vec::new();
                            collect_ls_entries(&json, "", &mut entries);
                            update_coord_last_used(&self.data_dir);
                            return Response::ok(serde_json::json!({ "entries": entries }));
                        }
                    }
                }
            }
        }

        match self.fetch_local_files_json(cid) {
            Ok(files) => {
                let entries: Vec<serde_json::Value> = files
                    .iter()
                    .map(|f| serde_json::json!({"name": f, "hash": "", "size": 0, "type": "file"}))
                    .collect();
                Response::ok(serde_json::json!({ "entries": entries }))
            }
            Err(e) => Response::error("ls_failed", &e),
        }
    }

    fn download_directory(&mut self, cid: &str, dest: &str) -> Response {
        let dest_path = Path::new(dest);
        if let Err(e) = validate_dest_path(dest_path) {
            return Response::error("invalid_dest", &e);
        }

        let _ = fs::create_dir_all(dest_path);

        // List files first
        let files = match self.list_dir_files(cid) {
            Ok(f) => f,
            Err(e) => return Response::error("ls_failed", &e),
        };

        let mut downloaded = Vec::new();
        let mut errors = Vec::new();

        for file_path in &files {
            // Path safety: reject traversal and absolute paths
            if file_path.contains("..") || file_path.starts_with('/') {
                eprintln!("ipfs-provider: skipping suspicious path: {}", file_path);
                continue;
            }

            let file_dest = dest_path.join(file_path);

            // Validate resolved path stays within dest
            if let Ok(canonical_dest) = fs::canonicalize(dest_path) {
                if let Some(parent) = file_dest.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let canonical_file = file_dest
                    .canonicalize()
                    .unwrap_or_else(|_| dest_path.join(file_path));
                if !canonical_file.starts_with(&canonical_dest) {
                    eprintln!("ipfs-provider: path escapes destination: {}", file_path);
                    continue;
                }
            }

            let arg = format!("{}/{}", cid, file_path);

            let bytes = if self.state == KuboState::Ready || self.ensure_kubo().is_ok() {
                let url = format!("{}/api/v0/cat?arg={}", self.api_url(), arg);
                match ureq::post(&url).timeout(LARGE_HTTP_TIMEOUT).call() {
                    Ok(resp) if resp.status() == 200 => {
                        let mut buf = Vec::new();
                        resp.into_reader().read_to_end(&mut buf).ok();
                        Some(buf)
                    }
                    _ => None,
                }
            } else {
                None
            };

            let bytes = match bytes {
                Some(b) => b,
                None => {
                    match self.fetch_from_local_gateway_with_timeout(&arg, LARGE_HTTP_TIMEOUT) {
                        Ok(b) => b,
                        Err(e) => {
                            errors.push(format!("{}: {}", file_path, e));
                            continue;
                        }
                    }
                }
            };

            if let Some(parent) = file_dest.parent() {
                let _ = fs::create_dir_all(parent);
            }
            match fs::write(&file_dest, &bytes) {
                Ok(()) => downloaded.push(file_path.clone()),
                Err(e) => errors.push(format!("{}: {}", file_path, e)),
            }
        }

        if !errors.is_empty() && downloaded.is_empty() {
            return Response::error(
                "download_failed",
                &format!("All downloads failed: {}", errors.join("; ")),
            );
        }

        update_coord_last_used(&self.data_dir);
        Response::ok(serde_json::json!({
            "files": downloaded,
            "errors": errors,
        }))
    }

    // ── Pin/Unpin ───────────────────────────────────────────────────

    fn pin(&mut self, cid: &str) -> Response {
        if let Err(e) = self.ensure_kubo() {
            return Response::error("kubo_unavailable", &e);
        }
        let url = format!("{}/api/v0/pin/add?arg={}", self.api_url(), cid);
        match ureq::post(&url).timeout(LARGE_HTTP_TIMEOUT).call() {
            Ok(resp) if resp.status() == 200 => Response::ok_empty(),
            Ok(resp) => Response::error("pin_failed", &format!("HTTP {}", resp.status())),
            Err(e) => Response::error("pin_failed", &e.to_string()),
        }
    }

    fn unpin(&mut self, cid: &str) -> Response {
        if let Err(e) = self.ensure_kubo() {
            return Response::error("kubo_unavailable", &e);
        }
        let url = format!("{}/api/v0/pin/rm?arg={}", self.api_url(), cid);
        match ureq::post(&url).timeout(HTTP_TIMEOUT).call() {
            Ok(resp) if resp.status() == 200 => Response::ok_empty(),
            Ok(resp) => Response::error("unpin_failed", &format!("HTTP {}", resp.status())),
            Err(e) => Response::error("unpin_failed", &e.to_string()),
        }
    }

    // ── Health/Status ───────────────────────────────────────────────

    fn health(&mut self) -> Response {
        if self.state != KuboState::Ready {
            return Response::ok(serde_json::json!({
                "healthy": false,
                "state": self.state,
            }));
        }

        let url = format!("{}/api/v0/id", self.api_url());
        match ureq::post(&url).timeout(Duration::from_secs(5)).call() {
            Ok(resp) if resp.status() == 200 => Response::ok(serde_json::json!({
                "healthy": true,
                "state": self.state,
                "api_port": self.api_port,
                "gateway_port": self.gateway_port,
            })),
            _ => {
                self.state = KuboState::Error;
                Response::ok(serde_json::json!({
                    "healthy": false,
                    "state": self.state,
                    "reason": "health_check_failed",
                }))
            }
        }
    }

    fn status(&self) -> Response {
        Response::ok(serde_json::json!({
            "version": PROVIDER_VERSION,
            "state": self.state,
            "api_endpoint": if self.api_port > 0 { Some(self.api_url()) } else { None },
            "gateway_endpoint": if self.gateway_port > 0 {
                Some(format!("http://127.0.0.1:{}", self.gateway_port))
            } else {
                None::<String>
            },
            "kubo_pid": read_coord_file(&self.data_dir).map(|c| c.kubo_pid),
        }))
    }

    fn shutdown(&mut self) -> Response {
        // DON'T kill shared Kubo — just update last_used
        update_coord_last_used(&self.data_dir);
        self.state = KuboState::Cold;
        Response::ok(serde_json::json!({"message": "ipfs-provider shutting down"}))
    }

    // ── Internal: Kubo API helpers ──────────────────────────────────

    fn kubo_add_bytes(&self, bytes: &[u8], filename: &str, pin: bool) -> Result<String, String> {
        let boundary = format!("----elastos{}", now_unix_secs());
        let mut body = Vec::new();

        write!(body, "--{}\r\n", boundary).unwrap();
        write!(
            body,
            "Content-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\n",
            filename
        )
        .unwrap();
        write!(body, "Content-Type: application/octet-stream\r\n\r\n").unwrap();
        body.extend_from_slice(bytes);
        write!(body, "\r\n--{}--\r\n", boundary).unwrap();

        let url = format!("{}/api/v0/add?pin={}", self.api_url(), pin);

        let resp = ureq::post(&url)
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={}", boundary),
            )
            .timeout(HTTP_TIMEOUT)
            .send_bytes(&body)
            .map_err(|e| format!("IPFS add failed: {}", e))?;

        if resp.status() != 200 {
            return Err(format!("IPFS add failed: HTTP {}", resp.status()));
        }

        let body_str = resp
            .into_string()
            .map_err(|e| format!("Failed to read response: {}", e))?;
        parse_add_response(&body_str)
    }

    fn kubo_add_path(&self, path: &Path, pin: bool) -> Result<String, String> {
        let bytes = fs::read(path).map_err(|e| format!("Failed to read file: {}", e))?;
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        self.kubo_add_bytes(&bytes, filename, pin)
    }

    fn kubo_add_directory(&self, files: Vec<DirFile>, pin: bool) -> Result<String, String> {
        let boundary = format!("----elastos{}", now_unix_secs());
        let mut body = Vec::new();

        for file in &files {
            let bytes = BASE64
                .decode(&file.data)
                .map_err(|e| format!("Invalid base64 for {}: {}", file.path, e))?;

            write!(body, "--{}\r\n", boundary).unwrap();
            write!(
                body,
                "Content-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\n",
                file.path
            )
            .unwrap();

            // Guess MIME type
            let mime = guess_mime(&file.path);
            write!(body, "Content-Type: {}\r\n\r\n", mime).unwrap();
            body.extend_from_slice(&bytes);
            write!(body, "\r\n").unwrap();
        }
        write!(body, "--{}--\r\n", boundary).unwrap();

        let url = format!(
            "{}/api/v0/add?wrap-with-directory=true&pin={}",
            self.api_url(),
            pin
        );

        let resp = ureq::post(&url)
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={}", boundary),
            )
            .timeout(LARGE_HTTP_TIMEOUT)
            .send_bytes(&body)
            .map_err(|e| format!("IPFS add directory failed: {}", e))?;

        if resp.status() != 200 {
            return Err(format!("IPFS add directory failed: HTTP {}", resp.status()));
        }

        // Parse NDJSON — the entry with empty Name is the root directory CID
        let body_str = resp
            .into_string()
            .map_err(|e| format!("Failed to read response: {}", e))?;
        let mut root_cid = None;
        for line in body_str.lines() {
            if line.is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
                let name = entry["Name"].as_str().unwrap_or("");
                let hash = entry["Hash"].as_str().unwrap_or("");
                if name.is_empty() && !hash.is_empty() {
                    root_cid = Some(hash.to_string());
                }
            }
        }

        root_cid.ok_or_else(|| "No root CID in IPFS response".to_string())
    }

    // ── Internal: local IPFS gateway only ──────────────────────────

    fn fetch_from_local_gateway_with_timeout(
        &self,
        arg: &str,
        timeout: Duration,
    ) -> Result<Vec<u8>, String> {
        if self.gateway_port == 0 {
            return Err(format!(
                "local Elastos IPFS gateway unavailable for {}. No HTTP fallback is allowed.",
                arg
            ));
        }

        let url = format!("http://127.0.0.1:{}/ipfs/{}", self.gateway_port, arg);
        match ureq::get(&url).timeout(timeout).call() {
            Ok(resp) if resp.status() == 200 => {
                let mut bytes = Vec::new();
                resp.into_reader()
                    .read_to_end(&mut bytes)
                    .map_err(|e| format!("local Elastos IPFS gateway -> {}", e))?;
                Ok(bytes)
            }
            Ok(resp) => Err(format!(
                "local Elastos IPFS gateway -> HTTP {} for {}. No HTTP fallback is allowed.",
                resp.status(),
                arg
            )),
            Err(e) => Err(format!(
                "local Elastos IPFS gateway -> {} for {}. No HTTP fallback is allowed.",
                e, arg
            )),
        }
    }

    fn fetch_local_files_json(&self, cid: &str) -> Result<Vec<String>, String> {
        if self.gateway_port == 0 {
            return Err(format!(
                "local Elastos IPFS gateway unavailable for {}/_files.json. No HTTP fallback is allowed.",
                cid
            ));
        }

        let url = format!(
            "http://127.0.0.1:{}/ipfs/{}/_files.json",
            self.gateway_port, cid
        );
        let resp = ureq::get(&url).timeout(HTTP_TIMEOUT).call().map_err(|e| {
            format!(
                "local Elastos IPFS gateway -> {} for {}/_files.json. No HTTP fallback is allowed.",
                e, cid
            )
        })?;

        if resp.status() != 200 {
            return Err(format!(
                "local Elastos IPFS gateway -> HTTP {} for {}/_files.json. No HTTP fallback is allowed.",
                resp.status(),
                cid
            ));
        }

        let body = resp
            .into_string()
            .map_err(|e| format!("Failed to read _files.json: {}", e))?;
        let mut files = serde_json::from_str::<Vec<String>>(&body)
            .map_err(|e| format!("Invalid _files.json for {}: {}", cid, e))?;
        for f in ["index.html", "_files.json"] {
            if !files.contains(&f.to_string()) {
                files.push(f.to_string());
            }
        }
        Ok(files)
    }

    fn list_dir_files(&mut self, cid: &str) -> Result<Vec<String>, String> {
        // Try Kubo API first
        if self.state == KuboState::Ready || self.ensure_kubo().is_ok() {
            let url = format!("{}/api/v0/ls?arg={}", self.api_url(), cid);
            if let Ok(resp) = ureq::post(&url).timeout(HTTP_TIMEOUT).call() {
                if resp.status() == 200 {
                    if let Ok(body) = resp.into_string() {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                            let mut files = Vec::new();
                            collect_ls_files_recursive(self, &json, "", &mut files);
                            if !files.is_empty() {
                                return Ok(files);
                            }
                        }
                    }
                }
            }
        }

        self.fetch_local_files_json(cid)
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn data_dir() -> PathBuf {
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

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn bind_free_port() -> Result<u16, String> {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind failed: {}", e))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("local_addr failed: {}", e))?
        .port();
    drop(listener);
    Ok(port)
}

fn guess_mime(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html",
        Some("css") => "text/css",
        Some("js") => "application/javascript",
        Some("json") => "application/json",
        Some("wasm") => "application/wasm",
        Some("md") => "text/markdown",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

fn parse_add_response(body: &str) -> Result<String, String> {
    // May be NDJSON — last entry with empty Name is root, or single JSON object
    let mut last_hash = None;
    let mut root_hash = None;

    for line in body.lines() {
        if line.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
            let hash = entry["Hash"].as_str().unwrap_or("");
            let name = entry["Name"].as_str().unwrap_or("");
            if !hash.is_empty() {
                last_hash = Some(hash.to_string());
                if name.is_empty() {
                    root_hash = Some(hash.to_string());
                }
            }
        }
    }

    root_hash
        .or(last_hash)
        .ok_or_else(|| "No Hash in IPFS add response".to_string())
}

// ── Coord file operations ───────────────────────────────────────────

fn coord_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ipfs-coords.json")
}

fn read_coord_file(data_dir: &Path) -> Option<CoordFile> {
    let path = coord_file_path(data_dir);
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

fn write_coord_file(data_dir: &Path, coord: &CoordFile) {
    let path = coord_file_path(data_dir);
    let _ = fs::create_dir_all(data_dir);
    let json = serde_json::to_string_pretty(coord).unwrap_or_default();
    // Atomic write: tmp + rename
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, &json).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

fn remove_coord_file(data_dir: &Path) {
    let _ = fs::remove_file(coord_file_path(data_dir));
}

fn update_coord_last_used(data_dir: &Path) {
    if let Some(mut coord) = read_coord_file(data_dir) {
        coord.last_used = now_unix_secs();
        write_coord_file(data_dir, &coord);
    }
}

// ── Process helpers ─────────────────────────────────────────────────

fn is_pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // kill(pid, 0) checks if process exists
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        // On non-Unix, check if the process is in the process list
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid)])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
}

#[cfg(unix)]
fn try_flock_exclusive(file: &fs::File) -> bool {
    use std::os::unix::io::AsRawFd;
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

#[cfg(not(unix))]
fn try_flock_exclusive(_file: &fs::File) -> bool {
    // On non-Unix, always "succeed" — coord file serves as coordination
    true
}

// ── Path safety ─────────────────────────────────────────────────────

fn validate_dest_path(dest: &Path) -> Result<(), String> {
    if !dest.is_absolute() {
        return Err("Destination path must be absolute".to_string());
    }

    // Canonicalize parent (dest itself may not exist yet)
    let resolved = if dest.exists() {
        dest.canonicalize()
            .map_err(|e| format!("Failed to canonicalize path: {}", e))?
    } else if let Some(parent) = dest.parent() {
        if parent.exists() {
            let p = parent
                .canonicalize()
                .map_err(|e| format!("Failed to canonicalize parent: {}", e))?;
            p.join(dest.file_name().unwrap_or_default())
        } else {
            dest.to_path_buf()
        }
    } else {
        dest.to_path_buf()
    };

    // Allowed prefixes
    let data_dir = data_dir();
    let tmp_dir = std::env::temp_dir();

    if !resolved.starts_with(&data_dir) && !resolved.starts_with(&tmp_dir) {
        return Err(format!(
            "Destination must be under {} or {}",
            data_dir.display(),
            tmp_dir.display()
        ));
    }

    Ok(())
}

/// Validate source paths for add_path — restrict to allowed roots.
/// Prevents arbitrary file exfiltration via the IPFS publish path.
fn validate_source_path(src: &Path) -> Result<(), String> {
    if !src.is_absolute() {
        return Err("Source path must be absolute".to_string());
    }

    // Canonicalize to resolve symlinks and ..
    let resolved = if src.exists() {
        src.canonicalize()
            .map_err(|e| format!("Failed to canonicalize source path: {}", e))?
    } else if let Some(parent) = src.parent() {
        if parent.exists() {
            let p = parent
                .canonicalize()
                .map_err(|e| format!("Failed to canonicalize parent: {}", e))?;
            p.join(src.file_name().unwrap_or_default())
        } else {
            src.to_path_buf()
        }
    } else {
        src.to_path_buf()
    };

    let data_dir = data_dir();
    let tmp_dir = std::env::temp_dir();

    if !resolved.starts_with(&data_dir) && !resolved.starts_with(&tmp_dir) {
        return Err(format!(
            "Source path must be under {} or {} (got: {})",
            data_dir.display(),
            tmp_dir.display(),
            resolved.display()
        ));
    }

    Ok(())
}

// ── Kubo binary discovery ───────────────────────────────────────────

fn find_kubo_binary() -> Option<PathBuf> {
    // 1. Explicit override
    if let Ok(path) = std::env::var("ELASTOS_IPFS_KUBO_PATH") {
        let p = PathBuf::from(&path);
        if p.is_file() {
            return Some(p);
        }
    }

    // 2. Standard install location
    let data_dir = data_dir();
    let installed = data_dir.join("bin/kubo");
    if installed.is_file() {
        return Some(installed);
    }

    // 3. System PATH (try both "kubo" and "ipfs")
    for name in ["kubo", "ipfs"] {
        if let Ok(output) = Command::new("which").arg(name).output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return Some(PathBuf::from(path));
                }
            }
        }
    }

    None
}

// ── ls helpers ──────────────────────────────────────────────────────

fn collect_ls_entries(json: &serde_json::Value, prefix: &str, out: &mut Vec<serde_json::Value>) {
    if let Some(objects) = json["Objects"].as_array() {
        for obj in objects {
            if let Some(links) = obj["Links"].as_array() {
                for link in links {
                    let name = link["Name"].as_str().unwrap_or("");
                    let hash = link["Hash"].as_str().unwrap_or("");
                    let size = link["Size"].as_u64().unwrap_or(0);
                    let link_type = link["Type"].as_u64().unwrap_or(0);
                    if name.is_empty() {
                        continue;
                    }
                    let path = if prefix.is_empty() {
                        name.to_string()
                    } else {
                        format!("{}/{}", prefix, name)
                    };
                    let type_str = match link_type {
                        1 => "directory",
                        2 => "file",
                        _ => "unknown",
                    };
                    out.push(serde_json::json!({
                        "name": path,
                        "hash": hash,
                        "size": size,
                        "type": type_str,
                    }));
                }
            }
        }
    }
}

fn collect_ls_files_recursive(
    provider: &IpfsProvider,
    json: &serde_json::Value,
    prefix: &str,
    out: &mut Vec<String>,
) {
    if let Some(objects) = json["Objects"].as_array() {
        for obj in objects {
            if let Some(links) = obj["Links"].as_array() {
                for link in links {
                    let name = link["Name"].as_str().unwrap_or("");
                    let hash = link["Hash"].as_str().unwrap_or("");
                    if name.is_empty() {
                        continue;
                    }
                    let path = if prefix.is_empty() {
                        name.to_string()
                    } else {
                        format!("{}/{}", prefix, name)
                    };
                    let link_type = link["Type"].as_u64().unwrap_or(0);
                    match link_type {
                        1 if !hash.is_empty() => {
                            // Directory — recurse
                            let url = format!("{}/api/v0/ls?arg={}", provider.api_url(), hash);
                            if let Ok(resp) = ureq::post(&url).timeout(HTTP_TIMEOUT).call() {
                                if resp.status() == 200 {
                                    if let Ok(body) = resp.into_string() {
                                        if let Ok(sub_json) =
                                            serde_json::from_str::<serde_json::Value>(&body)
                                        {
                                            collect_ls_files_recursive(
                                                provider, &sub_json, &path, out,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        2 => out.push(path),
                        _ => {}
                    }
                }
            }
        }
    }
}

// ── Idle timeout (background thread) ────────────────────────────────

fn spawn_idle_watcher(data_dir: PathBuf) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(IDLE_CHECK_INTERVAL);
            if let Some(coord) = read_coord_file(&data_dir) {
                let idle_secs = now_unix_secs().saturating_sub(coord.last_used);
                if idle_secs > IDLE_TIMEOUT_SECS {
                    eprintln!(
                        "ipfs-provider: Kubo idle for {}s (threshold {}s), stopping",
                        idle_secs, IDLE_TIMEOUT_SECS
                    );
                    // SIGTERM on Unix
                    #[cfg(unix)]
                    {
                        let pid_str = coord.kubo_pid.to_string();
                        let _ = Command::new("kill").args(["-TERM", &pid_str]).output();
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = Command::new("taskkill")
                            .args(["/PID", &coord.kubo_pid.to_string()])
                            .output();
                    }
                    remove_coord_file(&data_dir);
                    break;
                }
            } else {
                // No coord file — Kubo not running, exit watcher
                break;
            }
        }
    });
}

// ── Main loop ───────────────────────────────────────────────────────

fn main() {
    eprintln!("ipfs-provider: starting v{}", PROVIDER_VERSION);

    let mut provider = IpfsProvider::new();

    // Start idle watcher thread
    spawn_idle_watcher(provider.data_dir.clone());

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("ipfs-provider: stdin error: {}", e);
                break;
            }
        };
        if line.is_empty() {
            continue;
        }

        let request: Request = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(e) => {
                let response = Response::error("parse_error", &e.to_string());
                writeln!(stdout, "{}", serde_json::to_string(&response).unwrap()).unwrap();
                stdout.flush().unwrap();
                continue;
            }
        };

        let is_shutdown = matches!(request, Request::Shutdown);
        let response = provider.handle(request);

        let json = serde_json::to_string(&response).unwrap();
        writeln!(stdout, "{}", json).unwrap();
        stdout.flush().unwrap();

        if is_shutdown {
            break;
        }
    }

    // Shutdown: update last_used but don't kill shared Kubo
    provider.shutdown();
    eprintln!("ipfs-provider: exiting");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bind_free_port() {
        match bind_free_port() {
            Ok(port) => assert!(port > 0),
            Err(e) => eprintln!("Skipping (restricted env): {}", e),
        }
    }

    #[test]
    fn test_coord_file_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().to_path_buf();

        let coord = CoordFile {
            kubo_pid: 12345,
            api_port: 5001,
            gateway_port: 8080,
            started_at: 1000,
            last_used: 2000,
        };

        write_coord_file(&data_dir, &coord);
        let read = read_coord_file(&data_dir).expect("Should read coord file");
        assert_eq!(read.kubo_pid, 12345);
        assert_eq!(read.api_port, 5001);
        assert_eq!(read.gateway_port, 8080);

        remove_coord_file(&data_dir);
        assert!(read_coord_file(&data_dir).is_none());
    }

    #[test]
    fn test_stale_pid_reaping() {
        // PID 999999 should not be alive
        assert!(!is_pid_alive(999999));
    }

    #[test]
    fn test_kubo_state_serialization() {
        assert_eq!(serde_json::to_string(&KuboState::Cold).unwrap(), "\"cold\"");
        assert_eq!(
            serde_json::to_string(&KuboState::Starting).unwrap(),
            "\"starting\""
        );
        assert_eq!(
            serde_json::to_string(&KuboState::Ready).unwrap(),
            "\"ready\""
        );
        assert_eq!(
            serde_json::to_string(&KuboState::Error).unwrap(),
            "\"error\""
        );
    }

    #[test]
    fn test_request_deserialization() {
        let json = r#"{"op":"add_bytes","data":"aGVsbG8=","filename":"test.txt","pin":true}"#;
        let req: Request = serde_json::from_str(json).expect("Should parse add_bytes");
        match req {
            Request::AddBytes {
                data,
                filename,
                pin,
            } => {
                assert_eq!(data, "aGVsbG8=");
                assert_eq!(filename, "test.txt");
                assert!(pin);
            }
            _ => panic!("Expected AddBytes"),
        }
    }

    #[test]
    fn test_cat_request_deserialization() {
        let json = r#"{"op":"cat","cid":"QmTest","path":"file.txt"}"#;
        let req: Request = serde_json::from_str(json).expect("Should parse cat");
        match req {
            Request::Cat { cid, path } => {
                assert_eq!(cid, "QmTest");
                assert_eq!(path.as_deref(), Some("file.txt"));
            }
            _ => panic!("Expected Cat"),
        }
    }

    #[test]
    fn test_init_request_deserialization() {
        let json = r#"{"op":"init","config":{}}"#;
        let req: Request = serde_json::from_str(json).expect("Should parse init");
        assert!(matches!(req, Request::Init { .. }));
    }

    #[test]
    fn test_response_serialization() {
        let ok = Response::ok(serde_json::json!({"cid": "QmTest"}));
        let json = serde_json::to_string(&ok).unwrap();
        assert!(json.contains("\"status\":\"ok\""));
        assert!(json.contains("\"cid\":\"QmTest\""));

        let err = Response::error("test_code", "test message");
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("\"status\":\"error\""));
        assert!(json.contains("\"code\":\"test_code\""));
    }

    #[test]
    fn test_parse_add_response_single() {
        let body = r#"{"Name":"file.txt","Hash":"QmTest123","Size":"42"}"#;
        let cid = parse_add_response(body).unwrap();
        assert_eq!(cid, "QmTest123");
    }

    #[test]
    fn test_parse_add_response_ndjson_directory() {
        let body = r#"{"Name":"file.txt","Hash":"QmFile","Size":"42"}
{"Name":"","Hash":"QmRoot","Size":"100"}"#;
        let cid = parse_add_response(body).unwrap();
        assert_eq!(cid, "QmRoot");
    }

    #[test]
    fn test_validate_dest_path_rejects_relative() {
        let result = validate_dest_path(Path::new("relative/path"));
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_source_path_rejects_relative() {
        let result = validate_source_path(Path::new("relative/file"));
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_source_path_rejects_outside_roots() {
        let result = validate_source_path(Path::new("/etc/passwd"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be under"));
    }

    #[test]
    fn test_validate_source_path_allows_tmp() {
        let tmp = std::env::temp_dir().join("elastos-test-file");
        let result = validate_source_path(&tmp);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_source_path_allows_data_dir() {
        let path = data_dir().join("some-file.bin");
        let result = validate_source_path(&path);
        assert!(result.is_ok());
    }

    #[test]
    fn test_guess_mime() {
        assert_eq!(guess_mime("test.html"), "text/html");
        assert_eq!(guess_mime("test.json"), "application/json");
        assert_eq!(guess_mime("test.wasm"), "application/wasm");
        assert_eq!(guess_mime("test.bin"), "application/octet-stream");
    }

    #[test]
    fn test_status_cold() {
        let mut provider = IpfsProvider::new();
        let resp = provider.handle(Request::Status);
        match resp {
            Response::Ok { data: Some(d) } => {
                assert_eq!(d["state"], "cold");
            }
            other => panic!("Expected Ok, got {:?}", other),
        }
    }

    #[test]
    fn test_health_cold() {
        let mut provider = IpfsProvider::new();
        let resp = provider.handle(Request::Health);
        match resp {
            Response::Ok { data: Some(d) } => {
                assert_eq!(d["healthy"], false);
                assert_eq!(d["state"], "cold");
            }
            other => panic!("Expected Ok, got {:?}", other),
        }
    }

    #[test]
    fn test_shutdown_no_kubo() {
        let mut provider = IpfsProvider::new();
        let resp = provider.handle(Request::Shutdown);
        match resp {
            Response::Ok { data: Some(d) } => {
                assert!(d["message"].as_str().unwrap().contains("shutting down"));
            }
            other => panic!("Expected Ok, got {:?}", other),
        }
    }
}
