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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// Only Runtime's typed local Registry methods dispatch the private operations.
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod directory_hash;

const HEALTH_POLL_INTERVAL: Duration = Duration::from_secs(2);
const KUBO_STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
const KUBO_STOP_TIMEOUT: Duration = Duration::from_secs(5);
// Each of the two API calls before a migration's stop wait; with
// KUBO_STOP_TIMEOUT a migration fits Init's 10 s budget.
const KUBO_API_CHECK_TIMEOUT: Duration = Duration::from_secs(1);
const KUBO_STOP_POLL_INTERVAL: Duration = Duration::from_millis(100);
// Tests shorten the idle stop so it can be observed.
#[cfg(not(test))]
const IDLE_TIMEOUT_SECS: u64 = 600; // 10 minutes
#[cfg(test)]
const IDLE_TIMEOUT_SECS: u64 = 2;
#[cfg(not(test))]
const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(60);
#[cfg(test)]
const IDLE_CHECK_INTERVAL: Duration = Duration::from_millis(200);
const LOCKFILE_POLL_INTERVAL: Duration = Duration::from_millis(500);
const LOCKFILE_POLL_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const LARGE_HTTP_TIMEOUT: Duration = Duration::from_secs(300);
const CAT_TO_PATH_CHUNK: usize = 64 * 1024;
const ADD_PATH_CHUNK: usize = 1024 * 1024;
const BOUNDED_READ_TIMEOUT: Duration = Duration::from_secs(5);
const BOUNDED_READ_KUBO_TIMEOUT: &str = "100ms";
const MAX_BOUNDED_READ_BYTES: u64 = 64 * 1024;
// A whole-bounded read only reads this node's repo; 60 s covers a local read
// of the whole-content ceiling and keeps the one-at-a-time provider free.
const LOCAL_WHOLE_READ_TIMEOUT: Duration = Duration::from_secs(60);
const LOCAL_WHOLE_READ_KUBO_TIMEOUT: &str = "60s";
const MAX_CAPACITY_REQUIRED_BYTES: u64 = 64 * 1024 * 1024 * 1024;

// The upload reads one opened file. A short read or changed file cannot
// silently replace missing payload bytes with the multipart footer.
struct AddPathReader {
    file: fs::File,
    length: u64,
    remaining: u64,
    modified: SystemTime,
}

impl AddPathReader {
    fn new(file: fs::File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Upload source must be a regular file",
            ));
        }
        Ok(Self {
            file,
            length: metadata.len(),
            remaining: metadata.len(),
            modified: metadata.modified()?,
        })
    }

    fn unchanged(&self) -> io::Result<()> {
        let metadata = self.file.metadata()?;
        if metadata.len() != self.length || metadata.modified()? != self.modified {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Upload source changed during read",
            ));
        }
        Ok(())
    }
}

impl Read for AddPathReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            self.unchanged()?;
            return Ok(0);
        }
        let limit = buffer
            .len()
            .min(ADD_PATH_CHUNK)
            .min(self.remaining.min(usize::MAX as u64) as usize);
        let count = self.file.read(&mut buffer[..limit])?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Upload source ended before its recorded length",
            ));
        }
        self.remaining -= count as u64;
        if self.remaining == 0 {
            self.unchanged()?;
        }
        Ok(count)
    }
}

/// Opens a local object root below this provider's own data root. Every
/// component below the data root is opened O_NOFOLLOW|O_DIRECTORY, so no link
/// can lead the import outside it.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn open_local_object_root(data_root: &Path, root: &Path) -> io::Result<fs::File> {
    let outside = || {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Local object is outside the data root",
        )
    };
    let relative = root.strip_prefix(data_root).map_err(|_| outside())?;
    let mut dir = fs::File::open(data_root)?;
    if !dir.metadata()?.is_dir() {
        return Err(outside());
    }
    for part in relative.components() {
        let std::path::Component::Normal(name) = part else {
            return Err(outside());
        };
        dir = directory_hash::open_at(&dir, name.to_str().ok_or_else(outside)?, true)?;
    }
    Ok(dir)
}

/// Opens one declared file below an object root fd, following no link: it must
/// be a regular file of the recorded size owned by this user.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn open_local_object_file(root: &fs::File, path: &str, size: u64) -> io::Result<fs::File> {
    use std::os::unix::fs::MetadataExt;

    let (parents, name) = path.rsplit_once('/').unwrap_or(("", path));
    let mut dir = root.try_clone()?;
    for part in parents.split('/').filter(|part| !part.is_empty()) {
        dir = directory_hash::open_at(&dir, part, true)?;
    }
    let file = directory_hash::open_at(&dir, name, false)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } || metadata.len() != size
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Local object file is not the recorded regular file",
        ));
    }
    Ok(file)
}

/// Multipart body of one local object directory for Kubo `add`. Files open one
/// at a time when their part starts and stream through `AddPathReader`, so the
/// provider holds one bounded chunk whatever the object's size.
#[cfg(any(target_os = "linux", target_os = "macos"))]
struct LocalObjectBody<'a> {
    root: fs::File,
    files: std::slice::Iter<'a, StagedFile>,
    boundary: String,
    pending: io::Cursor<Vec<u8>>,
    current: Option<AddPathReader>,
    finished: bool,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl Read for LocalObjectBody<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.pending.read(buffer)?;
            if count != 0 {
                return Ok(count);
            }
            if let Some(reader) = self.current.as_mut() {
                let count = reader.read(buffer)?;
                if count != 0 {
                    return Ok(count);
                }
                self.current = None;
                self.pending = io::Cursor::new(b"\r\n".to_vec());
                continue;
            }
            if let Some(file) = self.files.next() {
                let source = open_local_object_file(&self.root, &file.path, file.size)?;
                self.current = Some(AddPathReader::new(source)?);
                self.pending = io::Cursor::new(directory_part_header(&self.boundary, &file.path));
            } else if !self.finished {
                self.finished = true;
                self.pending = io::Cursor::new(format!("--{}--\r\n", self.boundary).into_bytes());
            } else {
                return Ok(0);
            }
        }
    }
}

fn directory_part_header(boundary: &str, path: &str) -> Vec<u8> {
    format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{path}\"\r\nContent-Type: {}\r\n\r\n",
        guess_mime(path)
    )
    .into_bytes()
}

fn deserialize_metadata_max<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u64>, D::Error> {
    u64::deserialize(deserializer).map(Some)
}

const PROVIDER_VERSION: &str = env!("CARGO_PKG_VERSION");

// ── Protocol types ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    RuntimePrepareBackend {},
    RuntimeCheckCapacity {
        required_bytes: u64,
    },
    RuntimeHashStagedDirectory {
        directory: StagedDirectory,
    },
    RuntimeAddLocalObject {
        object: LocalObject,
    },
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
        #[serde(default, rename = "_runtime_invocation")]
        _runtime_invocation: Option<serde_json::Value>,
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
        #[serde(default, rename = "_runtime_invocation")]
        _runtime_invocation: Option<serde_json::Value>,
    },
    Cat {
        cid: String,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        bounded_read: bool,
        #[serde(default, deserialize_with = "deserialize_metadata_max")]
        max_bytes: Option<u64>,
        /// A whole read's own limit, at most the shared whole-content ceiling.
        #[serde(default)]
        whole_max_bytes: Option<u64>,
        #[serde(default, rename = "_runtime_invocation")]
        _runtime_invocation: Option<serde_json::Value>,
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
        #[serde(default, rename = "_runtime_invocation")]
        _runtime_invocation: Option<serde_json::Value>,
    },
    Unpin {
        cid: String,
        #[serde(default, rename = "_runtime_invocation")]
        _runtime_invocation: Option<serde_json::Value>,
    },
    PinStatus {
        cid: String,
        #[serde(default, rename = "_runtime_invocation")]
        _runtime_invocation: Option<serde_json::Value>,
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StagedFile {
    path: String,
    size: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StagedDirectory {
    root: PathBuf,
    files: Vec<StagedFile>,
    total_bytes: u64,
}

fn valid_root(root: &Path) -> bool {
    root.to_str().is_some_and(|root| {
        root.starts_with('/')
            && root.len() <= 4096
            && !root.contains('\0')
            && root[1..].split('/').count() <= 64
            && !root[1..].split('/').any(|p| matches!(p, "" | "." | ".."))
    })
}

impl StagedDirectory {
    fn valid_shape(&self) -> bool {
        if !valid_root(&self.root)
            || self.files.is_empty()
            || self.files.len() > 33
            || self.total_bytes == 0
            || self.total_bytes > 16 * 1024 * 1024 * 1024
        {
            return false;
        }
        let mut paths = std::collections::BTreeSet::new();
        let mut total = 0u64;
        for file in &self.files {
            if file.path.is_empty()
                || file.path.len() > 256
                || file.path.split('/').count() > 8
                || file.path.split('/').any(|p| {
                    matches!(p, "" | "." | "..")
                        || !p
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                })
                || !paths.insert(file.path.as_str())
            {
                return false;
            }
            let Some(sum) = total.checked_add(file.size) else {
                return false;
            };
            total = sum;
        }
        total == self.total_bytes
            && ["capsule.json", "_elastos_object.json"]
                .iter()
                .all(|required| {
                    self.files
                        .iter()
                        .any(|f| f.path == *required && (1..=65536).contains(&f.size))
                })
            && !self.files.iter().any(|f| {
                f.path
                    .match_indices('/')
                    .any(|(i, _)| paths.contains(&f.path[..i]))
            })
    }
}

const MAX_LOCAL_OBJECT_FILES: usize = 4096;

/// Runtime's descriptor of one complete local Content object directory: the
/// manifest's files plus `_elastos_object.json`, with their recorded sizes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalObject {
    root: PathBuf,
    files: Vec<StagedFile>,
}

impl LocalObject {
    fn valid_shape(&self) -> bool {
        let mut paths = std::collections::BTreeSet::new();
        // Paths become multipart filenames; quotes and controls cannot pass.
        valid_root(&self.root)
            && (1..=MAX_LOCAL_OBJECT_FILES).contains(&self.files.len())
            && self.files.iter().all(|file| {
                file.path.len() <= 4096
                    && !file.path.split('/').any(|p| matches!(p, "" | "." | ".."))
                    && !file
                        .path
                        .chars()
                        .any(|c| c.is_control() || c == '"' || c == '\\')
                    && paths.insert(file.path.as_str())
            })
            && paths.contains("_elastos_object.json")
            && !self.files.iter().any(|f| {
                f.path
                    .match_indices('/')
                    .any(|(i, _)| paths.contains(&f.path[..i]))
            })
    }
}

fn private_preparation_error() -> Response {
    Response::error(
        "preparation_unavailable",
        "Local content preparation unavailable",
    )
}

fn parse_request(line: &str) -> Result<Request, Response> {
    serde_json::from_str(line).map_err(|error| {
        // Private parse failures can contain descriptor values in serde's error.
        // Keep those values out of the provider-facing response.
        let private = serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|v| {
                v.get("op").and_then(|op| op.as_str()).map(|op| {
                    matches!(
                        op,
                        "runtime_prepare_backend"
                            | "runtime_hash_staged_directory"
                            | "runtime_check_capacity"
                            | "runtime_add_local_object"
                    )
                })
            })
            .unwrap_or(false);
        if private {
            private_preparation_error()
        } else {
            Response::error("parse_error", &error.to_string())
        }
    })
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum HostRole {
    #[default]
    User,
    Gateway,
}

impl HostRole {
    fn from_init(config: &serde_json::Value) -> Result<Self, ()> {
        // The role belongs to Runtime's private Init envelope, not arbitrary
        // top-level provider options or the process environment.
        if config.get("runtime_host_role").is_some() {
            return Err(());
        }
        match config
            .get("extra")
            .and_then(|extra| extra.get("runtime_host_role"))
        {
            None => Ok(Self::User),
            Some(serde_json::Value::String(role)) if role == "user" => Ok(Self::User),
            Some(serde_json::Value::String(role)) if role == "gateway" => Ok(Self::Gateway),
            Some(_) => Err(()),
        }
    }

    fn idle_stop_due(self, last_used: u64, now: u64) -> bool {
        self == Self::User && now.saturating_sub(last_used) > IDLE_TIMEOUT_SECS
    }
}

// ── Coord file ──────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct CoordFile {
    kubo_pid: u32,
    api_port: u16,
    gateway_port: u16,
    started_at: u64,
    last_used: u64,
    /// Set only for a daemon started with `--offline`. A coord file without it
    /// names a daemon from a release that ran Kubo online; it is never borrowed
    /// and is stopped under the startup lock so the repo restarts offline.
    #[serde(default)]
    offline: bool,
}

// ── Provider ────────────────────────────────────────────────────────

struct IpfsProvider {
    host_role: HostRole,
    initialized: bool,
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
            host_role: HostRole::User,
            initialized: false,
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

    fn kubo_cat_bytes(
        &mut self,
        arg: &str,
        timeout: Duration,
        limit: u64,
    ) -> Result<Vec<u8>, String> {
        let url = kubo_rpc_url(self.api_port, "cat", &[("arg", arg)]);
        match ureq::post(&url).timeout(timeout).call() {
            Ok(resp) if resp.status() == 200 => {
                let bytes = read_whole_content(resp.into_reader(), "kubo cat", limit)?;
                update_coord_last_used(&self.data_dir);
                Ok(bytes)
            }
            Ok(resp) => Err(format!("kubo cat -> HTTP {} for {}", resp.status(), arg)),
            Err(e) => Err(format!("kubo cat -> {} for {}", e, arg)),
        }
    }

    fn kubo_cat_to_path(
        &mut self,
        arg: &str,
        dest: &Path,
        timeout: Duration,
    ) -> Result<u64, String> {
        let url = kubo_rpc_url(self.api_port, "cat", &[("arg", arg), ("offline", "true")]);
        match ureq::post(&url).timeout(timeout).call() {
            Ok(resp) if resp.status() == 200 => {
                let written = copy_reader_to_path(resp.into_reader(), dest)?;
                update_coord_last_used(&self.data_dir);
                Ok(written)
            }
            Ok(resp) => Err(format!("kubo cat -> HTTP {} for {}", resp.status(), arg)),
            Err(e) => Err(format!("kubo cat -> {} for {}", e, arg)),
        }
    }

    /// One whole read of `arg` from this node's repo, refused once it passes
    /// `limit` bytes. Kubo runs offline, so a miss fails at once and Content
    /// goes to Carrier; no pin or gateway retry could find more.
    fn fetch_bytes(&mut self, arg: &str, limit: u64) -> Result<Vec<u8>, String> {
        self.ensure_kubo()?;
        self.kubo_cat_bytes(arg, LARGE_HTTP_TIMEOUT, limit)
    }

    /// One whole read of `arg` from this node's repo only, refused once it
    /// passes `limit` bytes. No network search, pin or gateway fallback. Kubo
    /// path cat of a missing DAG can keep an offline call open, so a short
    /// probe (as in `cat_bounded`) fails a miss before the full read.
    fn local_whole_bytes(&mut self, arg: &str, limit: u64) -> Result<Vec<u8>, String> {
        self.ensure_kubo()
            .map_err(|e| format!("local content backend is not ready: {e}"))?;
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .try_proxy_from_env(false)
            .build();
        let probe = kubo_rpc_url(
            self.api_port,
            "cat",
            &[
                ("arg", arg),
                ("offline", "true"),
                ("timeout", BOUNDED_READ_KUBO_TIMEOUT),
                ("length", "1"),
            ],
        );
        match agent.post(&probe).timeout(BOUNDED_READ_TIMEOUT).call() {
            Ok(resp) if resp.status() == 200 => {}
            _ => return Err(format!("{arg} is not in this node's IPFS repo")),
        }
        let url = kubo_rpc_url(
            self.api_port,
            "cat",
            &[
                ("arg", arg),
                ("offline", "true"),
                ("timeout", LOCAL_WHOLE_READ_KUBO_TIMEOUT),
            ],
        );
        match agent.post(&url).timeout(LOCAL_WHOLE_READ_TIMEOUT).call() {
            Ok(resp) if resp.status() == 200 => {
                let bytes = read_whole_content(resp.into_reader(), "kubo local cat", limit)?;
                update_coord_last_used(&self.data_dir);
                Ok(bytes)
            }
            Ok(resp) => Err(format!(
                "kubo local cat -> HTTP {} for {}",
                resp.status(),
                arg
            )),
            Err(e) => Err(format!("kubo local cat -> {} for {}", e, arg)),
        }
    }

    fn fetch_to_path(&mut self, arg: &str, dest: &Path) -> Result<u64, String> {
        // Dest copies borrow a local repo hit, then Content fails over to
        // Carrier.
        if self.ensure_kubo().is_ok() {
            self.kubo_cat_to_path(arg, dest, LARGE_HTTP_TIMEOUT)
        } else {
            Err("local dest cat backend is not ready".into())
        }
    }

    fn handle(&mut self, req: Request) -> Response {
        match req {
            Request::RuntimePrepareBackend {} => {
                if !cfg!(any(target_os = "linux", target_os = "macos")) {
                    return private_preparation_error();
                }
                match self.ensure_kubo() {
                    Ok(()) => {
                        #[cfg(any(target_os = "linux", target_os = "macos"))]
                        {
                            if let Err(error) =
                                directory_hash::seal_backend_repository(&self.repo_dir)
                            {
                                eprintln!("ipfs-provider: private repository seal failed: {error}");
                                self.state = KuboState::Error;
                                return private_preparation_error();
                            }
                            match directory_hash::verify_backend(self) {
                                Ok(()) => Response::ok_empty(),
                                Err(error) => {
                                    eprintln!(
                                        "ipfs-provider: private readiness probe failed: {error}"
                                    );
                                    self.state = KuboState::Error;
                                    private_preparation_error()
                                }
                            }
                        }
                        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                        {
                            private_preparation_error()
                        }
                    }
                    Err(error) => {
                        eprintln!("ipfs-provider: private readiness failed: {error}");
                        private_preparation_error()
                    }
                }
            }
            Request::RuntimeCheckCapacity { required_bytes } => {
                if !(1..=MAX_CAPACITY_REQUIRED_BYTES).contains(&required_bytes) {
                    return private_preparation_error();
                }
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                {
                    match directory_hash::check_capacity(self, required_bytes) {
                        Ok(observation) => {
                            // A model download reaches Kubo only through these checks.
                            update_coord_last_used(&self.data_dir);
                            Response::ok(serde_json::json!(observation))
                        }
                        Err(error) => {
                            eprintln!(
                                "ipfs-provider: private capacity observation failed: {error}"
                            );
                            private_preparation_error()
                        }
                    }
                }
                #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                {
                    private_preparation_error()
                }
            }
            Request::RuntimeHashStagedDirectory { directory } => {
                if !directory.valid_shape() {
                    return private_preparation_error();
                }
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                {
                    match directory_hash::hash_directory(self, &directory) {
                        Ok(cid) => {
                            update_coord_last_used(&self.data_dir);
                            Response::ok(serde_json::json!({"cid":cid}))
                        }
                        Err(error) => {
                            eprintln!("ipfs-provider: private directory hash failed: {error}");
                            private_preparation_error()
                        }
                    }
                }
                #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                {
                    private_preparation_error()
                }
            }
            Request::RuntimeAddLocalObject { object } => {
                if !object.valid_shape() {
                    return private_preparation_error();
                }
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                {
                    if let Err(error) = self.ensure_kubo() {
                        eprintln!("ipfs-provider: local object backend unavailable: {error}");
                        return private_preparation_error();
                    }
                    match self.kubo_add_local_object(&object) {
                        Ok(cid) => Response::ok(serde_json::json!({"cid":cid})),
                        Err(error) => {
                            eprintln!("ipfs-provider: local object import failed: {error}");
                            private_preparation_error()
                        }
                    }
                }
                #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                {
                    private_preparation_error()
                }
            }
            Request::Init { config } => self.init(config),
            Request::AddBytes {
                data,
                filename,
                pin,
                ..
            } => self.add_bytes(&data, &filename, pin),
            Request::AddPath { path, pin } => self.add_path(&path, pin),
            Request::AddDirectory { files, pin, .. } => self.add_directory(files, pin),
            Request::Cat {
                cid,
                path,
                bounded_read,
                max_bytes,
                whole_max_bytes,
                _runtime_invocation,
            } => {
                if bounded_read && whole_max_bytes.is_none() {
                    self.cat_bounded(
                        &cid,
                        path.as_deref(),
                        _runtime_invocation.as_ref(),
                        max_bytes,
                    )
                } else if bounded_read || max_bytes.is_some() {
                    Response::error("bounded_read_failed", "Bounded content read failed")
                } else {
                    self.cat(&cid, path.as_deref(), whole_max_bytes)
                }
            }
            Request::CatToPath { cid, path, dest } => {
                self.cat_to_path(&cid, path.as_deref(), &dest)
            }
            Request::GetBytes { cid, path } => self.cat(&cid, path.as_deref(), None),
            Request::Ls { cid } => self.ls(&cid),
            Request::DownloadDirectory { cid, dest } => self.download_directory(&cid, &dest),
            Request::Pin { cid, .. } => self.pin(&cid),
            Request::Unpin { cid, .. } => self.unpin(&cid),
            Request::PinStatus { cid, .. } => self.pin_status(&cid),
            Request::Health => self.health(),
            Request::Status => self.status(),
            Request::Shutdown => self.shutdown(),
        }
    }

    // ── Init ────────────────────────────────────────────────────────

    fn init(&mut self, config: serde_json::Value) -> Response {
        let Ok(host_role) = HostRole::from_init(&config) else {
            return Response::error("invalid_config", "Invalid Runtime IPFS host role");
        };
        let extra = config.get("extra").unwrap_or(&config);
        let selected_root = if let Some(base_path) = config
            .get("base_path")
            .and_then(serde_json::Value::as_str)
            .or_else(|| extra.get("data_dir").and_then(serde_json::Value::as_str))
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let path = PathBuf::from(base_path);
            if !path.is_absolute() {
                return Response::error(
                    "invalid_config",
                    "ipfs-provider data_dir must be absolute",
                );
            }
            path
        } else {
            self.data_dir.clone()
        };
        if self.initialized && (self.host_role != host_role || self.data_dir != selected_root) {
            return Response::error(
                "invalid_config",
                "Runtime IPFS role and data root are fixed at Init",
            );
        }
        if self.initialized {
            return Response::ok(
                serde_json::json!({"provider":"ipfs-provider", "state":self.state,
                "runtime_host_role":self.host_role}),
            );
        }
        self.host_role = host_role;
        self.data_dir = selected_root;
        self.repo_dir = self.data_dir.join("ipfs-repo");

        if extra.get("gateways").is_some() {
            eprintln!("ipfs-provider: ignoring gateway override; provider is local-IPFS only");
        }

        // Find Kubo binary
        match find_kubo_binary(&self.data_dir) {
            Some(path) => {
                eprintln!("ipfs-provider: found kubo at {}", path.display());
                self.kubo_binary = Some(path);
            }
            None => {
                eprintln!("ipfs-provider: kubo not found. Run: elastos setup --with kubo");
            }
        }

        // Borrow a running offline Kubo. One an earlier release started online
        // is asked to stop through its API now, so an update ends its
        // public-network activity; the next operation starts the repo offline.
        if let Some(coord) = read_coord_file(&self.data_dir) {
            if !is_pid_alive(coord.kubo_pid) {
                eprintln!(
                    "ipfs-provider: stale coord file (pid {} dead), removing",
                    coord.kubo_pid
                );
                remove_coord_file(&self.data_dir);
            } else if coord.offline {
                eprintln!(
                    "ipfs-provider: reusing existing Kubo (pid={}, api={})",
                    coord.kubo_pid, coord.api_port
                );
                self.api_port = coord.api_port;
                self.gateway_port = coord.gateway_port;
                self.state = KuboState::Ready;
                update_coord_last_used(&self.data_dir);
            } else if let Err(error) = stop_online_kubo_with_lock(&self.data_dir, &self.repo_dir) {
                eprintln!("ipfs-provider: {error}");
            }
        }

        self.initialized = true;
        eprintln!(
            "ipfs-provider: Runtime host role {:?}; idle stop {}",
            self.host_role,
            if self.host_role == HostRole::Gateway {
                "disabled"
            } else {
                "600s"
            }
        );
        Response::ok(serde_json::json!({
            "provider": "ipfs-provider",
            "state": self.state,
            "runtime_host_role": self.host_role,
        }))
    }

    // ── Ensure Kubo is running ──────────────────────────────────────

    fn ensure_kubo(&mut self) -> Result<(), String> {
        // Reap an owned daemon after the idle watcher or a crash stops it.
        if self
            .kubo_child
            .as_mut()
            .is_some_and(|child| child.try_wait().ok().flatten().is_some())
        {
            self.kubo_child = None;
        }
        if self.state == KuboState::Ready {
            // Verify still alive and offline
            if let Some(coord) = read_coord_file(&self.data_dir) {
                let alive = is_pid_alive(coord.kubo_pid);
                if alive && coord.offline {
                    self.api_port = coord.api_port;
                    self.gateway_port = coord.gateway_port;
                    update_coord_last_used(&self.data_dir);
                    return Ok(());
                }
                if !alive {
                    // PID died — remove stale coord and re-start
                    eprintln!(
                        "ipfs-provider: Kubo pid {} died, restarting",
                        coord.kubo_pid
                    );
                    remove_coord_file(&self.data_dir);
                }
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
        let lockfile = open_startup_lock(&self.data_dir)?;

        // Try exclusive lock (non-blocking)
        if try_flock_exclusive(&lockfile) {
            // We got the lock — check coord file again (another process may have finished)
            if let Some(coord) = read_coord_file(&self.data_dir) {
                if coord.offline && is_pid_alive(coord.kubo_pid) {
                    self.api_port = coord.api_port;
                    self.gateway_port = coord.gateway_port;
                    self.state = KuboState::Ready;
                    update_coord_last_used(&self.data_dir);
                    // Lock is released on drop
                    return Ok(());
                }
                if coord.offline {
                    remove_coord_file(&self.data_dir);
                } else {
                    retire_online_coord(&coord, &self.data_dir, &self.repo_dir)?;
                }
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
                    if coord.offline && is_pid_alive(coord.kubo_pid) {
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
        self.reap_previous_kubo()?;
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

        // Kubo v0.40.x has no --gateway flag, so the loopback gateway lives in
        // the repo config, rewritten with the local-store settings each start.
        if let Err(error) = write_local_store_config(&self.repo_dir, gw_port) {
            self.state = KuboState::Error;
            return Err(error);
        }

        // Start Kubo daemon
        let child = Command::new(binary)
            .args(kubo_daemon_args(api_port))
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
        let health_url = kubo_rpc_url(api_port, "id", &[]);
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
            offline: true,
        };
        write_coord_file(&self.data_dir, &coord);

        Ok(())
    }

    fn reap_previous_kubo(&mut self) -> Result<(), String> {
        let Some(child) = self.kubo_child.as_mut() else {
            return Ok(());
        };
        let mut deadline = Instant::now() + KUBO_STOP_TIMEOUT;
        let mut forced = false;
        loop {
            if child
                .try_wait()
                .map_err(|error| format!("Previous Kubo exit check failed: {error}"))?
                .is_some()
            {
                self.kubo_child = None;
                return Ok(());
            }
            if Instant::now() >= deadline {
                if forced {
                    return Err("Previous Kubo did not exit; replacement refused".into());
                }
                // Keep ownership until the stopped daemon releases its repo lock.
                // A concurrent exit can make kill fail; the next poll still reaps it.
                let _ = child.kill();
                forced = true;
                deadline = Instant::now() + KUBO_STOP_TIMEOUT;
            }
            std::thread::sleep(LOCKFILE_POLL_INTERVAL);
        }
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
        if let Err(e) = validate_source_path(&self.data_dir, file_path) {
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

    fn cat_bounded(
        &mut self,
        cid: &str,
        path: Option<&str>,
        invocation: Option<&serde_json::Value>,
        max_bytes: Option<u64>,
    ) -> Response {
        let result = (|| -> Result<serde_json::Value, String> {
            let invocation = invocation.ok_or("bounded read requires Runtime invocation")?;
            if invocation["schema"] != "elastos.provider.invocation/v1"
                || invocation["target"] != "ipfs"
                || invocation["op"] != "cat"
                || invocation["transport"] != "runtime-local-provider-plane"
                || !matches!(invocation["transfer"].as_str(), Some("bytes" | "stream"))
            {
                return Err("invalid bounded read invocation".into());
            }
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Range {
                start: u64,
                end: u64,
            }
            let range: Option<Range> = if let Some(max) = max_bytes {
                if !(1..=MAX_BOUNDED_READ_BYTES).contains(&max)
                    || path != Some("_elastos_object.json")
                    || invocation.get("range").is_some_and(|v| !v.is_null())
                    || invocation
                        .pointer("/progress/expected_bytes")
                        .is_some_and(|v| !v.is_null())
                {
                    return Err("invalid complete metadata request".into());
                }
                None
            } else {
                Some(
                    serde_json::from_value(invocation["range"].clone())
                        .map_err(|_| "bounded read requires a closed range")?,
                )
            };
            let length = if let Some(range) = &range {
                range
                    .end
                    .checked_sub(range.start)
                    .and_then(|n| n.checked_add(1))
                    .filter(|n| *n <= MAX_BOUNDED_READ_BYTES)
                    .ok_or("bounded read range exceeds limit")?
            } else {
                max_bytes.ok_or("missing metadata bound")?
            };
            if let Some(expected) = invocation
                .pointer("/progress/expected_bytes")
                .filter(|v| !v.is_null())
            {
                if expected.as_u64() != Some(length) {
                    return Err("bounded read expected length mismatch".into());
                }
            }
            let path = path.unwrap_or("");
            if cid.is_empty()
                || cid.len() > 128
                || !cid.bytes().all(|b| b.is_ascii_alphanumeric())
                || path.len() > 1024
                || (!path.is_empty()
                    && path.split('/').any(|part| {
                        part.is_empty()
                            || part == "."
                            || part == ".."
                            || !part
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                    }))
            {
                return Err("invalid bounded content identity".into());
            }
            // A bounded read borrows the ready backend; it never starts or pins it.
            if self.state != KuboState::Ready || self.api_port == 0 {
                return Err("bounded read backend is not ready".into());
            }
            let coord = read_coord_file(&self.data_dir)
                .filter(|coord| coord.offline && is_pid_alive(coord.kubo_pid));
            let Some(coord) = coord else {
                self.state = KuboState::Cold;
                return Err("bounded read backend is not ready".into());
            };
            self.api_port = coord.api_port;
            self.gateway_port = coord.gateway_port;
            let arg = if path.is_empty() {
                cid.to_string()
            } else {
                format!("{cid}/{path}")
            };
            update_coord_last_used(&self.data_dir);
            let agent = ureq::AgentBuilder::new()
                .redirects(0)
                .try_proxy_from_env(false)
                .build();
            // A bounded read is the local-repo probe before Carrier
            // availability. Searching DHT here would spend the whole
            // deadline on a miss and then still fail over. Kubo path cat
            // of a missing DAG also keeps the HTTP call open with
            // offline=true; a short API timeout fails that miss. A local
            // 64 KiB hit still returns the bounded range after Ready.
            let (offset_value, length_value) = match &range {
                Some(range) => (Some(range.start.to_string()), length.to_string()),
                None => (None, (length + 1).to_string()),
            };
            let mut query = vec![
                ("arg", arg.as_str()),
                ("offline", "true"),
                ("timeout", BOUNDED_READ_KUBO_TIMEOUT),
            ];
            if let Some(offset_value) = &offset_value {
                query.push(("offset", offset_value));
            }
            query.push(("length", &length_value));
            let request = agent
                .post(&kubo_rpc_url(self.api_port, "cat", &query))
                .set("Accept-Encoding", "identity")
                .timeout(BOUNDED_READ_TIMEOUT);
            let response = request.call().map_err(|e| e.to_string())?;
            let declared_length = response
                .header("Content-Length")
                .map(|value| {
                    value
                        .parse::<u64>()
                        .map_err(|_| "invalid bounded response length")
                })
                .transpose()?;
            if response.status() != 200
                || response
                    .header("Content-Encoding")
                    .is_some_and(|v| v != "identity")
                || declared_length.is_some_and(|n| {
                    if range.is_some() {
                        n != length
                    } else {
                        n > length
                    }
                })
            {
                return Err("invalid bounded read response".into());
            }
            let mut bytes = Vec::with_capacity(length as usize + 1);
            response
                .into_reader()
                .take(length + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if (range.is_some() && bytes.len() as u64 != length)
                || declared_length.is_some_and(|n| bytes.len() as u64 != n)
                || (range.is_none() && (bytes.is_empty() || bytes.len() as u64 > length))
            {
                return Err("bounded read response length mismatch".into());
            }
            update_coord_last_used(&self.data_dir);
            if range.is_none() {
                return Ok(serde_json::json!({"data":BASE64.encode(&bytes),
                    "_runtime_complete_metadata":{"schema":"elastos.provider.complete-metadata/v1",
                        "cid":cid,"path":path,"max_bytes":length,"actual_bytes":bytes.len(),"completed":true}}));
            }
            let range = range.ok_or("missing applied range")?;
            Ok(serde_json::json!({
                "data": BASE64.encode(&bytes),
                "_runtime_applied_range": {
                    "schema": "elastos.provider.applied-range/v1",
                    "cid": cid, "path": path, "start": range.start, "end": range.end
                }
            }))
        })();
        match result {
            Ok(data) => Response::ok(data),
            Err(error) => {
                eprintln!("ipfs-provider: bounded read failed: {error}");
                Response::error("bounded_read_failed", "Bounded content read failed")
            }
        }
    }

    /// One whole file, refused once it passes `whole_max_bytes` (when given)
    /// or the shared whole-content ceiling. A read with `whole_max_bytes`
    /// comes from this node's repo only.
    fn cat(&mut self, cid: &str, path: Option<&str>, whole_max_bytes: Option<u64>) -> Response {
        let limit = match whole_max_bytes {
            None => WHOLE_CONTENT_CEILING,
            Some(limit) if (1..=WHOLE_CONTENT_CEILING).contains(&limit) => limit,
            Some(_) => {
                return Response::error(
                    "invalid_request",
                    "whole_max_bytes must be between 1 and the whole-content ceiling",
                )
            }
        };
        let arg = match path {
            Some(p) if !p.is_empty() => format!("{}/{}", cid, p.trim_start_matches('/')),
            _ => cid.to_string(),
        };

        let result = if whole_max_bytes.is_some() {
            self.local_whole_bytes(&arg, limit)
        } else {
            self.fetch_bytes(&arg, limit)
        };
        match result {
            Ok(bytes) => Response::ok(serde_json::json!({
                "data": BASE64.encode(&bytes)
            })),
            Err(e) => Response::error("cat_failed", &e),
        }
    }

    fn cat_to_path(&mut self, cid: &str, path: Option<&str>, dest: &str) -> Response {
        // Path safety validation
        let dest_path = Path::new(dest);
        if let Err(e) = validate_dest_path(&self.data_dir, dest_path) {
            return Response::error("invalid_dest", &e);
        }

        let arg = match path {
            Some(p) if !p.is_empty() => format!("{}/{}", cid, p.trim_start_matches('/')),
            _ => cid.to_string(),
        };

        match self.fetch_to_path(&arg, dest_path) {
            Ok(_) => Response::ok_empty(),
            Err(e) => Response::error("cat_failed", &e),
        }
    }

    fn ls(&mut self, cid: &str) -> Response {
        match self.kubo_ls(cid) {
            Ok(json) => {
                let mut entries = Vec::new();
                collect_ls_entries(&json, "", &mut entries);
                update_coord_last_used(&self.data_dir);
                Response::ok(serde_json::json!({ "entries": entries }))
            }
            Err(e) => Response::error("ls_failed", &e),
        }
    }

    /// Kubo's listing of `cid` from this node's repo (Kubo runs offline).
    fn kubo_ls(&mut self, cid: &str) -> Result<serde_json::Value, String> {
        self.ensure_kubo()?;
        let url = kubo_rpc_url(self.api_port, "ls", &[("arg", cid)]);
        let resp = ureq::post(&url)
            .timeout(HTTP_TIMEOUT)
            .call()
            .map_err(|e| format!("kubo ls -> {e} for {cid}"))?;
        let body = resp
            .into_string()
            .map_err(|e| format!("kubo ls -> {e} for {cid}"))?;
        serde_json::from_str(&body).map_err(|e| format!("kubo ls -> {e} for {cid}"))
    }

    fn download_directory(&mut self, cid: &str, dest: &str) -> Response {
        let dest_path = Path::new(dest);
        if let Err(e) = validate_dest_path(&self.data_dir, dest_path) {
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

            let bytes = match self.fetch_bytes(&arg, WHOLE_CONTENT_CEILING) {
                Ok(bytes) => bytes,
                Err(e) => {
                    errors.push(format!("{}: {}", file_path, e));
                    continue;
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
        let url = kubo_rpc_url(self.api_port, "pin/add", &[("arg", cid)]);
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
        let url = kubo_rpc_url(self.api_port, "pin/rm", &[("arg", cid)]);
        match ureq::post(&url).timeout(HTTP_TIMEOUT).call() {
            Ok(resp) if resp.status() == 200 => Response::ok_empty(),
            Ok(resp) => Response::error("unpin_failed", &format!("HTTP {}", resp.status())),
            Err(e) => Response::error("unpin_failed", &e.to_string()),
        }
    }

    /// Whether this node pins `cid` recursively. Kubo answers from the local
    /// pin set offline, so the question never fetches the CID.
    fn pin_status(&mut self, cid: &str) -> Response {
        if let Err(e) = self.ensure_kubo() {
            return Response::error("kubo_unavailable", &e);
        }
        let url = kubo_rpc_url(
            self.api_port,
            "pin/ls",
            &[("arg", cid), ("type", "recursive"), ("offline", "true")],
        );
        match ureq::post(&url).timeout(HTTP_TIMEOUT).call() {
            Ok(resp) if resp.status() == 200 => {
                // A pin answer names the pin: one recursive entry under `Keys`.
                let recursive = resp
                    .into_string()
                    .ok()
                    .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
                    .and_then(|body| body.get("Keys")?.as_object().cloned())
                    .is_some_and(|keys| keys.values().any(|pin| pin["Type"] == "recursive"));
                if recursive {
                    Response::ok(serde_json::json!({ "pinned": true }))
                } else {
                    Response::error("pin_status_failed", "Kubo returned no recursive pin")
                }
            }
            Ok(resp) => Response::error("pin_status_failed", &format!("HTTP {}", resp.status())),
            Err(ureq::Error::Status(_, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                if body.contains("is not pinned") || body.contains("not found locally") {
                    Response::ok(serde_json::json!({ "pinned": false }))
                } else {
                    Response::error("pin_status_failed", "Kubo could not read the pin set")
                }
            }
            Err(e) => Response::error("pin_status_failed", &e.to_string()),
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

        let url = kubo_rpc_url(self.api_port, "id", &[]);
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

        let url = kubo_rpc_url(self.api_port, "add", &[("pin", &pin.to_string())]);

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
        if !fs::metadata(path)
            .map_err(|e| format!("Failed to read file: {}", e))?
            .is_file()
        {
            return Err("Upload source must be a regular file".into());
        }
        let file = fs::File::open(path).map_err(|e| format!("Failed to open file: {}", e))?;
        let reader = file
            .try_clone()
            .and_then(AddPathReader::new)
            .map_err(|e| format!("Failed to read file: {}", e))?;
        let length = reader.length;
        let modified = reader.modified;
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        let boundary = format!("----elastos{}", now_unix_secs());
        let prefix = format!("--{}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\nContent-Type: application/octet-stream\r\n\r\n", boundary, filename).into_bytes();
        let footer = format!("\r\n--{}--\r\n", boundary).into_bytes();
        let content_length = length
            .checked_add(prefix.len() as u64)
            .and_then(|n| n.checked_add(footer.len() as u64))
            .ok_or("Upload content length exceeds its bound")?;
        let body = io::Cursor::new(prefix)
            .chain(reader)
            .chain(io::Cursor::new(footer));
        let url = kubo_rpc_url(self.api_port, "add", &[("pin", &pin.to_string())]);
        let resp = ureq::post(&url)
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={}", boundary),
            )
            .set("Content-Length", &content_length.to_string())
            .timeout(LARGE_HTTP_TIMEOUT)
            .send(body)
            .map_err(|e| format!("IPFS add failed: {}", e))?;
        let metadata = file
            .metadata()
            .map_err(|e| format!("Failed to read file: {}", e))?;
        if metadata.len() != length || metadata.modified().map_err(|e| e.to_string())? != modified {
            return Err("Upload source changed during read".into());
        }
        if resp.status() != 200 {
            return Err(format!("IPFS add failed: HTTP {}", resp.status()));
        }
        let body_str = resp
            .into_string()
            .map_err(|e| format!("Failed to read response: {}", e))?;
        parse_add_response(&body_str)
    }

    fn kubo_add_directory(&self, files: Vec<DirFile>, pin: bool) -> Result<String, String> {
        let boundary = format!("----elastos{}", now_unix_secs());
        let mut body = Vec::new();

        for file in &files {
            let bytes = BASE64
                .decode(&file.data)
                .map_err(|e| format!("Invalid base64 for {}: {}", file.path, e))?;
            body.extend_from_slice(&directory_part_header(&boundary, &file.path));
            body.extend_from_slice(&bytes);
            write!(body, "\r\n").unwrap();
        }
        write!(body, "--{}--\r\n", boundary).unwrap();

        let resp = ureq::post(&self.add_directory_url(pin))
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={}", boundary),
            )
            .timeout(LARGE_HTTP_TIMEOUT)
            .send_bytes(&body)
            .map_err(|e| format!("IPFS add directory failed: {}", e))?;
        directory_root(resp)
    }

    /// Adds one complete local object directory to Kubo unpinned, with the same
    /// parts and add options as `add_directory`, so its CID equals the original
    /// import; Runtime pins it only after comparing that CID. Files stream from
    /// disk through fds opened below the data root without following links.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn kubo_add_local_object(&self, object: &LocalObject) -> Result<String, String> {
        let root = open_local_object_root(&self.data_dir, &object.root)
            .map_err(|e| format!("Local object root refused: {}", e))?;
        let boundary = format!("----elastos{}", now_unix_secs());
        let mut length = format!("--{boundary}--\r\n").len() as u64;
        for file in &object.files {
            open_local_object_file(&root, &file.path, file.size)
                .map_err(|e| format!("Local object file refused: {}", e))?;
            length = length
                .checked_add(directory_part_header(&boundary, &file.path).len() as u64 + 2)
                .and_then(|n| n.checked_add(file.size))
                .ok_or("Local object length exceeds its bound")?;
        }
        let body = LocalObjectBody {
            root,
            files: object.files.iter(),
            boundary: boundary.clone(),
            pending: io::Cursor::new(Vec::new()),
            current: None,
            finished: false,
        };
        let resp = ureq::post(&self.add_directory_url(false))
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={}", boundary),
            )
            .set("Content-Length", &length.to_string())
            .timeout(LARGE_HTTP_TIMEOUT)
            .send(body)
            .map_err(|e| format!("IPFS add local object failed: {}", e))?;
        directory_root(resp)
    }

    fn add_directory_url(&self, pin: bool) -> String {
        let pin = pin.to_string();
        kubo_rpc_url(
            self.api_port,
            "add",
            &[
                ("wrap-with-directory", "true"),
                ("pin", &pin),
                ("cid-version", "1"),
                ("hash", "sha2-256"),
                ("raw-leaves", "true"),
                ("chunker", "size-262144"),
                ("trickle", "false"),
                ("max-file-links", "174"),
                ("max-directory-links", "0"),
                ("max-hamt-fanout", "256"),
                ("inline", "false"),
                ("inline-limit", "32"),
                ("nocopy", "false"),
                ("fscache", "false"),
                ("preserve-mode", "false"),
                ("preserve-mtime", "false"),
                ("empty-dirs", "false"),
                ("progress", "false"),
                ("fast-provide-root", "false"),
                ("fast-provide-wait", "false"),
            ],
        )
    }

    fn list_dir_files(&mut self, cid: &str) -> Result<Vec<String>, String> {
        let json = self.kubo_ls(cid)?;
        let mut files = Vec::new();
        collect_ls_files_recursive(self, &json, "", &mut files);
        if files.is_empty() {
            return Err(format!("kubo ls -> no files under {cid}"));
        }
        Ok(files)
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

struct DataDirChoice<'a> {
    explicit: Option<&'a str>,
    parent_content_repo: Option<&'a Path>,
    xdg_data_home: Option<&'a str>,
    home: Option<&'a Path>,
}

fn select_data_dir(choice: DataDirChoice<'_>) -> PathBuf {
    if let Some(dir) = choice.explicit.map(str::trim).filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(parent) = choice.parent_content_repo {
        if parent.is_absolute() && parent.join("ipfs-repo").join("config").is_file() {
            return parent.to_path_buf();
        }
    }
    if let Some(dir) = choice
        .xdg_data_home
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
    {
        return PathBuf::from(dir).join("elastos");
    }
    if let Some(home) = choice.home {
        #[cfg(target_os = "macos")]
        {
            return home.join("Library/Application Support/elastos");
        }
        #[cfg(not(target_os = "macos"))]
        {
            return home.join(".local/share/elastos");
        }
    }
    PathBuf::from("/tmp/elastos")
}

fn data_dir() -> PathBuf {
    let explicit = std::env::var("ELASTOS_DATA_DIR").ok();
    let parent = std::env::var("ELASTOS_HOME_LAUNCH_TRUSTED_AUTH_DATA_DIR").ok();
    let parent_path = parent.as_deref().map(Path::new);
    let xdg = std::env::var("XDG_DATA_HOME").ok();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    select_data_dir(DataDirChoice {
        explicit: explicit.as_deref(),
        parent_content_repo: parent_path,
        xdg_data_home: xdg.as_deref(),
        home: home.as_deref(),
    })
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

/// Root CID of a wrapped directory add: the NDJSON entry with an empty Name.
fn directory_root(resp: ureq::Response) -> Result<String, String> {
    if resp.status() != 200 {
        return Err(format!("IPFS add directory failed: HTTP {}", resp.status()));
    }
    let body = resp
        .into_string()
        .map_err(|e| format!("Failed to read response: {}", e))?;
    body.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|entry| entry["Name"].as_str().unwrap_or("").is_empty())
        .filter_map(|entry| entry["Hash"].as_str().map(str::to_owned))
        .filter(|hash| !hash.is_empty())
        .next_back()
        .ok_or_else(|| "No root CID in IPFS response".to_string())
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

fn open_startup_lock(data_dir: &Path) -> Result<fs::File, String> {
    fs::create_dir_all(data_dir).map_err(|e| format!("Failed to create data dir: {}", e))?;
    fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(data_dir.join("ipfs-startup.lock"))
        .map_err(|e| format!("Failed to open lockfile: {}", e))
}

// ── Offline Kubo ────────────────────────────────────────────────────

/// `--offline` runs Kubo with no swarm, bitswap, DHT or provide, so this repo
/// is a local content store and a missing CID fails at once. Content moves
/// between Homes only over Carrier.
fn kubo_daemon_args(api_port: u16) -> [String; 5] {
    [
        "daemon".into(),
        "--offline".into(),
        "--api".into(),
        format!("/ip4/127.0.0.1/tcp/{api_port}"),
        "--enable-gc".into(),
    ]
}

/// Sets the repo config so Kubo neither announces nor routes even if someone
/// starts it without `--offline`: no swarm listener, bootstrap, routing,
/// provide, LAN discovery, AutoConf fetch or telemetry; the gateway stays on
/// loopback. Kubo refuses AutoConf `auto` placeholders once AutoConf is off,
/// so DNS keeps only explicitly configured resolvers.
fn apply_local_store_config(
    config: &mut serde_json::Value,
    gateway_port: u16,
) -> Result<(), String> {
    use serde_json::json;
    let settings: [(&[&str], serde_json::Value); 10] = [
        (
            &["Addresses", "Gateway"],
            json!(format!("/ip4/127.0.0.1/tcp/{gateway_port}")),
        ),
        (&["Addresses", "Swarm"], json!([])),
        (&["Bootstrap"], json!([])),
        (&["Routing", "Type"], json!("none")),
        (&["Routing", "DelegatedRouters"], json!([])),
        (&["Ipns", "DelegatedPublishers"], json!([])),
        (&["Provide", "Enabled"], json!(false)),
        (&["Discovery", "MDNS", "Enabled"], json!(false)),
        (&["AutoConf", "Enabled"], json!(false)),
        (
            &["Plugins", "Plugins", "telemetry", "Config", "Mode"],
            json!("off"),
        ),
    ];
    for (path, value) in settings {
        *kubo_config_entry(config, path)? = value;
    }
    let resolvers = kubo_config_entry(config, &["DNS", "Resolvers"])?;
    match resolvers.as_object_mut() {
        Some(resolvers) => resolvers.retain(|_, resolver| resolver.as_str() != Some("auto")),
        None => *resolvers = json!({}),
    }
    Ok(())
}

/// The config value at `path`, creating missing or null objects on the way
/// (`kubo init` writes `"Plugins": {"Plugins": null}`).
fn kubo_config_entry<'a>(
    config: &'a mut serde_json::Value,
    path: &[&str],
) -> Result<&'a mut serde_json::Value, String> {
    let mut node = config;
    for key in path {
        if node.is_null() {
            *node = serde_json::Value::Object(Default::default());
        }
        node = node
            .as_object_mut()
            .ok_or_else(|| format!("Kubo config above {} is not an object", path.join(".")))?
            .entry(*key)
            .or_insert(serde_json::Value::Null);
    }
    Ok(node)
}

/// Rewrites `repo/config` with `apply_local_store_config`, keeping it
/// owner-only (it holds the node key). Runs with no daemon on the repo.
fn write_local_store_config(repo_dir: &Path, gateway_port: u16) -> Result<(), String> {
    let path = repo_dir.join("config");
    let bytes = fs::read(&path).map_err(|e| format!("Failed to read Kubo config: {e}"))?;
    let mut config: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid Kubo config: {e}"))?;
    apply_local_store_config(&mut config, gateway_port)?;
    let body =
        serde_json::to_vec_pretty(&config).map_err(|e| format!("Kubo config encode: {e}"))?;
    let tmp = repo_dir.join("config.elastos-tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&tmp)
        .map_err(|e| format!("Failed to write Kubo config: {e}"))?;
    file.write_all(&body)
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&tmp, &path))
        .map_err(|e| format!("Failed to write Kubo config: {e}"))
}

/// Init's migration: retires an online daemon now unless another process
/// holds the startup lock, which then retires it on its own start.
fn stop_online_kubo_with_lock(data_dir: &Path, repo_dir: &Path) -> Result<(), String> {
    let lockfile = open_startup_lock(data_dir)?;
    if !try_flock_exclusive(&lockfile) {
        return Ok(());
    }
    match read_coord_file(data_dir) {
        Some(coord) if !coord.offline => retire_online_coord(&coord, data_dir, repo_dir),
        _ => Ok(()),
    }
}

/// Retires a coord file without `offline`, under the startup lock. A daemon
/// whose API answers with this repo's peer identity is asked to stop through
/// that API and is never signalled. One that has not stopped within
/// `KUBO_STOP_TIMEOUT`, or whose identity check gets no clear answer, keeps
/// its coord and the offline start is refused, so a second daemon never opens
/// the repo. Any other process (a reused PID with no API, a different repo's
/// Kubo) is left alone and only the coord goes.
fn retire_online_coord(coord: &CoordFile, data_dir: &Path, repo_dir: &Path) -> Result<(), String> {
    let pid = coord.kubo_pid;
    if is_pid_alive(pid) {
        match api_identity(coord.api_port, repo_dir) {
            ApiIdentity::ThisRepo => {
                eprintln!("ipfs-provider: stopping online Kubo pid={pid} through its API; it restarts offline");
                let _ = ureq::post(&kubo_rpc_url(coord.api_port, "shutdown", &[]))
                    .timeout(KUBO_API_CHECK_TIMEOUT)
                    .call();
                let deadline = Instant::now() + KUBO_STOP_TIMEOUT;
                while !online_kubo_stopped(coord) {
                    if Instant::now() >= deadline {
                        return Err(migration_refused(format!(
                            "online Kubo pid {pid} did not stop through its API"
                        )));
                    }
                    std::thread::sleep(KUBO_STOP_POLL_INTERVAL);
                }
            }
            ApiIdentity::Unknown => {
                return Err(migration_refused(format!(
                    "online Kubo pid {pid} gave no clear answer to its identity check"
                )));
            }
            ApiIdentity::Other => {
                eprintln!("ipfs-provider: pid {pid} is not this repo's Kubo; left running");
            }
        }
    }
    remove_coord_file(data_dir);
    Ok(())
}

/// Logs a migration that cannot finish now; its coord stays for the next try.
fn migration_refused(reason: String) -> String {
    let error = format!("{reason}; offline start refused");
    eprintln!("ipfs-provider: {error}");
    error
}

enum ApiIdentity {
    /// The API answered `/id` with this repo's peer identity.
    ThisRepo,
    /// Not this repo's daemon: the repo has no identity, nothing listens on
    /// the API port, or the API answered with another identity.
    Other,
    /// A timeout, an HTTP error or an unreadable answer.
    Unknown,
}

/// What the Kubo API on `api_port` says about `repo_dir`'s peer identity.
fn api_identity(api_port: u16, repo_dir: &Path) -> ApiIdentity {
    let Some(repo_peer) = fs::read(repo_dir.join("config"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|config| config["Identity"]["PeerID"].as_str().map(str::to_owned))
    else {
        return ApiIdentity::Other;
    };
    let response = match ureq::post(&kubo_rpc_url(api_port, "id", &[]))
        .timeout(KUBO_API_CHECK_TIMEOUT)
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Transport(error)) if error.kind() == ureq::ErrorKind::ConnectionFailed => {
            return ApiIdentity::Other;
        }
        Err(_) => return ApiIdentity::Unknown,
    };
    match response
        .into_json::<serde_json::Value>()
        .ok()
        .and_then(|id| id["ID"].as_str().map(str::to_owned))
    {
        Some(api_peer) if api_peer == repo_peer => ApiIdentity::ThisRepo,
        Some(_) => ApiIdentity::Other,
        None => ApiIdentity::Unknown,
    }
}

/// Whether an online daemon asked to shut down is gone: its API refuses
/// connections and its PID no longer runs.
fn online_kubo_stopped(coord: &CoordFile) -> bool {
    let api = std::net::SocketAddr::from(([127, 0, 0, 1], coord.api_port));
    std::net::TcpStream::connect_timeout(&api, KUBO_STOP_POLL_INTERVAL).is_err()
        && !is_pid_alive(coord.kubo_pid)
}

/// The largest file any whole read may return (one shared constant with
/// Runtime's Carrier transfers).
const WHOLE_CONTENT_CEILING: u64 = elastos_common::MAX_WHOLE_CONTENT_BYTES as u64;

// Starts every refusal of content past a whole read's limit.
use elastos_common::WHOLE_CONTENT_TOO_LARGE;

/// Read one whole file into memory, refusing it once it passes `limit`, before
/// more than `limit` bytes are buffered.
fn read_whole_content(reader: impl Read, source: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{source} -> {e}"))?;
    if bytes.len() as u64 > limit {
        return Err(format!("{WHOLE_CONTENT_TOO_LARGE} of {limit} bytes"));
    }
    Ok(bytes)
}

/// Kubo RPC URL for `command` (for example `pin/add`), each `(name, value)`
/// appended as one form-encoded query pair, so a CID or path in a value can
/// never add, end or change another parameter.
fn kubo_rpc_url(api_port: u16, command: &str, query: &[(&str, &str)]) -> String {
    let mut url = local_backend_url(api_port);
    url.path_segments_mut()
        .expect("http URL has a path")
        .clear()
        .extend(["api", "v0"])
        .extend(command.split('/'));
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    url.into()
}

fn local_backend_url(port: u16) -> url::Url {
    let mut url = url::Url::parse("http://127.0.0.1/").expect("static local URL");
    url.set_port(Some(port)).expect("http URL takes a port");
    url
}

// ── Path safety ─────────────────────────────────────────────────────

fn copy_reader_to_path(mut reader: impl Read, dest: &Path) -> Result<u64, String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create dest parent -> {e}"))?;
    }
    let mut tmp_name = dest.as_os_str().to_os_string();
    tmp_name.push(".part");
    let tmp = PathBuf::from(tmp_name);
    let copied = (|| {
        let mut file = fs::File::create(&tmp).map_err(|e| format!("create dest -> {e}"))?;
        let mut buf = vec![0u8; CAT_TO_PATH_CHUNK];
        let mut written = 0u64;
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| format!("read cat body -> {e}"))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])
                .map_err(|e| format!("write dest -> {e}"))?;
            written += n as u64;
        }
        file.sync_all().map_err(|e| format!("sync dest -> {e}"))?;
        Ok(written)
    })();
    match copied {
        Ok(written) => {
            fs::rename(&tmp, dest).map_err(|e| {
                let _ = fs::remove_file(&tmp);
                format!("commit dest -> {e}")
            })?;
            Ok(written)
        }
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            Err(err)
        }
    }
}

fn validate_dest_path(data_dir: &Path, dest: &Path) -> Result<(), String> {
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

    // Allowed prefixes. The destination path above is canonicalized, so the
    // roots must be canonicalized too (macOS: /tmp and $TMPDIR are symlinks
    // into /private).
    let data_dir = data_dir
        .canonicalize()
        .unwrap_or_else(|_| data_dir.to_path_buf());
    let tmp_dir = std::env::temp_dir();
    let tmp_dir = tmp_dir.canonicalize().unwrap_or(tmp_dir);

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
fn validate_source_path(data_dir: &Path, src: &Path) -> Result<(), String> {
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

    // The source path above is canonicalized, so the allowed roots must be
    // canonicalized too (macOS: /tmp and $TMPDIR are symlinks into /private).
    let data_dir = data_dir
        .canonicalize()
        .unwrap_or_else(|_| data_dir.to_path_buf());
    let tmp_dir = std::env::temp_dir();
    let tmp_dir = tmp_dir.canonicalize().unwrap_or(tmp_dir);

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

fn find_kubo_binary(data_dir: &Path) -> Option<PathBuf> {
    // 1. Explicit override
    if let Ok(path) = std::env::var("ELASTOS_IPFS_KUBO_PATH") {
        let p = PathBuf::from(&path);
        if p.is_file() {
            return Some(p);
        }
    }

    // 2. Standard install location
    let installed = data_dir.join("bin/kubo");
    if installed.is_file() {
        return Some(installed);
    }

    // 3. Managed runtimes keep provider/external binaries in the parent runtime bin dir.
    if let Some(dir) = std::env::var_os("ELASTOS_CAPSULE_BIN_DIR") {
        let installed = PathBuf::from(dir).join("kubo");
        if installed.is_file() {
            return Some(installed);
        }
    }

    // 4. System PATH (try both "kubo" and "ipfs")
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
    provider: &mut IpfsProvider,
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
                            if provider.ensure_kubo().is_err() {
                                continue;
                            }
                            let url = kubo_rpc_url(provider.api_port, "ls", &[("arg", hash)]);
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

fn spawn_idle_watcher(data_dir: PathBuf, activity: Arc<Mutex<()>>, role: HostRole) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(IDLE_CHECK_INTERVAL);
            // The provider keeps startup and active reads ahead of an idle stop.
            let _activity = activity.lock().unwrap();
            if let Some(coord) = read_coord_file(&data_dir) {
                let idle_secs = now_unix_secs().saturating_sub(coord.last_used);
                if role.idle_stop_due(coord.last_used, now_unix_secs()) {
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
                }
            }
        }
    });
}

// ── Main loop ───────────────────────────────────────────────────────

fn main() {
    eprintln!("ipfs-provider: starting v{}", PROVIDER_VERSION);

    let mut provider = IpfsProvider::new();

    let activity = Arc::new(Mutex::new(()));
    let mut idle_watcher_started = false;

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

        let request: Request = match parse_request(&line) {
            Ok(req) => req,
            Err(response) => {
                writeln!(stdout, "{}", serde_json::to_string(&response).unwrap()).unwrap();
                stdout.flush().unwrap();
                continue;
            }
        };

        let is_shutdown = matches!(request, Request::Shutdown);
        let _activity = activity.lock().unwrap();
        let response = provider.handle(request);
        if !idle_watcher_started
            && provider.state == KuboState::Ready
            && provider.host_role == HostRole::User
        {
            // Init has selected the data root before this daemon becomes ready.
            spawn_idle_watcher(
                provider.data_dir.clone(),
                Arc::clone(&activity),
                provider.host_role,
            );
            idle_watcher_started = true;
        }

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

    const ADD_PATH_RESPONSE: &[u8] =
        b"{\"Name\":\"release.tar.gz\",\"Hash\":\"QmStreamedFixture\",\"Size\":\"17\"}\n";

    fn add_http_fixture(
        status: u16,
        response: &'static [u8],
    ) -> (u16, std::thread::JoinHandle<(String, Vec<u8>)>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let mut socket = accept_bounded_fixture(&listener);
            let headers = read_bounded_fixture_headers(&mut socket);
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|value| value.trim().parse().unwrap())
                })
                .unwrap();
            assert!(length < 4 * ADD_PATH_CHUNK);
            let mut body = vec![0; length];
            socket.read_exact(&mut body).unwrap();
            write!(
                socket,
                "HTTP/1.1 {} fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                status,
                response.len()
            )
            .unwrap();
            socket.write_all(response).unwrap();
            (headers, body)
        });
        (port, server)
    }

    #[test]
    fn add_path_streams_exact_multipart_and_keeps_single_file_pin_options() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("release.tar.gz");
        let payload: Vec<u8> = (0..2 * ADD_PATH_CHUNK + 17)
            .map(|n| (n % 251) as u8)
            .collect();
        fs::write(&path, &payload).unwrap();
        for pin in [true, false] {
            let (port, server) = add_http_fixture(200, ADD_PATH_RESPONSE);
            let provider = bounded_cat_fixture_provider(root.path(), port);
            assert_eq!(
                provider.kubo_add_path(&path, pin).unwrap(),
                "QmStreamedFixture"
            );
            let (headers, body) = server.join().unwrap();
            assert!(headers.starts_with(&format!("POST /api/v0/add?pin={} HTTP/1.1\r\n", pin)));
            assert!(!headers.to_ascii_lowercase().contains("transfer-encoding:"));
            let content_type = headers
                .lines()
                .find(|line| line.to_ascii_lowercase().starts_with("content-type:"))
                .unwrap();
            let boundary = content_type.split("boundary=").nth(1).unwrap();
            assert!(content_type.contains("multipart/form-data; boundary=----elastos"));
            let prefix = format!("--{}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"release.tar.gz\"\r\nContent-Type: application/octet-stream\r\n\r\n", boundary).into_bytes();
            let footer = format!("\r\n--{}--\r\n", boundary).into_bytes();
            assert!(body.starts_with(&prefix) && body.ends_with(&footer));
            assert_eq!(body.len(), prefix.len() + payload.len() + footer.len());
            assert_eq!(
                &body[prefix.len()..body.len() - footer.len()],
                payload.as_slice()
            );
        }
    }

    #[test]
    fn add_path_stream_reader_bounds_reads_and_refuses_short_or_changed_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("payload");
        let payload = vec![b'x'; 2 * ADD_PATH_CHUNK + 17];
        fs::write(&path, &payload).unwrap();
        let mut reader = AddPathReader::new(fs::File::open(&path).unwrap()).unwrap();
        let mut buffer = vec![0; 3 * ADD_PATH_CHUNK];
        assert_eq!(reader.read(&mut []).unwrap(), 0);
        let mut total = 0;
        loop {
            let count = reader.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            assert!(count <= ADD_PATH_CHUNK);
            assert_eq!(&buffer[..count], &payload[total..total + count]);
            total += count;
        }
        assert_eq!(total, payload.len());
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(payload.len() as u64 + 1)
            .unwrap();
        assert_eq!(
            reader.read(&mut buffer).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        fs::write(&path, &payload).unwrap();
        let mut reader = AddPathReader::new(fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(reader.read(&mut buffer).unwrap(), ADD_PATH_CHUNK);
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(1)
            .unwrap();
        assert_eq!(
            reader.read(&mut buffer).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn add_path_stream_request_failure_and_nonregular_source_are_errors() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("release.tar.gz");
        fs::write(&path, b"fixture payload").unwrap();
        let (port, server) = add_http_fixture(500, ADD_PATH_RESPONSE);
        let provider = bounded_cat_fixture_provider(root.path(), port);
        let error = provider.kubo_add_path(&path, true).unwrap_err();
        assert!(error.contains("IPFS add failed") && error.contains("500"));
        let (_, body) = server.join().unwrap();
        assert!(body
            .windows(b"fixture payload".len())
            .any(|part| part == b"fixture payload"));
        assert!(provider
            .kubo_add_path(root.path(), true)
            .unwrap_err()
            .contains("regular file"));
    }

    const DIRECTORY_ROOT_RESPONSE: &[u8] =
        b"{\"Name\":\"assets/data.bin\",\"Hash\":\"bafkfile\"}\n{\"Name\":\"\",\"Hash\":\"bafyroot\"}\n";

    fn multipart_boundary(headers: &str) -> String {
        headers
            .lines()
            .find_map(|line| line.split("boundary=").nth(1))
            .unwrap()
            .trim()
            .to_owned()
    }

    fn without_boundary(body: &[u8], boundary: &str) -> Vec<u8> {
        let mut kept = Vec::with_capacity(body.len());
        let mut rest = body;
        while let Some((&byte, tail)) = rest.split_first() {
            if rest.starts_with(boundary.as_bytes()) {
                rest = &rest[boundary.len()..];
            } else {
                kept.push(byte);
                rest = tail;
            }
        }
        kept
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn local_object_add_streams_the_parts_and_options_of_directory_add() {
        let root = tempfile::tempdir().unwrap();
        let object = root.path().join("object");
        fs::create_dir_all(object.join("assets")).unwrap();
        let files: [(&str, Vec<u8>); 3] = [
            ("index.html", b"<p>held</p>".to_vec()),
            (
                "assets/data.bin",
                (0..2 * ADD_PATH_CHUNK + 17)
                    .map(|n| (n % 251) as u8)
                    .collect(),
            ),
            ("_elastos_object.json", b"{\"files\":[]}".to_vec()),
        ];
        for (path, bytes) in &files {
            fs::write(object.join(path), bytes).unwrap();
        }
        let (port, server) = add_http_fixture(200, DIRECTORY_ROOT_RESPONSE);
        let provider = bounded_cat_fixture_provider(root.path(), port);
        let encoded = files
            .iter()
            .map(|(path, bytes)| DirFile {
                path: path.to_string(),
                data: BASE64.encode(bytes),
            })
            .collect();
        assert_eq!(
            provider.kubo_add_directory(encoded, false).unwrap(),
            "bafyroot"
        );
        let (json_headers, json_body) = server.join().unwrap();

        let (port, server) = add_http_fixture(200, DIRECTORY_ROOT_RESPONSE);
        let provider = bounded_cat_fixture_provider(root.path(), port);
        let local = LocalObject {
            root: object,
            files: files
                .iter()
                .map(|(path, bytes)| StagedFile {
                    path: path.to_string(),
                    size: bytes.len() as u64,
                })
                .collect(),
        };
        assert!(local.valid_shape());
        assert_eq!(provider.kubo_add_local_object(&local).unwrap(), "bafyroot");
        let (headers, body) = server.join().unwrap();
        // The same add options and parts, apart from the random boundary, give
        // Kubo the same directory, so the CID equals the original JSON import.
        // Runtime pins only after comparing it.
        assert_eq!(headers.lines().next(), json_headers.lines().next());
        assert!(headers.lines().next().unwrap().contains("&pin=false&"));
        assert!(!headers.to_ascii_lowercase().contains("transfer-encoding:"));
        assert_eq!(
            without_boundary(&body, &multipart_boundary(&headers)),
            without_boundary(&json_body, &multipart_boundary(&json_headers))
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn local_object_add_refuses_unsafe_descriptors_links_and_changed_files() {
        let root = tempfile::tempdir().unwrap();
        let object = root.path().join("object");
        fs::create_dir(&object).unwrap();
        fs::write(object.join("a.txt"), b"abc").unwrap();
        fs::write(object.join("_elastos_object.json"), b"{}").unwrap();
        let mut provider = bounded_cat_fixture_provider(root.path(), 0);
        let refused = serde_json::to_value(private_preparation_error()).unwrap();
        let manifest = serde_json::json!({"path":"_elastos_object.json","size":2});
        let request = |root: serde_json::Value, files: serde_json::Value| serde_json::json!({"op":"runtime_add_local_object","object":{"root":root,"files":files}});
        let mut requests: Vec<_> = [
            serde_json::json!([]),
            serde_json::json!([{"path":"a.txt","size":3}]),
            serde_json::json!([{"path":"../a.txt","size":3}, manifest]),
            serde_json::json!([{"path":"a\"b","size":3}, manifest]),
            serde_json::json!([{"path":"a\r\nb","size":3}, manifest]),
            serde_json::json!([{"path":"a.txt","size":3}, {"path":"a.txt/b","size":1}, manifest]),
            serde_json::json!([manifest, manifest]),
        ]
        .into_iter()
        .map(|files| request(serde_json::json!(object), files))
        .collect();
        requests.push(request(
            serde_json::json!("object"),
            serde_json::json!([manifest]),
        ));
        let mut extra = request(serde_json::json!(object), serde_json::json!([manifest]));
        extra["pin"] = serde_json::json!(false);
        requests.push(extra);
        for request in requests {
            let response = match parse_request(&request.to_string()) {
                Ok(parsed) => provider.handle(parsed),
                Err(response) => response,
            };
            assert_eq!(
                serde_json::to_value(response).unwrap(),
                refused,
                "{request}"
            );
        }
        // Roots and files the import cannot trust are refused before anything
        // reaches Kubo: this fixture has no backend, so a sent request would
        // fail differently.
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret"), b"abc").unwrap();
        fs::write(outside.path().join("_elastos_object.json"), b"{}").unwrap();
        fs::create_dir(outside.path().join("dir")).unwrap();
        fs::write(outside.path().join("dir/a.txt"), b"abc").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), object.join("linked.txt"))
            .unwrap();
        std::os::unix::fs::symlink(outside.path().join("dir"), object.join("linked")).unwrap();
        let linked_root = root.path().join("linked-root");
        std::os::unix::fs::symlink(outside.path(), &linked_root).unwrap();
        for (object_root, path, size, refusal) in [
            (&object, "a.txt", 4, "Local object file refused"),
            (&object, "linked.txt", 3, "Local object file refused"),
            (&object, "linked/a.txt", 3, "Local object file refused"),
            (
                &outside.path().to_path_buf(),
                "secret",
                3,
                "Local object root refused",
            ),
            (&linked_root, "secret", 3, "Local object root refused"),
        ] {
            let candidate = LocalObject {
                root: object_root.clone(),
                files: vec![
                    StagedFile {
                        path: path.into(),
                        size,
                    },
                    StagedFile {
                        path: "_elastos_object.json".into(),
                        size: 2,
                    },
                ],
            };
            assert!(candidate.valid_shape());
            let error = provider.kubo_add_local_object(&candidate).unwrap_err();
            assert!(error.starts_with(refusal), "{path}: {error}");
        }
    }

    fn private_hash_fixture_request() -> serde_json::Value {
        serde_json::json!({"op":"runtime_hash_staged_directory","directory":{
            "root":"/private/fixture",
            "files":[{"path":"capsule.json","size":2},{"path":"_elastos_object.json","size":2},{"path":"weights.gguf","size":4}],
            "total_bytes":8,
        }})
    }

    #[test]
    fn private_ipfs_capacity_request_is_strict_and_cold_is_unavailable() {
        for wire in [
            serde_json::json!({"op":"runtime_check_capacity"}),
            serde_json::json!({"op":"runtime_check_capacity","required_bytes":null}),
            serde_json::json!({"op":"runtime_check_capacity","required_bytes":-1}),
            serde_json::json!({"op":"runtime_check_capacity","required_bytes":"private-value"}),
            serde_json::json!({"op":"runtime_check_capacity","required_bytes":1,"path":"private-value"}),
        ] {
            let error = parse_request(&wire.to_string()).unwrap_err();
            assert_eq!(
                serde_json::to_value(error).unwrap(),
                serde_json::to_value(private_preparation_error()).unwrap()
            );
        }
        let root = tempfile::tempdir().unwrap();
        let mut provider = bounded_cat_fixture_provider(root.path(), 0);
        provider.state = KuboState::Cold;
        for required_bytes in [0, 1, MAX_CAPACITY_REQUIRED_BYTES + 1, u64::MAX] {
            let wire =
                serde_json::json!({"op":"runtime_check_capacity","required_bytes":required_bytes});
            assert!(wire.to_string().len() < 128);
            let response = provider.handle(parse_request(&wire.to_string()).unwrap());
            assert_eq!(
                serde_json::to_value(response).unwrap(),
                serde_json::to_value(private_preparation_error()).unwrap()
            );
        }
        assert_eq!(provider.state, KuboState::Cold);
        assert!(provider.kubo_child.is_none());
        assert!(!provider.repo_dir.exists());
    }

    #[test]
    fn private_ipfs_descriptor_is_strict_and_failure_is_sanitized() {
        for (pointer, value) in [
            ("/directory/root", serde_json::json!(42)),
            (
                "/directory/files/0/size",
                serde_json::json!("private-value"),
            ),
            ("/directory/total_bytes", serde_json::json!(-1)),
        ] {
            let mut wire = private_hash_fixture_request();
            *wire.pointer_mut(pointer).unwrap() = value;
            let err = parse_request(&wire.to_string()).unwrap_err();
            assert_eq!(
                serde_json::to_value(err).unwrap(),
                serde_json::to_value(private_preparation_error()).unwrap()
            );
        }
        for pointer in ["", "/directory", "/directory/files/0"] {
            let mut wire = private_hash_fixture_request();
            wire.pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("expected_cid".into(), serde_json::json!("private-value"));
            assert!(parse_request(&wire.to_string()).is_err());
        }
        assert!(
            parse_request(r#"{"op":"runtime_prepare_backend","path":"private-value"}"#).is_err()
        );
        let root = tempfile::tempdir().unwrap();
        let mut provider = IpfsProvider {
            host_role: crate::HostRole::User,
            initialized: true,
            state: KuboState::Cold,
            api_port: 0,
            gateway_port: 0,
            kubo_binary: None,
            kubo_child: None,
            data_dir: root.path().join("absent"),
            repo_dir: root.path().join("absent-repo"),
        };
        for (pointer, value) in [
            ("/directory/root", serde_json::json!("relative")),
            ("/directory/files/0/path", serde_json::json!("../secret")),
            ("/directory/files", serde_json::json!([])),
            ("/directory/files/0/size", serde_json::json!(u64::MAX)),
            ("/directory/total_bytes", serde_json::json!(9)),
        ] {
            let mut wire = private_hash_fixture_request();
            *wire.pointer_mut(pointer).unwrap() = value;
            let response = provider.handle(parse_request(&wire.to_string()).unwrap());
            assert_eq!(
                serde_json::to_value(response).unwrap(),
                serde_json::to_value(private_preparation_error()).unwrap()
            );
        }
        let response =
            provider.handle(parse_request(r#"{"op":"runtime_prepare_backend"}"#).unwrap());
        assert_eq!(
            serde_json::to_value(response).unwrap(),
            serde_json::to_value(private_preparation_error()).unwrap()
        );
        assert!(!provider.data_dir.exists());
        assert!(!provider.repo_dir.exists());
        assert_eq!(provider.state, KuboState::Cold);
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn private_ipfs_cold_readiness_precedes_bounded_read_without_prefetch() {
        let root = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut provider = bounded_cat_fixture_provider(root.path(), port);
        provider.state = KuboState::Cold;
        provider.api_port = 0;
        // Existing live coordination makes this binary unnecessary to execute.
        provider.kubo_binary = Some(root.path().join("must-not-execute"));
        assert!(matches!(
            provider.handle(serde_json::from_value(bounded_cat_fixture_request()).unwrap()),
            Response::Error { .. }
        ));
        assert!(matches!(listener.accept(), Err(e) if e.kind() == io::ErrorKind::WouldBlock));
        let backend = std::thread::spawn(move || {
            let mut version_socket = accept_bounded_fixture(&listener);
            let version_headers = read_bounded_fixture_headers(&mut version_socket);
            version_socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\n{\"Version\":\"0.40.1\"}").unwrap();
            drop(version_socket);
            let mut socket = accept_bounded_fixture(&listener);
            let headers = read_bounded_fixture_headers(&mut socket);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n89ab")
                .unwrap();
            (version_headers, headers)
        });
        let ready = provider.handle(parse_request(r#"{"op":"runtime_prepare_backend"}"#).unwrap());
        let result =
            provider.handle(serde_json::from_value(bounded_cat_fixture_request()).unwrap());
        let (version_headers, headers) = backend.join().unwrap();
        assert_eq!(
            serde_json::to_value(ready).unwrap(),
            serde_json::json!({"status":"ok"})
        );
        assert_eq!(provider.state, KuboState::Ready);
        assert!(version_headers.starts_with("POST /api/v0/version"));
        assert!(headers.starts_with("POST /api/v0/cat?"));
        assert!(headers.contains("offline=true"));
        assert!(headers.contains("timeout=100ms"));
        assert!(headers.contains("offset=8") && headers.contains("length=4"));
        let Response::Ok { data: Some(data) } = result else {
            panic!("bounded read failed");
        };
        assert_eq!(
            BASE64.decode(data["data"].as_str().unwrap()).unwrap(),
            b"89ab"
        );
        assert!(!provider.repo_dir.exists());
        assert!(provider.kubo_child.is_none());
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn private_ipfs_readiness_rejects_live_pid_with_dead_api() {
        let root = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let mut provider = bounded_cat_fixture_provider(root.path(), port);
        provider.state = KuboState::Cold;
        provider.kubo_binary = Some(root.path().join("must-not-execute"));
        let response =
            provider.handle(parse_request(r#"{"op":"runtime_prepare_backend"}"#).unwrap());
        assert_eq!(
            serde_json::to_value(response).unwrap(),
            serde_json::to_value(private_preparation_error()).unwrap()
        );
        assert_eq!(provider.state, KuboState::Error);
        assert!(provider.kubo_child.is_none());
        assert!(!provider.repo_dir.exists());
    }

    #[test]
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn private_ipfs_unsupported_platform_fails_without_startup() {
        let mut provider = IpfsProvider::new();
        for request in [
            private_hash_fixture_request(),
            serde_json::json!({"op":"runtime_prepare_backend"}),
            serde_json::json!({"op":"runtime_check_capacity","required_bytes":1}),
        ] {
            let response = provider.handle(parse_request(&request.to_string()).unwrap());
            assert_eq!(
                serde_json::to_value(response).unwrap(),
                serde_json::to_value(private_preparation_error()).unwrap()
            );
        }
        assert_eq!(provider.state, KuboState::Cold);
        assert!(provider.kubo_child.is_none());
    }

    fn bounded_cat_fixture_request() -> serde_json::Value {
        serde_json::json!({
            "op": "cat", "bounded_read": true,
            "cid": "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
            "path": "weights.gguf",
            "_runtime_invocation": {
                "schema": "elastos.provider.invocation/v1", "source": "content-provider",
                "target": "ipfs", "op": "cat", "transport": "runtime-local-provider-plane",
                "transfer": "bytes", "range": { "start": 8, "end": 11 },
                "progress": { "request_id": "range-fixture", "expected_bytes": 4 }
            }
        })
    }

    fn bounded_cat_fixture_provider(root: &Path, port: u16) -> IpfsProvider {
        write_coord_file(
            root,
            &CoordFile {
                kubo_pid: std::process::id(),
                api_port: port,
                gateway_port: port,
                started_at: 1,
                last_used: 1,
                offline: true,
            },
        );
        IpfsProvider {
            host_role: HostRole::User,
            initialized: true,
            state: KuboState::Ready,
            api_port: port,
            gateway_port: port,
            kubo_binary: None,
            kubo_child: None,
            data_dir: root.to_path_buf(),
            repo_dir: root.join("unused-repo"),
        }
    }

    #[test]
    fn bounded_read_after_idle_stop_refuses_within_deadline_without_startup() {
        for missing_coord in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let mut provider =
                bounded_cat_fixture_provider(root.path(), listener.local_addr().unwrap().port());
            provider.kubo_binary = Some(root.path().join("must-not-execute"));
            if missing_coord {
                remove_coord_file(root.path());
            } else {
                let mut coord = read_coord_file(root.path()).unwrap();
                coord.kubo_pid = 999999;
                assert!(!is_pid_alive(coord.kubo_pid));
                write_coord_file(root.path(), &coord);
            }
            let started = Instant::now();
            let response =
                provider.handle(serde_json::from_value(bounded_cat_fixture_request()).unwrap());
            assert!(started.elapsed() < BOUNDED_READ_TIMEOUT);
            assert!(
                matches!(response, Response::Error { code, .. } if code == "bounded_read_failed")
            );
            assert_eq!(provider.state, KuboState::Cold);
            assert!(provider.kubo_child.is_none());
            assert!(!provider.repo_dir.exists());
            assert!(matches!(listener.accept(), Err(e) if e.kind() == io::ErrorKind::WouldBlock));
        }
    }

    #[test]
    fn unbounded_read_refreshes_ready_backend_ports_from_coord() {
        let root = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut provider =
            bounded_cat_fixture_provider(root.path(), listener.local_addr().unwrap().port());
        provider.api_port = 0;
        provider.gateway_port = 0;
        let backend = std::thread::spawn(move || {
            let mut socket = accept_bounded_fixture(&listener);
            let headers = read_bounded_fixture_headers(&mut socket);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n89ab")
                .unwrap();
            headers
        });
        assert_eq!(
            provider
                .fetch_bytes("release-cid", WHOLE_CONTENT_CEILING)
                .unwrap(),
            b"89ab"
        );
        assert!(backend
            .join()
            .unwrap()
            .starts_with("POST /api/v0/cat?arg=release-cid"));
        assert_eq!(provider.state, KuboState::Ready);
        assert!(provider.kubo_child.is_none());
    }

    /// A Kubo double whose `served_by` request gets `answer`; every other
    /// request fails, so the read reaches that route. The thread returns the
    /// request lines and the body bytes `answer` managed to send.
    fn kubo_answering<F>(
        served_by: &'static str,
        answer: F,
    ) -> (u16, std::thread::JoinHandle<(Vec<String>, u64)>)
    where
        F: FnOnce(&mut std::net::TcpStream) -> u64 + Send + 'static,
    {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let serving = std::thread::spawn(move || {
            let mut requests = Vec::new();
            loop {
                let mut socket = accept_bounded_fixture(&listener);
                let headers = read_bounded_fixture_headers(&mut socket);
                requests.push(headers.lines().next().unwrap_or_default().to_string());
                if !headers.starts_with(served_by) {
                    socket
                        .write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .unwrap();
                    continue;
                }
                socket
                    .set_write_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let sent = answer(&mut socket);
                return (requests, sent);
            }
        });
        (port, serving)
    }

    /// Writes `total` bytes of `byte` in 1 MiB pieces until the reader stops.
    fn write_body(socket: &mut std::net::TcpStream, total: u64, byte: impl Fn(u64) -> u8) -> u64 {
        let mut sent = 0u64;
        while sent < total {
            let piece: Vec<u8> = (sent..total.min(sent + 1024 * 1024)).map(&byte).collect();
            if socket.write_all(&piece).is_err() {
                break;
            }
            sent += piece.len() as u64;
        }
        sent
    }

    #[test]
    fn whole_read_past_its_limit_is_refused_while_reading() {
        let limit = elastos_common::MAX_WHOLE_CONTENT_BYTES as u64;
        // The local Kubo read; a read carrying its own limit is in
        // `whole_bounded_cat_hit_past_its_limit_is_refused_while_reading`.
        let served_by = "POST /api/v0/cat";
        let root = tempfile::tempdir().unwrap();
        // One answer of three times the limit, with no declared length.
        let (port, backend) = kubo_answering(served_by, move |socket| {
            if socket
                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                .is_err()
            {
                return 0;
            }
            write_body(socket, 3 * limit, |_| b'a')
        });
        let mut provider = bounded_cat_fixture_provider(root.path(), port);
        let request = serde_json::json!({"op": "cat", "cid": "large-cid"});
        let response = provider.handle(serde_json::from_value(request).unwrap());
        let (requests, sent) = backend.join().unwrap();
        match &response {
            Response::Error { code, message } => {
                assert_eq!(code, "cat_failed");
                assert_eq!(
                    message,
                    &format!("{WHOLE_CONTENT_TOO_LARGE} of {limit} bytes")
                );
            }
            Response::Ok { .. } => panic!("content past the limit was returned"),
        }
        assert!(
            sent < 2 * limit,
            "the provider read on past the limit ({sent} bytes sent)"
        );
        assert_eq!(
            requests,
            [format!("{served_by}?arg=large-cid HTTP/1.1")],
            "a refused read is not repeated"
        );
    }

    #[test]
    fn whole_read_of_the_largest_release_artifact_succeeds() {
        // elastos-x86_64-linux in the 0.8.0-alpha.24 signed set.
        const SIZE: u64 = 115_118_616;
        let byte = |offset: u64| (offset.wrapping_mul(2_654_435_761) >> 13) as u8;
        let (port, backend) = kubo_answering("POST /api/v0/cat", move |socket| {
            let header =
                format!("HTTP/1.1 200 OK\r\nContent-Length: {SIZE}\r\nConnection: close\r\n\r\n");
            if socket.write_all(header.as_bytes()).is_err() {
                return 0;
            }
            write_body(socket, SIZE, byte)
        });
        let root = tempfile::tempdir().unwrap();
        let mut provider = bounded_cat_fixture_provider(root.path(), port);
        let response = provider.cat("release-cid", None, None);
        let (_, sent) = backend.join().unwrap();
        assert_eq!(sent, SIZE);
        let Response::Ok { data: Some(data) } = response else {
            panic!("release-sized read failed: {response:?}");
        };
        let bytes = BASE64.decode(data["data"].as_str().unwrap()).unwrap();
        assert_eq!(bytes.len() as u64, SIZE);
        assert!(bytes
            .iter()
            .enumerate()
            .all(|(offset, value)| *value == byte(offset as u64)));
    }

    #[test]
    #[cfg(unix)]
    fn immediate_recovery_read_reaps_delayed_previous_daemon_before_replacement() {
        let root = tempfile::tempdir().unwrap();
        let mut previous = Command::new("sh")
            .args(["-c", "trap 'sleep 1; exit 0' TERM; printf ready; read line"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut ready = [0; 5];
        previous
            .stdout
            .as_mut()
            .unwrap()
            .read_exact(&mut ready)
            .unwrap();
        assert_eq!(&ready, b"ready");
        let previous_pid = previous.id();
        let mut provider = bounded_cat_fixture_provider(root.path(), 0);
        provider.kubo_child = Some(previous);
        remove_coord_file(root.path());

        // The replacement fixture exposes only the startup health and cat calls.
        provider.kubo_binary = Some(kubo_fixture(root.path()));
        let started_path = provider.repo_dir.with_extension("started");
        let backend = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(12);
            let port = loop {
                if let Ok(address) = fs::read_to_string(&started_path) {
                    if let Some(port) = address
                        .rsplit('/')
                        .next()
                        .and_then(|value| value.parse::<u16>().ok())
                    {
                        break port;
                    }
                }
                assert!(Instant::now() < deadline, "replacement was not started");
                std::thread::sleep(Duration::from_millis(10));
            };
            let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let mut health = accept_bounded_fixture(&listener);
            let health_headers = read_bounded_fixture_headers(&mut health);
            health
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .unwrap();
            drop(health);
            let mut cat = accept_bounded_fixture(&listener);
            let cat_headers = read_bounded_fixture_headers(&mut cat);
            cat.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n89ab")
                .unwrap();
            (health_headers, cat_headers)
        });

        assert_eq!(unsafe { libc::kill(previous_pid as i32, libc::SIGTERM) }, 0);
        let started = Instant::now();
        let result = provider.fetch_bytes("release-cid", WHOLE_CONTENT_CEILING);
        let elapsed = started.elapsed();
        let previous_reaped = !is_pid_alive(previous_pid);
        if let Some(mut replacement) = provider.kubo_child.take() {
            let _ = replacement.kill();
            replacement.wait().unwrap();
        }
        // Settle the previous fixture even when a regression loses its Child handle.
        unsafe { libc::waitpid(previous_pid as i32, std::ptr::null_mut(), 0) };
        let (health_headers, cat_headers) = backend.join().unwrap();
        assert_eq!(result.unwrap(), b"89ab");
        assert!(elapsed >= Duration::from_secs(1));
        assert!(
            previous_reaped,
            "previous Child was replaced before it was reaped"
        );
        assert!(health_headers.starts_with("POST /api/v0/id"));
        assert!(cat_headers.starts_with("POST /api/v0/cat?arg=release-cid"));
    }

    #[test]
    #[cfg(unix)]
    fn previous_daemon_is_force_stopped_and_reaped_within_stop_bound() {
        let root = tempfile::tempdir().unwrap();
        let previous = Command::new("sleep").arg("60").spawn().unwrap();
        let previous_pid = previous.id();
        let mut provider = bounded_cat_fixture_provider(root.path(), 0);
        provider.kubo_child = Some(previous);
        let started = Instant::now();
        let result = provider.reap_previous_kubo();
        let elapsed = started.elapsed();
        if let Some(mut remaining) = provider.kubo_child.take() {
            let _ = remaining.kill();
            remaining.wait().unwrap();
        }
        result.unwrap();
        assert!(elapsed >= KUBO_STOP_TIMEOUT);
        assert!(elapsed < KUBO_STOP_TIMEOUT * 2 + LOCKFILE_POLL_INTERVAL);
        assert!(!is_pid_alive(previous_pid));
    }

    /// A Kubo stand-in: `init` writes a config with an identity, `daemon`
    /// records its arguments and API address, then sleeps; others succeed.
    #[cfg(unix)]
    fn kubo_fixture(root: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        // At `<data>/bin/kubo`, where Init finds the managed Kubo.
        fs::create_dir_all(root.join("bin")).unwrap();
        let kubo = root.join("bin/kubo");
        fs::write(
            &kubo,
            "#!/bin/sh\ncase \"$1\" in\n\
             init) mkdir -p \"$IPFS_PATH\" && printf '%s' '{\"Identity\":{\"PeerID\":\"fixture-peer\"},\"Bootstrap\":[\"auto\"],\"DNS\":{\"Resolvers\":{\".\":\"auto\",\"eth.\":\"https://dns.example/dns-query\"}}}' > \"$IPFS_PATH/config\";;\n\
             daemon) printf '%s' \"$*\" > \"$IPFS_PATH.args\"; printf '%s' \"$4\" > \"$IPFS_PATH.started\"; sleep 60 & wait;;\n\
             *) exit 0;;\nesac\n",
        )
        .unwrap();
        fs::set_permissions(&kubo, fs::Permissions::from_mode(0o700)).unwrap();
        kubo
    }

    /// Answers the health call of the daemon `kubo_fixture` starts for
    /// `repo_dir`; returns its API port.
    fn answer_fixture_health(repo_dir: PathBuf) -> std::thread::JoinHandle<u16> {
        std::thread::spawn(move || {
            let started_path = repo_dir.with_extension("started");
            let deadline = Instant::now() + Duration::from_secs(12);
            let port = loop {
                if let Some(port) = fs::read_to_string(&started_path)
                    .ok()
                    .and_then(|address| address.rsplit('/').next()?.parse::<u16>().ok())
                {
                    break port;
                }
                assert!(Instant::now() < deadline, "offline daemon was not started");
                std::thread::sleep(Duration::from_millis(10));
            };
            let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let mut health = accept_bounded_fixture(&listener);
            assert!(read_bounded_fixture_headers(&mut health).starts_with("POST /api/v0/id"));
            health
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .unwrap();
            port
        })
    }

    fn cold_fixture_provider(root: &Path) -> IpfsProvider {
        let mut provider = bounded_cat_fixture_provider(root, 0);
        remove_coord_file(root);
        provider.state = KuboState::Cold;
        provider.repo_dir = root.join("ipfs-repo");
        provider.api_port = 0;
        provider.gateway_port = 0;
        provider
    }

    /// Stops the daemon a fixture provider started.
    fn stop_fixture_daemon(provider: &mut IpfsProvider) {
        if let Some(mut child) = provider.kubo_child.take() {
            let _ = child.kill();
            child.wait().unwrap();
        }
    }

    /// An online daemon an earlier release started: the Kubo stand-in runs
    /// `daemon --api` on its own repo, its API answers `/id` with `peer`, and
    /// the coord file has no `offline`. The API thread owns the stand-in and
    /// reaps it once stopped, so its PID is then gone. With
    /// `honours_shutdown`, `/api/v0/shutdown` stops it and closes the API, as
    /// Kubo does; without, the call is answered and ignored.
    fn online_daemon_coord(
        root: &Path,
        kubo: &Path,
        peer: &'static str,
        honours_shutdown: bool,
    ) -> OnlineDaemon {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let api_port = listener.local_addr().unwrap().port();
        let mut daemon = Command::new(kubo)
            .args(["daemon", "--api", &format!("/ip4/127.0.0.1/tcp/{api_port}")])
            .env("IPFS_PATH", root.join("old-daemon"))
            .spawn()
            .unwrap();
        let pid = daemon.id();
        write_old_coord(root, pid, api_port);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let api = std::thread::spawn(move || {
            while !stopping.load(std::sync::atomic::Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        // The migration's stop probe connects and closes
                        // without a request; it gets no answer.
                        let Some(headers) = read_fixture_request(&mut socket) else {
                            continue;
                        };
                        let shutdown = headers.starts_with("POST /api/v0/shutdown");
                        let body = if shutdown {
                            String::new()
                        } else {
                            format!("{{\"ID\":\"{peer}\"}}")
                        };
                        let _ = socket.write_all(
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            )
                            .as_bytes(),
                        );
                        if shutdown && honours_shutdown {
                            break;
                        }
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
            drop(listener);
            let _ = daemon.kill();
            daemon.wait().unwrap();
        });
        OnlineDaemon { pid, stop, api }
    }

    /// A request's headers, or `None` for a connection closed before them.
    fn read_fixture_request(socket: &mut std::net::TcpStream) -> Option<String> {
        socket.set_nonblocking(false).ok()?;
        socket.set_read_timeout(Some(Duration::from_secs(1))).ok()?;
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") && bytes.len() < 4096 {
            let mut byte = [0];
            socket.read_exact(&mut byte).ok()?;
            bytes.push(byte[0]);
        }
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    struct OnlineDaemon {
        pid: u32,
        stop: Arc<std::sync::atomic::AtomicBool>,
        api: std::thread::JoinHandle<()>,
    }

    impl OnlineDaemon {
        fn running(&self) -> bool {
            is_pid_alive(self.pid)
        }

        /// Stops the stand-in if it still runs and closes its API.
        fn finish(self) {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            self.api.join().unwrap();
        }
    }

    fn write_old_coord(root: &Path, pid: u32, api_port: u16) {
        fs::write(
            coord_file_path(root),
            serde_json::to_vec(&serde_json::json!({
                "kubo_pid": pid, "api_port": api_port, "gateway_port": api_port,
                "started_at": 1, "last_used": now_unix_secs(),
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn kubo_daemon_runs_offline() {
        assert_eq!(
            kubo_daemon_args(5001),
            [
                "daemon",
                "--offline",
                "--api",
                "/ip4/127.0.0.1/tcp/5001",
                "--enable-gc"
            ]
        );
    }

    #[test]
    fn local_store_config_neither_announces_nor_routes() {
        // As `kubo init` 0.40.1 writes it, with its AutoConf placeholders.
        let mut config = serde_json::json!({
            "Identity": {"PeerID": "12D3KooWfixture", "PrivKey": "secret"},
            "Addresses": {"API": "/ip4/127.0.0.1/tcp/5001", "Gateway": "/ip4/127.0.0.1/tcp/8080",
                "Swarm": ["/ip4/0.0.0.0/tcp/4001", "/ip6/::/udp/4001/quic-v1"]},
            "Bootstrap": ["auto"],
            "DNS": {"Resolvers": {".": "auto", "eth.": "https://dns.example/dns-query"}},
            "Ipns": {"DelegatedPublishers": ["auto"], "ResolveCacheSize": 128},
            "Routing": {"DelegatedRouters": ["auto"]},
            "Provide": {"DHT": {}},
            "Discovery": {"MDNS": {"Enabled": true}},
            "AutoConf": {},
            "Plugins": {"Plugins": null},
            "Datastore": {"StorageMax": "10GB"},
        });
        apply_local_store_config(&mut config, 4242).unwrap();
        let expected = serde_json::json!({
            "Identity": {"PeerID": "12D3KooWfixture", "PrivKey": "secret"},
            "Addresses": {"API": "/ip4/127.0.0.1/tcp/5001", "Gateway": "/ip4/127.0.0.1/tcp/4242",
                "Swarm": []},
            "Bootstrap": [],
            "DNS": {"Resolvers": {"eth.": "https://dns.example/dns-query"}},
            "Ipns": {"DelegatedPublishers": [], "ResolveCacheSize": 128},
            "Routing": {"DelegatedRouters": [], "Type": "none"},
            "Provide": {"DHT": {}, "Enabled": false},
            "Discovery": {"MDNS": {"Enabled": false}},
            "AutoConf": {"Enabled": false},
            "Plugins": {"Plugins": {"telemetry": {"Config": {"Mode": "off"}}}},
            "Datastore": {"StorageMax": "10GB"},
        });
        assert_eq!(config, expected);
        // A repeated start changes only the gateway port.
        apply_local_store_config(&mut config, 4243).unwrap();
        assert_eq!(config["Addresses"]["Gateway"], "/ip4/127.0.0.1/tcp/4243");
        assert!(apply_local_store_config(&mut serde_json::json!({"Routing": 1}), 1).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn started_daemon_is_offline_on_a_local_store_config() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let mut provider = cold_fixture_provider(root.path());
        provider.kubo_binary = Some(kubo_fixture(root.path()));
        let health = answer_fixture_health(provider.repo_dir.clone());
        let result = provider.ensure_kubo();
        let args = fs::read_to_string(provider.repo_dir.with_extension("args"));
        stop_fixture_daemon(&mut provider);
        result.unwrap();
        let port = health.join().unwrap();
        assert_eq!(
            args.unwrap(),
            format!("daemon --offline --api /ip4/127.0.0.1/tcp/{port} --enable-gc")
        );
        let coord = read_coord_file(root.path()).unwrap();
        assert!(coord.offline);
        assert_eq!(coord.api_port, port);
        let config_path = provider.repo_dir.join("config");
        let config: serde_json::Value =
            serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        assert_eq!(config["Identity"]["PeerID"], "fixture-peer");
        assert_eq!(config["Routing"]["Type"], "none");
        assert_eq!(config["Provide"]["Enabled"], false);
        assert_eq!(config["Bootstrap"], serde_json::json!([]));
        assert_eq!(
            config["Addresses"]["Gateway"],
            format!("/ip4/127.0.0.1/tcp/{}", coord.gateway_port)
        );
        assert_eq!(
            fs::metadata(&config_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    /// An update leaves the online daemon of an earlier release running. Every
    /// path that borrows a running daemon must stop it and start the same repo
    /// offline instead.
    #[test]
    #[cfg(unix)]
    fn online_daemon_is_stopped_and_the_repo_restarted_offline() {
        for path in ["init", "ready reuse", "lock acquisition"] {
            let root = tempfile::tempdir().unwrap();
            let mut provider = cold_fixture_provider(root.path());
            provider.kubo_binary = Some(kubo_fixture(root.path()));
            fs::create_dir_all(provider.repo_dir.join("blocks")).unwrap();
            fs::write(provider.repo_dir.join("blocks/held.data"), b"held block").unwrap();
            fs::write(
                provider.repo_dir.join("config"),
                br#"{"Identity":{"PeerID":"kept"}}"#,
            )
            .unwrap();
            let kubo = provider.kubo_binary.clone().unwrap();
            let online = online_daemon_coord(root.path(), &kubo, "kept", true);
            match path {
                "init" => {
                    provider.initialized = false;
                    let config = serde_json::json!({"base_path": root.path()});
                    assert!(matches!(provider.init(config), Response::Ok { .. }));
                    // Init stops it at once; the next operation starts offline.
                    assert_eq!(provider.state, KuboState::Cold, "{path}");
                    assert!(read_coord_file(root.path()).is_none(), "{path}");
                    assert!(!online.running(), "{path}: still running");
                    // Init may find this machine's own Kubo; keep the stand-in.
                    provider.kubo_binary = Some(kubo_fixture(root.path()));
                }
                "ready reuse" => {
                    provider.state = KuboState::Ready;
                    provider.api_port = 1;
                }
                _ => {}
            }
            let health = answer_fixture_health(provider.repo_dir.clone());
            let result = provider.ensure_kubo();
            stop_fixture_daemon(&mut provider);
            let online_exited = !online.running();
            let online_pid = online.pid;
            online.finish();
            result.unwrap_or_else(|e| panic!("{path}: {e}"));
            let port = health.join().unwrap();
            assert!(
                online_exited,
                "{path}: online daemon {online_pid} kept running"
            );
            let coord = read_coord_file(root.path()).unwrap();
            assert!(coord.offline && coord.api_port == port, "{path}: {coord:?}");
            assert_eq!(provider.api_port, port, "{path}");
            assert!(fs::read_to_string(provider.repo_dir.with_extension("args"))
                .unwrap()
                .contains("--offline"));
            // The repo survives: blocks untouched, identity kept.
            assert_eq!(
                fs::read(provider.repo_dir.join("blocks/held.data")).unwrap(),
                b"held block"
            );
            let config: serde_json::Value =
                serde_json::from_slice(&fs::read(provider.repo_dir.join("config")).unwrap())
                    .unwrap();
            assert_eq!(config["Identity"]["PeerID"], "kept", "{path}");
        }
    }

    /// While another process holds the startup lock, a waiting provider never
    /// borrows the online daemon; it takes the offline one that process starts.
    #[test]
    #[cfg(unix)]
    fn lock_waiter_borrows_only_an_offline_daemon() {
        let root = tempfile::tempdir().unwrap();
        let mut provider = cold_fixture_provider(root.path());
        provider.kubo_binary = Some(root.path().join("must-not-execute"));
        let held = open_startup_lock(root.path()).unwrap();
        assert!(try_flock_exclusive(&held));
        let mut online = Command::new("sleep").arg("60").spawn().unwrap();
        write_old_coord(root.path(), online.id(), 1);
        let data_dir = root.path().to_path_buf();
        let starter = std::thread::spawn(move || {
            std::thread::sleep(LOCKFILE_POLL_INTERVAL * 3);
            write_coord_file(
                &data_dir,
                &CoordFile {
                    kubo_pid: std::process::id(),
                    api_port: 2,
                    gateway_port: 3,
                    started_at: 1,
                    last_used: 1,
                    offline: true,
                },
            );
            drop(held);
        });
        let result = provider.ensure_kubo();
        starter.join().unwrap();
        let _ = online.kill();
        online.wait().unwrap();
        result.unwrap();
        assert_eq!((provider.api_port, provider.gateway_port), (2, 3));
    }

    /// A coord whose PID now names some other process (the PID was reused, or
    /// it is a different repo's Kubo) is dropped without signalling it.
    #[test]
    #[cfg(unix)]
    fn unrelated_process_in_an_old_coord_is_never_signalled() {
        for case in ["not kubo", "other repo"] {
            let root = tempfile::tempdir().unwrap();
            let mut provider = cold_fixture_provider(root.path());
            let kubo = kubo_fixture(root.path());
            provider.kubo_binary = Some(kubo.clone());
            fs::create_dir_all(&provider.repo_dir).unwrap();
            fs::write(
                provider.repo_dir.join("config"),
                br#"{"Identity":{"PeerID":"mine"}}"#,
            )
            .unwrap();
            let (sleeper, online) = if case == "not kubo" {
                let sleeper = Command::new("sleep").arg("60").spawn().unwrap();
                write_old_coord(root.path(), sleeper.id(), 1);
                (Some(sleeper), None)
            } else {
                let online = online_daemon_coord(root.path(), &kubo, "someone-else", true);
                (None, Some(online))
            };
            let other_pid = read_coord_file(root.path()).unwrap().kubo_pid;
            let health = answer_fixture_health(provider.repo_dir.clone());
            let result = provider.ensure_kubo();
            stop_fixture_daemon(&mut provider);
            let other_running = is_pid_alive(other_pid);
            if let Some(mut sleeper) = sleeper {
                let _ = sleeper.kill();
                sleeper.wait().unwrap();
            }
            if let Some(online) = online {
                online.finish();
            }
            result.unwrap_or_else(|e| panic!("{case}: {e}"));
            health.join().unwrap();
            assert!(other_running, "{case}: an unrelated process was signalled");
            assert!(read_coord_file(root.path()).unwrap().offline, "{case}");
        }
    }

    /// A daemon that answers `/id` with this repo's identity but ignores
    /// `/shutdown` is never signalled: it keeps running and keeps its coord,
    /// Init still finishes inside the bridge's 10 s limit, and the offline
    /// start is refused, so no second daemon opens the repo.
    #[test]
    #[cfg(unix)]
    fn online_daemon_ignoring_shutdown_keeps_its_coord_and_nothing_starts() {
        let root = tempfile::tempdir().unwrap();
        let mut provider = cold_fixture_provider(root.path());
        let kubo = kubo_fixture(root.path());
        fs::create_dir_all(&provider.repo_dir).unwrap();
        fs::write(
            provider.repo_dir.join("config"),
            br#"{"Identity":{"PeerID":"mine"}}"#,
        )
        .unwrap();
        let online = online_daemon_coord(root.path(), &kubo, "mine", false);
        provider.initialized = false;
        let started = Instant::now();
        let init = provider.init(serde_json::json!({"base_path": root.path()}));
        let init_elapsed = started.elapsed();
        // Init may find this machine's own Kubo; keep the stand-in.
        provider.kubo_binary = Some(kubo);
        let result = provider.ensure_kubo();
        let still_running = online.running();
        let coord = read_coord_file(root.path());
        online.finish();
        assert!(matches!(init, Response::Ok { .. }));
        assert!(init_elapsed < Duration::from_secs(8), "{init_elapsed:?}");
        let error = result.unwrap_err();
        assert!(error.contains("did not stop"), "{error}");
        assert!(still_running, "the online daemon was signalled");
        assert!(coord.is_some_and(|coord| !coord.offline));
        assert!(
            !provider.repo_dir.with_extension("started").exists(),
            "a second daemon started"
        );
    }

    /// An identity check without a clear answer (a slow `/id`, an HTTP error,
    /// an unreadable body) is not a different identity: the coord stays, the
    /// process is left running and no second daemon starts on the repo.
    #[test]
    #[cfg(unix)]
    fn unclear_identity_answer_keeps_the_coord_and_starts_nothing() {
        for case in ["slow", "http error", "invalid json"] {
            let root = tempfile::tempdir().unwrap();
            let mut provider = cold_fixture_provider(root.path());
            provider.kubo_binary = Some(kubo_fixture(root.path()));
            fs::create_dir_all(&provider.repo_dir).unwrap();
            fs::write(
                provider.repo_dir.join("config"),
                br#"{"Identity":{"PeerID":"mine"}}"#,
            )
            .unwrap();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let api_port = listener.local_addr().unwrap().port();
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stopping = Arc::clone(&stop);
            let api = std::thread::spawn(move || {
                let mut held = Vec::new();
                while !stopping.load(std::sync::atomic::Ordering::SeqCst) {
                    let Ok((mut socket, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    };
                    let reply: &[u8] = match case {
                        "http error" => b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        "invalid json" => b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nnot json",
                        _ => b"",
                    };
                    if read_fixture_request(&mut socket).is_some() {
                        let _ = socket.write_all(reply);
                    }
                    held.push(socket);
                }
            });
            let mut process = Command::new("sleep").arg("60").spawn().unwrap();
            write_old_coord(root.path(), process.id(), api_port);
            let started = Instant::now();
            let result = provider.ensure_kubo();
            let elapsed = started.elapsed();
            let still_running = process.try_wait().unwrap().is_none();
            let coord = read_coord_file(root.path());
            let _ = process.kill();
            process.wait().unwrap();
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
            api.join().unwrap();
            let error = result.unwrap_err();
            assert!(error.contains("no clear answer"), "{case}: {error}");
            assert!(still_running, "{case}: the process was signalled");
            assert!(coord.is_some_and(|coord| !coord.offline), "{case}");
            assert!(
                !provider.repo_dir.with_extension("started").exists(),
                "{case}: a second daemon started"
            );
            assert!(elapsed < Duration::from_secs(3), "{case}: {elapsed:?}");
        }
    }

    /// Kubo runs offline, so a CID this repo lacks fails on its one local
    /// read: no pin/add or gateway retry follows, and Content reaches Carrier
    /// at once.
    #[test]
    fn missing_cid_fails_on_one_local_read() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let backend = std::thread::spawn(move || {
            let mut requests = Vec::new();
            let mut idle_since = Instant::now();
            while idle_since.elapsed() < Duration::from_millis(500) {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        let headers = read_bounded_fixture_headers(&mut socket);
                        requests.push(headers.lines().next().unwrap_or_default().to_string());
                        let body = br#"{"Message":"block was not found locally (offline)","Code":0,"Type":"error"}"#;
                        let _ = socket.write_all(
                            format!(
                                "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            )
                            .as_bytes(),
                        );
                        let _ = socket.write_all(body);
                        idle_since = Instant::now();
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
            requests
        });
        let root = tempfile::tempdir().unwrap();
        let mut provider = bounded_cat_fixture_provider(root.path(), port);
        let started = Instant::now();
        let read = provider.cat("bafkreimissing", None, None);
        let listed = provider.ls("bafkreimissing");
        let elapsed = started.elapsed();
        let requests = backend.join().unwrap();
        assert!(matches!(read, Response::Error { ref code, .. } if code == "cat_failed"));
        assert!(matches!(listed, Response::Error { ref code, .. } if code == "ls_failed"));
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert!(requests[0].starts_with("POST /api/v0/cat?arg=bafkreimissing"));
        assert!(requests[1].starts_with("POST /api/v0/ls?arg=bafkreimissing"));
    }

    /// The shipped Kubo, run as the provider runs it. Needs
    /// `ELASTOS_TEST_KUBO=<kubo 0.40.1 binary>`: an online daemon (as an
    /// earlier release left it) is replaced by an offline one on the same repo,
    /// which has no swarm and fails a missing CID at once.
    #[test]
    #[ignore = "needs ELASTOS_TEST_KUBO"]
    #[cfg(unix)]
    fn real_kubo_online_daemon_is_restarted_offline_and_misses_fast() {
        let kubo = PathBuf::from(std::env::var("ELASTOS_TEST_KUBO").expect("ELASTOS_TEST_KUBO"));
        let root = tempfile::tempdir().unwrap();
        let mut provider = cold_fixture_provider(root.path());
        provider.kubo_binary = Some(kubo.clone());
        let run = |args: &[&str]| {
            let out = Command::new(&kubo)
                .args(args)
                .env("IPFS_PATH", &provider.repo_dir)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
            String::from_utf8(out.stdout).unwrap()
        };
        // The test profile keeps the old online daemon off the public network.
        run(&["init", "--profile=test"]);
        let held = run(&["add", "-Q", "--cid-version=1", "--pin=true", "Cargo.toml"]);
        let held = held.trim().to_string();
        let old_port = bind_free_port().unwrap();
        let mut online = Command::new(&kubo)
            .args(["daemon", "--api", &format!("/ip4/127.0.0.1/tcp/{old_port}")])
            .env("IPFS_PATH", &provider.repo_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // Reaped as soon as it exits, as init reaps a daemon whose parent is gone.
        let online_pid = online.id();
        let (exited, reaped) = std::sync::mpsc::channel();
        let reaper = std::thread::spawn(move || {
            online.wait().unwrap();
            let _ = exited.send(());
        });
        let id = kubo_rpc_url(old_port, "id", &[]);
        let deadline = Instant::now() + Duration::from_secs(30);
        while ureq::post(&id).call().is_err() {
            assert!(Instant::now() < deadline, "online daemon did not start");
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(ureq::post(&kubo_rpc_url(old_port, "swarm/peers", &[]))
            .call()
            .is_ok());
        fs::write(
            coord_file_path(root.path()),
            serde_json::to_vec(&serde_json::json!({
                "kubo_pid": online_pid, "api_port": old_port, "gateway_port": 0,
                "started_at": 1, "last_used": now_unix_secs(),
            }))
            .unwrap(),
        )
        .unwrap();

        let migrated = provider.ensure_kubo();
        let online_exited = reaped.recv_timeout(Duration::from_secs(1)).is_ok();
        if !online_exited {
            unsafe { libc::kill(online_pid as i32, libc::SIGKILL) };
        }
        reaper.join().unwrap();
        let checks = (|| -> Result<(), String> {
            migrated?;
            if !online_exited {
                return Err("online daemon kept running".into());
            }
            let coord = read_coord_file(root.path()).ok_or("no coord")?;
            if !coord.offline || coord.api_port == old_port {
                return Err(format!("not restarted offline: {coord:?}"));
            }
            match ureq::post(&kubo_rpc_url(coord.api_port, "swarm/peers", &[])).call() {
                Err(ureq::Error::Status(_, resp)) => {
                    let body = resp.into_string().unwrap_or_default();
                    if !body.contains("online mode") {
                        return Err(format!("swarm/peers: {body}"));
                    }
                }
                other => return Err(format!("swarm/peers answered offline daemon: {other:?}")),
            }
            let Response::Ok { data: Some(data) } = provider.cat(&held, None, None) else {
                return Err("held CID not readable after migration".into());
            };
            if BASE64
                .decode(data["data"].as_str().unwrap_or_default())
                .ok()
                != fs::read("Cargo.toml").ok()
            {
                return Err("held CID bytes changed".into());
            }
            let missing = "bafkreigh2akiscaildcqabsyg3dfr6chu3fgpregiymsck7e7aqa4s52zy";
            let started = Instant::now();
            let read = provider.cat(missing, None, None);
            let pinned = provider.pin(missing);
            let listed = provider.ls(missing);
            let elapsed = started.elapsed();
            if [read, pinned, listed]
                .iter()
                .any(|response| matches!(response, Response::Ok { .. }))
                || elapsed > Duration::from_secs(2)
            {
                return Err(format!("missing CID did not fail fast: {elapsed:?}"));
            }
            Ok(())
        })();
        stop_fixture_daemon(&mut provider);
        checks.unwrap();
    }

    fn complete_metadata_request(max: u64) -> serde_json::Value {
        let mut request = bounded_cat_fixture_request();
        request["path"] = serde_json::json!("_elastos_object.json");
        request["max_bytes"] = serde_json::json!(max);
        let envelope = request["_runtime_invocation"].as_object_mut().unwrap();
        envelope.remove("range");
        envelope.remove("progress");
        request
    }

    #[test]
    fn complete_metadata_http_preserves_whitespace_and_enforces_eof_and_cap() {
        let whitespace = b" \n{ \"files\": [] }\t\n".to_vec();
        let cap = vec![b' '; MAX_BOUNDED_READ_BYTES as usize];
        let oversized = vec![b' '; MAX_BOUNDED_READ_BYTES as usize + 1];
        let mut cases = Vec::new();
        cases.push((b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n2\r\n{}\r\n0\r\n\r\n".to_vec(), Some(b"{}".to_vec())));
        for bytes in [whitespace, cap] {
            let mut response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .into_bytes();
            response.extend_from_slice(&bytes);
            cases.push((response, Some(bytes)));
        }
        // A chunked body has no advertised size; cap+1 must still fail.
        let mut chunked = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n",
            oversized.len()
        )
        .into_bytes();
        chunked.extend_from_slice(&oversized);
        chunked.extend_from_slice(b"\r\n0\r\n\r\n");
        cases.push((chunked, None));
        for response in [
            &b"HTTP/1.1 200 OK\r\nContent-Length: invalid\r\nConnection: close\r\n\r\n{}"[..],
            &b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n{}"[..],
            &b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n2\r\n{}\r\n"[..],
            &b"HTTP/1.1 500 Error\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"[..],
            &b"HTTP/1.1 302 Found\r\nLocation: /other\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"[..],
            &b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"[..],
        ] { cases.push((response.to_vec(), None)); }
        for (response, expected) in cases {
            let root = tempfile::tempdir().unwrap();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let mut provider =
                bounded_cat_fixture_provider(root.path(), listener.local_addr().unwrap().port());
            let backend = std::thread::spawn(move || {
                let mut socket = accept_bounded_fixture(&listener);
                let headers = read_bounded_fixture_headers(&mut socket);
                socket.write_all(&response).unwrap();
                headers
            });
            let request = complete_metadata_request(MAX_BOUNDED_READ_BYTES);
            let result = provider.handle(serde_json::from_value(request.clone()).unwrap());
            let headers = backend.join().unwrap();
            assert!(headers.contains("_elastos_object.json"));
            assert!(!headers.contains("offset="));
            assert!(headers.contains(&format!("length={}", MAX_BOUNDED_READ_BYTES + 1)));
            if let Some(bytes) = expected {
                let Response::Ok { data: Some(data) } = result else {
                    panic!("metadata read failed: {result:?}")
                };
                assert_eq!(
                    BASE64.decode(data["data"].as_str().unwrap()).unwrap(),
                    bytes
                );
                assert_eq!(
                    data["_runtime_complete_metadata"],
                    serde_json::json!({
                        "schema":"elastos.provider.complete-metadata/v1", "cid":request["cid"],
                        "path":"_elastos_object.json", "max_bytes":MAX_BOUNDED_READ_BYTES,
                        "actual_bytes":bytes.len(), "completed":true
                    })
                );
                assert!(data.get("_runtime_applied_range").is_none());
            } else {
                assert!(matches!(result, Response::Error { .. }));
            }
            assert!(!root.path().join("unused-repo").exists());
        }
    }

    #[test]
    fn complete_metadata_rejects_ambiguous_inputs_before_backend_effect() {
        let root = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut provider =
            bounded_cat_fixture_provider(root.path(), listener.local_addr().unwrap().port());
        for value in [
            serde_json::Value::Null,
            serde_json::json!("64"),
            serde_json::json!(-1),
        ] {
            let mut request = complete_metadata_request(64);
            request["max_bytes"] = value;
            assert!(serde_json::from_value::<Request>(request).is_err());
        }
        for (field, value) in [
            ("max_bytes", serde_json::json!(0)),
            ("max_bytes", serde_json::json!(65537)),
            ("path", serde_json::json!("weights.gguf")),
            ("cid", serde_json::json!("cid?arg=other")),
            ("bounded_read", serde_json::json!(false)),
        ] {
            let mut request = complete_metadata_request(64);
            request[field] = value;
            assert!(matches!(
                provider.handle(serde_json::from_value(request).unwrap()),
                Response::Error { .. }
            ));
        }
        for (field, value) in [
            ("range", serde_json::json!({"start":0,"end":3})),
            (
                "progress",
                serde_json::json!({"request_id":"metadata","expected_bytes":4}),
            ),
            ("transfer", serde_json::json!("json")),
            ("transport", serde_json::json!("carrier")),
        ] {
            let mut request = complete_metadata_request(64);
            request["_runtime_invocation"][field] = value;
            assert!(matches!(
                provider.handle(serde_json::from_value(request).unwrap()),
                Response::Error { .. }
            ));
        }
        assert!(matches!(listener.accept(), Err(err) if err.kind() == io::ErrorKind::WouldBlock));
        assert_eq!(read_coord_file(root.path()).unwrap().last_used, 1);
        assert!(!root.path().join("unused-repo").exists());
    }

    #[test]
    fn complete_metadata_timeout_closes_http_without_returning_partial_bytes() {
        let root = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut provider =
            bounded_cat_fixture_provider(root.path(), listener.local_addr().unwrap().port());
        let backend = std::thread::spawn(move || {
            let mut socket = accept_bounded_fixture(&listener);
            read_bounded_fixture_headers(&mut socket);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n{")
                .unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(8)))
                .unwrap();
            let mut byte = [0];
            match socket.read(&mut byte) {
                Ok(0) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
                    ) => {}
                other => panic!("metadata timeout left HTTP active: {other:?}"),
            }
        });
        let started = Instant::now();
        let result =
            provider.handle(serde_json::from_value(complete_metadata_request(64)).unwrap());
        assert!(matches!(result, Response::Error { .. }));
        assert!(started.elapsed() >= BOUNDED_READ_TIMEOUT - Duration::from_millis(100));
        backend.join().unwrap();
    }

    fn accept_bounded_fixture(listener: &std::net::TcpListener) -> std::net::TcpStream {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            match listener.accept() {
                Ok((socket, _)) => return socket,
                Err(err)
                    if err.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(err) => panic!("fixture accept failed or expired: {err}"),
            }
        }
    }

    fn read_bounded_fixture_headers(socket: &mut std::net::TcpStream) -> String {
        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            assert!(bytes.len() < 4096 && Instant::now() < deadline);
            let mut byte = [0];
            socket.read_exact(&mut byte).unwrap();
            bytes.push(byte[0]);
        }
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn bounded_read_body_deadline_closes_socket_and_next_read_succeeds() {
        let root = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let backend = std::thread::spawn(move || {
            let mut stalled = accept_bounded_fixture(&listener);
            let first = read_bounded_fixture_headers(&mut stalled);
            stalled
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n8")
                .unwrap();
            // Observe the actual HTTP connection ending, not a dropped caller future.
            stalled
                .set_read_timeout(Some(Duration::from_secs(8)))
                .unwrap();
            let mut byte = [0];
            match stalled.read(&mut byte) {
                Ok(0) => {}
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
                    ) => {}
                other => panic!("stalled native read did not close its socket: {other:?}"),
            }
            let mut next = accept_bounded_fixture(&listener);
            let second = read_bounded_fixture_headers(&mut next);
            next.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\ncdef",
            )
            .unwrap();
            (first, second)
        });
        let mut provider = bounded_cat_fixture_provider(root.path(), port);
        let started = Instant::now();
        let failed =
            provider.handle(serde_json::from_value(bounded_cat_fixture_request()).unwrap());
        let elapsed = started.elapsed();
        let mut next_request = bounded_cat_fixture_request();
        next_request["_runtime_invocation"]["range"] =
            serde_json::json!({ "start": 12, "end": 15 });
        next_request["_runtime_invocation"]["progress"]["request_id"] =
            serde_json::json!("next-consumer");
        let next = provider.handle(serde_json::from_value(next_request).unwrap());
        let (first, second) = backend.join().unwrap();
        assert!(matches!(failed, Response::Error { .. }));
        assert!(
            elapsed >= BOUNDED_READ_TIMEOUT - Duration::from_millis(100)
                && elapsed < Duration::from_secs(8)
        );
        for (headers, offset) in [(first, "offset=8"), (second, "offset=12")] {
            assert!(headers.starts_with("POST /api/v0/cat?"));
            assert!(headers.contains("offline=true"));
            assert!(headers.contains("timeout=100ms"));
            assert!(headers.contains(offset) && headers.contains("length=4"));
        }
        let Response::Ok { data: Some(data) } = next else {
            panic!("next read failed: {next:?}")
        };
        assert_eq!(
            BASE64.decode(data["data"].as_str().unwrap()).unwrap(),
            b"cdef"
        );
        assert_eq!(provider.state, KuboState::Ready);
        assert!(!root.path().join("unused-repo").exists());
    }

    #[test]
    fn bounded_read_rejects_redirect_short_and_chunked_oversized_responses() {
        for response in [
            &b"HTTP/1.1 302 Found\r\nLocation: /unexpected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"[..],
            &b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n89a"[..],
            &b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\n89abc\r\n0\r\n\r\n"[..],
        ] {
            let root = tempfile::tempdir().unwrap();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let port = listener.local_addr().unwrap().port();
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let backend = std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut requests = Vec::new();
                while Instant::now() < deadline {
                    let mut socket = match listener.accept() {
                        Ok((socket, _)) => socket,
                        Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                            if done_rx.try_recv().is_ok() { break; }
                            std::thread::sleep(Duration::from_millis(1));
                            continue;
                        }
                        Err(err) => panic!("fixture accept failed: {err}"),
                    };
                    requests.push(read_bounded_fixture_headers(&mut socket));
                    socket.write_all(response).unwrap();
                }
                requests
            });
            let mut provider = bounded_cat_fixture_provider(root.path(), port);
            let result = provider.handle(serde_json::from_value(bounded_cat_fixture_request()).unwrap());
            done_tx.send(()).unwrap();
            let requests = backend.join().unwrap();
            assert_eq!(requests.len(), 1, "bounded failure must not follow, retry or prefetch");
            assert!(requests[0].starts_with("POST /api/v0/cat?"));
            assert!(requests[0].contains("offline=true"));
            assert!(requests[0].contains("timeout=100ms"));
            assert!(matches!(result, Response::Error { .. }));
        }
    }

    #[test]
    fn bounded_read_rejects_invalid_input_without_backend_or_startup() {
        let root = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut provider =
            bounded_cat_fixture_provider(root.path(), listener.local_addr().unwrap().port());
        for (pointer, value) in [
            ("/cid", serde_json::json!("cid&length=999")),
            ("/path", serde_json::json!("../weights.gguf")),
            ("/path", serde_json::json!("weights.gguf?offset=0")),
            ("/path", serde_json::json!("/weights.gguf")),
            ("/_runtime_invocation/range/end", serde_json::Value::Null),
            ("/_runtime_invocation/range/end", serde_json::json!(7)),
            (
                "/_runtime_invocation/range/end",
                serde_json::json!(u64::MAX),
            ),
            (
                "/_runtime_invocation/progress/expected_bytes",
                serde_json::json!(5),
            ),
        ] {
            let mut request = bounded_cat_fixture_request();
            *request.pointer_mut(pointer).unwrap() = value;
            assert!(matches!(
                provider.handle(serde_json::from_value(request).unwrap()),
                Response::Error { .. }
            ));
        }
        let mut request = bounded_cat_fixture_request();
        request["bounded_read"] = serde_json::json!("true");
        assert!(serde_json::from_value::<Request>(request).is_err());
        provider.state = KuboState::Cold;
        assert!(matches!(
            provider.handle(serde_json::from_value(bounded_cat_fixture_request()).unwrap()),
            Response::Error { .. }
        ));
        assert!(matches!(listener.accept(), Err(err) if err.kind() == io::ErrorKind::WouldBlock));
        assert_eq!(read_coord_file(root.path()).unwrap().last_used, 1);
        assert!(!root.path().join("unused-repo").exists());
    }

    #[test]
    fn test_cat_native_range_dispatch_is_bounded_without_prefetch() {
        use std::net::TcpListener;
        use std::sync::mpsc;

        let mut observations = Vec::new();
        for ignore_range in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let port = listener.local_addr().unwrap().port();
            let (done_tx, done_rx) = mpsc::channel();
            let backend = std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut requests = Vec::new();
                while Instant::now() < deadline {
                    let (mut socket, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                            if done_rx.try_recv().is_ok() {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(1));
                            continue;
                        }
                        Err(err) => panic!("fixture accept failed: {err}"),
                    };
                    socket.set_nonblocking(false).unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    socket
                        .set_write_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let mut headers = Vec::new();
                    while !headers.ends_with(b"\r\n\r\n") {
                        assert!(headers.len() < 4096 && Instant::now() < deadline);
                        let mut byte = [0];
                        socket.read_exact(&mut byte).unwrap();
                        headers.push(byte[0]);
                    }
                    let headers = String::from_utf8(headers).unwrap();
                    let request = headers.lines().next().unwrap().to_string();
                    let target = request.split_whitespace().nth(1).unwrap();
                    let query: Vec<_> = target.split_once('?').unwrap().1.split('&').collect();
                    let ranged = query.contains(&"offset=8") && query.contains(&"length=4");
                    let body: &[u8] = if ranged && !ignore_range {
                        b"89ab"
                    } else {
                        b"0123456789abcdef"
                    };
                    let mut reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .into_bytes();
                    reply.extend_from_slice(body);
                    socket.write_all(&reply).unwrap();
                    requests.push((request, ranged));
                }
                requests
            });
            let mut provider = bounded_cat_fixture_provider(root.path(), port);
            let request = serde_json::from_value(bounded_cat_fixture_request()).unwrap();
            let response = provider.handle(request);
            done_tx.send(()).unwrap();
            let requests = backend.join().unwrap();
            assert!(!root.path().join("unused-repo").exists());
            let coord = read_coord_file(root.path()).unwrap();
            assert!(
                coord.last_used > 1,
                "bounded read must refresh backend activity"
            );
            assert_eq!(coord.kubo_pid, std::process::id());
            assert_eq!(coord.api_port, port);
            observations.push((ignore_range, requests, response));
        }

        for (ignore_range, requests, response) in observations {
            assert_eq!(
                requests.len(),
                1,
                "bounded Cat must not pin, prefetch or retry"
            );
            assert!(requests[0].0.starts_with("POST /api/v0/cat?"));
            assert!(requests[0].0.contains("offline=true"));
            assert!(requests[0].0.contains("timeout=100ms"));
            assert!(requests[0].1, "bounded Cat must send offset=8 and length=4");
            if ignore_range {
                assert!(
                    matches!(response, Response::Error { .. }),
                    "oversized response accepted"
                );
            } else {
                let Response::Ok { data: Some(data) } = response else {
                    panic!("exact range failed: {response:?}");
                };
                assert_eq!(
                    BASE64.decode(data["data"].as_str().unwrap()).unwrap(),
                    b"89ab"
                );
            }
        }
    }

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
            offline: true,
        };

        write_coord_file(&data_dir, &coord);
        update_coord_last_used(&data_dir);
        let read = read_coord_file(&data_dir).expect("Should read coord file");
        assert_eq!(read.kubo_pid, 12345);
        assert_eq!(read.api_port, 5001);
        assert_eq!(read.gateway_port, 8080);
        assert!(read.offline, "a last-used refresh keeps the offline mark");

        // A coord file an earlier release wrote names an online daemon.
        fs::write(
            coord_file_path(&data_dir),
            br#"{"kubo_pid":1,"api_port":5001,"gateway_port":8080,"started_at":1,"last_used":2}"#,
        )
        .unwrap();
        assert!(!read_coord_file(&data_dir).unwrap().offline);

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
                ..
            } => {
                assert_eq!(data, "aGVsbG8=");
                assert_eq!(filename, "test.txt");
                assert!(pin);
            }
            _ => panic!("Expected AddBytes"),
        }
    }

    #[test]
    fn test_request_accepts_runtime_invocation_metadata() {
        let runtime = r#"{
            "schema":"elastos.provider.invocation/v1",
            "source":"content-provider",
            "target":"ipfs",
            "op":"add_bytes",
            "transport":"runtime-local-provider-plane",
            "transfer":"bytes"
        }"#;
        let json = format!(
            r#"{{"op":"add_bytes","data":"aGVsbG8=","filename":"test.txt","pin":true,"_runtime_invocation":{runtime}}}"#
        );
        let req: Request = serde_json::from_str(&json).expect("Should parse runtime envelope");
        assert!(matches!(req, Request::AddBytes { .. }));

        let json = r#"{"op":"cat","cid":"QmTest","_runtime_invocation":{"schema":"elastos.provider.invocation/v1"}}"#;
        let req: Request = serde_json::from_str(json).expect("Should parse runtime cat envelope");
        assert!(matches!(req, Request::Cat { .. }));
    }

    #[test]
    fn test_request_still_rejects_unknown_fields() {
        let json =
            r#"{"op":"add_bytes","data":"aGVsbG8=","filename":"test.txt","pin":true,"admin":true}"#;
        let err = serde_json::from_str::<Request>(json).expect_err("Should reject unknown fields");
        assert!(err.to_string().contains("unknown field `admin`"));
    }

    #[test]
    fn test_cat_request_deserialization() {
        let json = r#"{"op":"cat","cid":"QmTest","path":"file.txt"}"#;
        let req: Request = serde_json::from_str(json).expect("Should parse cat");
        match req {
            Request::Cat { cid, path, .. } => {
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
    fn test_init_config_base_path_overrides_data_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("kubo"), b"test").unwrap();
        let mut provider = IpfsProvider::new();
        let response = provider.init(serde_json::json!({
            "base_path": tmp.path().to_string_lossy()
        }));

        assert!(matches!(response, Response::Ok { .. }));
        assert_eq!(provider.data_dir, tmp.path());
        assert_eq!(provider.repo_dir, tmp.path().join("ipfs-repo"));
        assert_eq!(provider.kubo_binary, Some(tmp.path().join("bin/kubo")));
    }

    #[test]
    fn runtime_role_init_preserves_native_environment_roots() {
        // Child test processes isolate native environment selection from other
        // tests. Init must preserve that selected root for both Runtime roles.
        if let Some(expected) = std::env::var_os("ISSUE89_INIT_EXPECTED_ROOT") {
            let expected = PathBuf::from(expected);
            for role in ["user", "gateway"] {
                let mut provider = IpfsProvider::new();
                assert_eq!(provider.data_dir, expected);
                let request: Request = serde_json::from_value(serde_json::json!({
                    "op":"init", "config":{"base_path":"", "allowed_paths":[],
                    "read_only":false, "encryption_key":"",
                    "extra":{"runtime_host_role":role}}
                }))
                .unwrap();
                assert!(matches!(provider.handle(request), Response::Ok { .. }));
                assert_eq!(provider.data_dir, expected);
                assert_eq!(provider.repo_dir, expected.join("ipfs-repo"));
                assert_eq!(provider.kubo_binary, Some(expected.join("bin/kubo")));
                assert_eq!(
                    provider.host_role,
                    if role == "user" {
                        HostRole::User
                    } else {
                        HostRole::Gateway
                    }
                );
                assert!(provider.initialized);
            }
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("parent");
        let explicit = root.path().join("explicit");
        let xdg = root.path().join("child-xdg");
        let home = root.path().join("child-home");
        for selected in [&parent, &explicit, &xdg.join("elastos")] {
            fs::create_dir_all(selected.join("bin")).unwrap();
            fs::create_dir_all(selected.join("ipfs-repo")).unwrap();
            fs::write(
                selected.join("bin/kubo"),
                b"fixture; Init does not execute Kubo",
            )
            .unwrap();
            fs::write(selected.join("ipfs-repo/config"), b"{}\n").unwrap();
        }
        for (case, expected, managed_parent) in [
            ("managed-parent", parent.clone(), parent.clone()),
            ("explicit", explicit.clone(), parent.clone()),
            (
                "xdg",
                xdg.join("elastos"),
                root.path().join("absent-parent"),
            ),
        ] {
            let mut child = Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "tests::runtime_role_init_preserves_native_environment_roots",
                    "--nocapture",
                ])
                .env_clear()
                .env("HOME", &home)
                .env("XDG_DATA_HOME", &xdg)
                .env("ELASTOS_HOME_LAUNCH_TRUSTED_AUTH_DATA_DIR", &managed_parent)
                .env("ISSUE89_INIT_EXPECTED_ROOT", &expected);
            if case == "explicit" {
                child.env("ELASTOS_DATA_DIR", &explicit);
            }
            let result = child.output().unwrap();
            assert!(
                result.status.success()
                    && String::from_utf8_lossy(&result.stdout).contains("1 passed; 0 failed"),
                "{case}: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }

    #[test]
    fn runtime_host_role_defaults_to_user_and_only_gateway_keeps_kubo() {
        for config in [
            serde_json::json!({}),
            serde_json::json!({"extra":null}),
            serde_json::json!({"extra":{"runtime_host_role":"user", "keep_alive":true}}),
        ] {
            assert_eq!(HostRole::from_init(&config), Ok(HostRole::User));
        }
        assert_eq!(
            HostRole::from_init(&serde_json::json!({"extra":{"runtime_host_role":"gateway"}})),
            Ok(HostRole::Gateway)
        );
        let limit = IDLE_TIMEOUT_SECS;
        assert!(!HostRole::User.idle_stop_due(100, 100 + limit));
        assert!(HostRole::User.idle_stop_due(100, 101 + limit));
        assert!(!HostRole::User.idle_stop_due(100 + limit, 100));
        assert!(!HostRole::Gateway.idle_stop_due(0, u64::MAX));
        for role in [
            serde_json::Value::Null,
            serde_json::json!(true),
            serde_json::json!(600),
            serde_json::json!("Gateway"),
            serde_json::json!("publisher"),
            serde_json::json!("gateway "),
        ] {
            assert!(
                HostRole::from_init(&serde_json::json!({"extra":{"runtime_host_role":role}}))
                    .is_err()
            );
        }
        assert!(HostRole::from_init(&serde_json::json!({"runtime_host_role":"gateway"})).is_err());
    }

    #[test]
    fn runtime_init_refuses_role_or_root_changes_without_mutation() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        for role in ["user", "gateway"] {
            let mut provider = IpfsProvider::new();
            let config =
                serde_json::json!({"base_path":root.path(), "extra":{"runtime_host_role":role}});
            assert!(matches!(provider.init(config.clone()), Response::Ok { .. }));
            assert!(matches!(provider.init(config.clone()), Response::Ok { .. }));
            let bound_role = provider.host_role;
            for change in [
                serde_json::json!({"base_path":other.path(), "extra":{"runtime_host_role":role}}),
                serde_json::json!({"base_path":root.path(), "extra":{"runtime_host_role":if role=="user" {"gateway"} else {"user"}}}),
            ] {
                assert!(matches!(provider.init(change), Response::Error { .. }));
                assert_eq!(provider.host_role, bound_role);
                assert_eq!(provider.data_dir, root.path());
                assert_eq!(provider.repo_dir, root.path().join("ipfs-repo"));
                assert_eq!(provider.state, KuboState::Cold);
            }
        }
        let mut provider = IpfsProvider::new();
        let previous = provider.data_dir.clone();
        assert!(matches!(
            provider.init(serde_json::json!({"base_path":root.path(),
            "extra":{"runtime_host_role":true}})),
            Response::Error { .. }
        ));
        assert_eq!(provider.data_dir, previous);
        assert!(!provider.initialized);
        assert_eq!(provider.host_role, HostRole::User);
    }

    #[test]
    fn test_find_kubo_binary_uses_managed_runtime_bin_dir() {
        let data_dir = tempfile::tempdir().unwrap();
        let shared_bin = tempfile::tempdir().unwrap();
        fs::write(shared_bin.path().join("kubo"), b"test").unwrap();
        std::env::set_var("ELASTOS_CAPSULE_BIN_DIR", shared_bin.path());

        let kubo = find_kubo_binary(data_dir.path());

        std::env::remove_var("ELASTOS_CAPSULE_BIN_DIR");
        assert_eq!(kubo, Some(shared_bin.path().join("kubo")));
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
        let data_dir = data_dir();
        let result = validate_dest_path(&data_dir, Path::new("relative/path"));
        assert!(result.is_err());
    }

    #[test]
    fn dest_cat_to_path_uses_offline_local_cat_without_prefetch() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("weights.gguf");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let backend = std::thread::spawn(move || {
            let mut socket = accept_bounded_fixture(&listener);
            let headers = read_bounded_fixture_headers(&mut socket);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nGGUF")
                .unwrap();
            drop(socket);
            assert!(
                matches!(listener.accept(), Err(e) if e.kind() == io::ErrorKind::WouldBlock),
                "dest cat must not pin, prefetch or retry"
            );
            headers
        });
        let mut provider = bounded_cat_fixture_provider(root.path(), port);
        let response = provider.handle(
            parse_request(
                &serde_json::json!({
                    "op": "cat_to_path",
                    "cid": "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
                    "path": "weights.gguf",
                    "dest": dest.to_string_lossy(),
                })
                .to_string(),
            )
            .unwrap(),
        );
        let headers = backend.join().unwrap();
        assert!(matches!(response, Response::Ok { .. }), "{response:?}");
        assert!(headers.starts_with("POST /api/v0/cat?"));
        assert!(headers.contains("offline=true"));
        assert!(!headers.contains("/api/v0/pin/add"));
        assert_eq!(std::fs::read(&dest).unwrap(), b"GGUF");
    }

    /// Port, stop flag and serving thread of `kubo_serving_all`.
    type ServingKubo = (
        u16,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        std::thread::JoinHandle<(Vec<String>, u64)>,
    );

    /// A Kubo double that serves every request with `answer` until the
    /// caller sets the returned stop flag. The thread returns each request's
    /// first line and the body bytes the answers managed to send.
    fn kubo_serving_all<F>(answer: F) -> ServingKubo
    where
        F: Fn(&str, &mut std::net::TcpStream) -> u64 + Send + 'static,
    {
        use std::sync::atomic::Ordering;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = stop.clone();
        let serving = std::thread::spawn(move || {
            let mut requests = Vec::new();
            let mut sent = 0;
            while !stopped.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        let headers = read_bounded_fixture_headers(&mut socket);
                        requests.push(headers.lines().next().unwrap_or_default().to_string());
                        socket
                            .set_write_timeout(Some(Duration::from_secs(10)))
                            .unwrap();
                        sent += answer(&headers, &mut socket);
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(e) => panic!("fixture accept failed: {e}"),
                }
            }
            (requests, sent)
        });
        (port, stop, serving)
    }

    fn whole_bounded_cat(provider: &mut IpfsProvider, whole_max_bytes: u64) -> Response {
        provider.handle(
            serde_json::from_value(serde_json::json!({
                "op": "cat",
                "cid": "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
                "path": "index.html",
                "whole_max_bytes": whole_max_bytes,
            }))
            .unwrap(),
        )
    }

    #[test]
    fn whole_bounded_cat_miss_reads_only_the_local_repo() {
        let root = tempfile::tempdir().unwrap();
        let (port, stop, backend) = kubo_serving_all(|_, socket| {
            let body =
                br#"{"Message":"block was not found locally (offline)","Code":0,"Type":"error"}"#;
            let head = format!(
                "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(head.as_bytes());
            let _ = socket.write_all(body);
            0
        });
        let mut provider = bounded_cat_fixture_provider(root.path(), port);
        let started = Instant::now();
        let response = whole_bounded_cat(&mut provider, 1024 * 1024);
        let elapsed = started.elapsed();
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let (requests, _) = backend.join().unwrap();
        assert!(
            matches!(&response, Response::Error { code, .. } if code == "cat_failed"),
            "{response:?}"
        );
        assert!(!requests.is_empty());
        for request in &requests {
            assert!(
                request.starts_with("POST /api/v0/cat?") && request.contains("offline=true"),
                "a whole-bounded read only reads the local repo: {requests:?}"
            );
        }
        assert!(elapsed < BOUNDED_READ_TIMEOUT, "{elapsed:?}");
    }

    /// Answers the local-repo probe with one byte, as Kubo does for a hit.
    fn answer_local_probe(headers: &str, socket: &mut std::net::TcpStream) -> bool {
        if !headers.contains("length=1") {
            return false;
        }
        let _ =
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\n<");
        true
    }

    #[test]
    fn whole_bounded_cat_hit_returns_the_local_bytes() {
        let root = tempfile::tempdir().unwrap();
        let (port, stop, backend) = kubo_serving_all(|headers, socket| {
            if !answer_local_probe(headers, socket) {
                let _ = socket.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\nConnection: close\r\n\r\n<html>hi</html>",
                );
            }
            0
        });
        let mut provider = bounded_cat_fixture_provider(root.path(), port);
        let response = whole_bounded_cat(&mut provider, 1024);
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let (requests, _) = backend.join().unwrap();
        let Response::Ok { data: Some(data) } = &response else {
            panic!("local hit failed: {response:?}")
        };
        assert_eq!(data["data"], BASE64.encode(b"<html>hi</html>"));
        assert_eq!(requests.len(), 2, "{requests:?}");
        let (probe, read) = (&requests[0], &requests[1]);
        assert!(probe.starts_with("POST /api/v0/cat?") && read.starts_with("POST /api/v0/cat?"));
        assert!(
            probe.contains("offline=true&timeout=100ms&length=1"),
            "{probe}"
        );
        assert!(read.contains("offline=true&timeout=60s") && !read.contains("length="));
    }

    #[test]
    fn whole_bounded_cat_hit_past_its_limit_is_refused_while_reading() {
        let root = tempfile::tempdir().unwrap();
        let limit = 100 * 1024 * 1024;
        // One answer of three times the limit, with no declared length.
        let (port, stop, backend) = kubo_serving_all(move |headers, socket| {
            if answer_local_probe(headers, socket)
                || socket
                    .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                    .is_err()
            {
                return 0;
            }
            write_body(socket, 3 * limit, |_| b'a')
        });
        let mut provider = bounded_cat_fixture_provider(root.path(), port);
        let response = whole_bounded_cat(&mut provider, limit);
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let (requests, sent) = backend.join().unwrap();
        match &response {
            Response::Error { code, message } => {
                assert_eq!(code, "cat_failed");
                assert_eq!(
                    message,
                    &format!("{WHOLE_CONTENT_TOO_LARGE} of {limit} bytes")
                );
            }
            Response::Ok { .. } => panic!("content past the limit was returned"),
        }
        assert!(
            sent < 2 * limit,
            "the provider read on past the limit ({sent} bytes sent)"
        );
        assert_eq!(
            requests.len(),
            2,
            "a refused read is not repeated: {requests:?}"
        );
        assert!(requests.iter().all(|r| r.contains("offline=true")));
    }

    #[test]
    fn pin_status_reads_the_local_pin_set_offline() {
        let root = tempfile::tempdir().unwrap();
        for (status, body, pinned) in [
            (
                "200 OK",
                r#"{"Keys":{"bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku":{"Type":"recursive","Name":""}}}"#,
                Some(true),
            ),
            ("200 OK", r#"{"Keys":{}}"#, None),
            (
                "500 Internal Server Error",
                r#"{"Message":"path 'bafy' is not pinned","Code":0,"Type":"error"}"#,
                Some(false),
            ),
            (
                "500 Internal Server Error",
                r#"{"Message":"context deadline exceeded","Code":0,"Type":"error"}"#,
                None,
            ),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let port = listener.local_addr().unwrap().port();
            let reply = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let backend = std::thread::spawn(move || {
                let mut socket = accept_bounded_fixture(&listener);
                let headers = read_bounded_fixture_headers(&mut socket);
                socket.write_all(reply.as_bytes()).unwrap();
                headers
            });
            let mut provider = bounded_cat_fixture_provider(root.path(), port);
            let response = provider.handle(
                parse_request(
                    r#"{"op":"pin_status","cid":"bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku"}"#,
                )
                .unwrap(),
            );
            let headers = backend.join().unwrap();
            assert!(headers.starts_with("POST /api/v0/pin/ls?"), "{headers}");
            assert!(headers.contains("offline=true") && headers.contains("type=recursive"));
            match pinned {
                Some(pinned) => assert_eq!(
                    serde_json::to_value(&response).unwrap()["data"]["pinned"],
                    pinned,
                    "{response:?}"
                ),
                None => assert!(matches!(response, Response::Error { .. }), "{response:?}"),
            }
        }
    }

    #[test]
    fn copy_reader_to_path_writes_chunked_bytes() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("out.bin");
        let data: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
        let written = copy_reader_to_path(std::io::Cursor::new(data.clone()), &dest).unwrap();
        assert_eq!(written, data.len() as u64);
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert!(!dest.with_extension("bin.part").exists());
    }

    #[test]
    fn managed_home_content_attaches_to_existing_parent_repo() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("gateway");
        let child = root.path().join("child-xdg");
        std::fs::create_dir_all(parent.join("ipfs-repo")).unwrap();
        std::fs::write(parent.join("ipfs-repo").join("config"), b"{}\n").unwrap();
        let selected = select_data_dir(DataDirChoice {
            explicit: None,
            parent_content_repo: Some(&parent),
            xdg_data_home: Some(child.to_str().unwrap()),
            home: Some(root.path()),
        });
        assert_eq!(selected, parent);
    }

    #[test]
    fn explicit_data_dir_overrides_parent_content_repo() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("gateway");
        let explicit = root.path().join("explicit");
        std::fs::create_dir_all(parent.join("ipfs-repo")).unwrap();
        std::fs::write(parent.join("ipfs-repo").join("config"), b"{}\n").unwrap();
        let selected = select_data_dir(DataDirChoice {
            explicit: Some(explicit.to_str().unwrap()),
            parent_content_repo: Some(&parent),
            xdg_data_home: Some(root.path().join("xdg").to_str().unwrap()),
            home: Some(root.path()),
        });
        assert_eq!(selected, explicit);
    }

    #[test]
    fn absent_parent_repo_keeps_the_child_data_dir() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("gateway");
        let child = root.path().join("child-xdg");
        std::fs::create_dir_all(&parent).unwrap();
        let selected = select_data_dir(DataDirChoice {
            explicit: None,
            parent_content_repo: Some(&parent),
            xdg_data_home: Some(child.to_str().unwrap()),
            home: Some(root.path()),
        });
        assert_eq!(selected, child.join("elastos"));
    }

    #[test]
    fn test_validate_source_path_rejects_relative() {
        let data_dir = data_dir();
        let result = validate_source_path(&data_dir, Path::new("relative/file"));
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_source_path_rejects_outside_roots() {
        let data_dir = data_dir();
        let result = validate_source_path(&data_dir, Path::new("/etc/passwd"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be under"));
    }

    #[test]
    fn test_validate_source_path_allows_tmp() {
        let data_dir = data_dir();
        let tmp = std::env::temp_dir().join("elastos-test-file");
        let result = validate_source_path(&data_dir, &tmp);
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn test_validate_source_path_allows_data_dir() {
        let data_dir = data_dir();
        let path = data_dir.join("some-file.bin");
        let result = validate_source_path(&data_dir, &path);
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

    /// A Kubo double that reads requests as Kubo does: RPC query pairs are
    /// form-decoded and `offset`/`length` are honoured. `files` maps literal
    /// `cid/path` to bytes.
    fn literal_path_kubo(
        files: std::collections::HashMap<String, Vec<u8>>,
    ) -> (
        u16,
        Arc<std::sync::atomic::AtomicBool>,
        std::thread::JoinHandle<()>,
    ) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let serving = std::thread::spawn(move || {
            while !stopping.load(std::sync::atomic::Ordering::SeqCst) {
                let mut socket = match listener.accept() {
                    Ok((socket, _)) => socket,
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(err) => panic!("fixture accept failed: {err}"),
                };
                let headers = read_bounded_fixture_headers(&mut socket);
                let target = headers.split(' ').nth(1).unwrap_or_default();
                let url = url::Url::parse(&format!("http://kubo{target}")).unwrap();
                let body = if url.path() == "/api/v0/cat" {
                    let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
                    let value = |name: &str| {
                        pairs
                            .iter()
                            .find(|(key, _)| key == name)
                            .map(|(_, value)| value.clone())
                    };
                    value("arg").and_then(|arg| files.get(&arg)).map(|bytes| {
                        let offset = value("offset").map_or(0, |v| v.parse().unwrap_or(0));
                        let length = value("length").map_or(usize::MAX, |v| v.parse().unwrap_or(0));
                        bytes
                            .iter()
                            .skip(offset)
                            .take(length)
                            .copied()
                            .collect::<Vec<u8>>()
                    })
                } else {
                    None
                };
                let reply = match body {
                    Some(bytes) => {
                        let mut reply = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            bytes.len()
                        )
                        .into_bytes();
                        reply.extend_from_slice(&bytes);
                        reply
                    }
                    None => b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                };
                let _ = socket.write_all(&reply);
            }
        });
        (port, stop, serving)
    }

    #[test]
    fn literal_file_paths_reach_kubo_unchanged_on_every_read_route() {
        const CID: &str = "bafyliteralpaths";
        let paths = elastos_common::test_fixtures::literal_content_paths();
        let expected = |path: &str| format!("content of {path:?}").into_bytes();
        let files: std::collections::HashMap<String, Vec<u8>> = paths
            .iter()
            .map(|path| (format!("{CID}/{path}"), expected(path)))
            .collect();
        let mut failures = Vec::new();
        for route in ["kubo cat", "kubo cat to path"] {
            let (port, stop, serving) = literal_path_kubo(files.clone());
            let root = tempfile::tempdir().unwrap();
            let root_path = fs::canonicalize(root.path()).unwrap();
            let mut provider = bounded_cat_fixture_provider(&root_path, port);
            for (index, path) in paths.iter().enumerate() {
                let got = if route == "kubo cat to path" {
                    let dest = root_path.join(format!("out-{index}"));
                    match provider.cat_to_path(CID, Some(path), dest.to_str().unwrap()) {
                        Response::Ok { .. } => fs::read(&dest).ok(),
                        Response::Error { .. } => None,
                    }
                } else {
                    match provider.cat(CID, Some(path), None) {
                        Response::Ok { data: Some(data) } => data["data"]
                            .as_str()
                            .and_then(|encoded| BASE64.decode(encoded).ok()),
                        _ => None,
                    }
                };
                if got.as_deref() != Some(expected(path).as_slice()) {
                    failures.push(format!(
                        "{route}: {path:?} -> {:?}",
                        got.map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    ));
                }
            }
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
            serving.join().unwrap();
        }
        assert!(
            failures.is_empty(),
            "{} of {} literal path reads changed:\n{}",
            failures.len(),
            paths.len() * 2,
            failures.join("\n")
        );
    }
}
