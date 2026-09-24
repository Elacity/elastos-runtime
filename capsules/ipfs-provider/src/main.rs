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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HEALTH_POLL_INTERVAL: Duration = Duration::from_secs(2);
const KUBO_STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
const IDLE_TIMEOUT_SECS: u64 = 600; // 10 minutes
const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(60);
const LOCKFILE_POLL_INTERVAL: Duration = Duration::from_millis(500);
const LOCKFILE_POLL_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const LARGE_HTTP_TIMEOUT: Duration = Duration::from_secs(300);

const PIN_PROBE_FIRST_SAMPLE: Duration = Duration::from_secs(15);
const PIN_PROBE_SAMPLE_INTERVAL: Duration = Duration::from_secs(30);

/// Samples Kubo while a pin is in flight.
///
/// A pin is one blocking HTTP call, so the only thing the caller can report
/// about a slow one is how long it took -- which says nothing about why. The
/// interesting state lives inside Kubo and only exists *during* the call:
/// whether the CID is still in the wantlist, whether any blocks are arriving,
/// and how many peers are connected. Sampled after the fact it is all gone,
/// and the question gets answered by guessing instead.
///
/// Runs on its own thread because the pin blocks this one, stops when the pin
/// returns, and never fails the pin: every sample is best-effort and a probe
/// that cannot reach Kubo simply says so.
struct KuboPinProbe {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl KuboPinProbe {
    fn start(api_url: String, cid: String) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_signal = stop.clone();
        let handle = std::thread::spawn(move || {
            let started = Instant::now();
            let mut due = PIN_PROBE_FIRST_SAMPLE;
            while !stop_signal.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(250));
                if started.elapsed() < due {
                    continue;
                }
                due += PIN_PROBE_SAMPLE_INTERVAL;
                let stat = kubo_probe_json(&api_url, "bitswap/stat");
                let peers = kubo_probe_json(&api_url, "swarm/peers");
                let wanted = stat
                    .as_ref()
                    .and_then(|stat| stat.get("Wantlist"))
                    .and_then(|list| list.as_array())
                    .map(|list| {
                        list.iter()
                            .any(|entry| entry.get("/").and_then(|v| v.as_str()) == Some(&cid))
                    })
                    .unwrap_or(false);
                eprintln!(
                    "ipfs-provider: pin waiting cid={} elapsed_s={} cid_in_wantlist={} wantlist={} blocks_received={} data_received={} dup_blocks={} peers_bitswap={} peers_swarm={}",
                    cid,
                    started.elapsed().as_secs(),
                    wanted,
                    kubo_probe_len(stat.as_ref(), "Wantlist"),
                    kubo_probe_num(stat.as_ref(), "BlocksReceived"),
                    kubo_probe_num(stat.as_ref(), "DataReceived"),
                    kubo_probe_num(stat.as_ref(), "DupBlksReceived"),
                    kubo_probe_len(stat.as_ref(), "Peers"),
                    peers
                        .as_ref()
                        .and_then(|peers| peers.get("Peers"))
                        .and_then(|peers| peers.as_array())
                        .map(|peers| peers.len() as i64)
                        .unwrap_or(-1),
                );
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for KuboPinProbe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn kubo_probe_json(api_url: &str, path: &str) -> Option<serde_json::Value> {
    ureq::post(&format!("{api_url}/api/v0/{path}"))
        .timeout(Duration::from_secs(10))
        .call()
        .ok()
        .and_then(|resp| resp.into_json::<serde_json::Value>().ok())
}

fn kubo_probe_num(stat: Option<&serde_json::Value>, key: &str) -> i64 {
    stat.and_then(|stat| stat.get(key))
        .and_then(|value| value.as_i64())
        .unwrap_or(-1)
}

fn kubo_probe_len(stat: Option<&serde_json::Value>, key: &str) -> i64 {
    stat.and_then(|stat| stat.get(key))
        .and_then(|value| value.as_array())
        .map(|value| value.len() as i64)
        .unwrap_or(-1)
}

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

// ── Protocol types ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
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
    EnsureStarted {
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

// ── Peering ─────────────────────────────────────────────────────────

/// A node whose swarm connection must survive ConnMgr pruning.
///
/// Stock kubo drops an untagged connection within seconds of crossing the
/// ConnMgr HighWater mark, so co-operating ElastOS nodes lose each other and
/// can only re-find a fresh CID through the DHT (minutes). Peering tags the
/// connection permanently. `addrs` may be empty: peering by peer id alone is
/// enough to protect an *inbound* connection, which is all the host side can
/// do when the other node lives behind a Docker bridge it cannot dial.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PeeringPeer {
    id: String,
    addrs: Vec<String>,
}

fn default_elacity_storage_peer() -> PeeringPeer {
    PeeringPeer {
        id: "12D3KooWNieM3HRBJdVqaQucZEJdqA3oWKrKf3Gx3hp2cmtR9GNK".to_string(),
        addrs: vec![
            "/ip4/34.77.31.164/tcp/4001".to_string(),
            "/ip4/34.77.31.164/udp/4001/quic-v1".to_string(),
        ],
    }
}

const ELACITY_PEER_ENABLED_ENV: &str = "ELASTOS_IPFS_ELACITY_PEER_ENABLED";

fn elacity_peer_enabled(value: Option<&str>) -> Result<bool, String> {
    match value.map(str::trim) {
        None | Some("true") => Ok(true),
        Some("false") => Ok(false),
        _ => Err(format!("{ELACITY_PEER_ENABLED_ENV} must be true or false")),
    }
}

fn with_elacity_peer(mut peers: Vec<PeeringPeer>, enabled: bool) -> Vec<PeeringPeer> {
    if enabled {
        let default_peer = default_elacity_storage_peer();
        if let Some(peer) = peers.iter_mut().find(|peer| peer.id == default_peer.id) {
            for addr in default_peer.addrs {
                if !peer.addrs.contains(&addr) {
                    peer.addrs.push(addr);
                }
            }
        } else {
            peers.push(default_peer);
        }
    }
    peers
}

fn elacity_bootstrap_peers(mut peers: Vec<String>, enabled: bool) -> Vec<String> {
    let elacity = peering_multiaddrs(&default_elacity_storage_peer());
    peers.retain(|peer| !elacity.contains(peer));
    if enabled {
        peers.extend(elacity);
    }
    peers
}

/// Parse `extra.peering`. Fails closed: a malformed entry is a config error,
/// never a silently dropped peer (a missing peer looks exactly like the
/// discovery failure this feature exists to prevent).
fn parse_peering_config(value: Option<&serde_json::Value>) -> Result<Vec<PeeringPeer>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }

    let entries = value
        .as_array()
        .ok_or("ipfs-provider peering must be an array of {id, addrs} objects")?;

    let mut peers = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let entry = entry
            .as_object()
            .ok_or_else(|| format!("ipfs-provider peering[{}] must be an object", index))?;

        let id = entry
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .ok_or_else(|| format!("ipfs-provider peering[{}] requires a string id", index))?;
        if id.is_empty() {
            return Err(format!("ipfs-provider peering[{}] has an empty id", index));
        }
        // base58btc peer ids ("12D3Koo…", "Qm…") are alphanumeric; anything
        // else would corrupt the multiaddr we build from it.
        if !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(format!(
                "ipfs-provider peering[{}] id must be alphanumeric, got {:?}",
                index, id
            ));
        }

        let mut addrs = Vec::new();
        match entry.get("addrs") {
            None => {}
            Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Array(items)) => {
                for item in items {
                    let addr = item
                        .as_str()
                        .map(str::trim)
                        .filter(|addr| !addr.is_empty())
                        .ok_or_else(|| {
                            format!(
                                "ipfs-provider peering[{}] addrs must be non-empty strings",
                                index
                            )
                        })?;
                    addrs.push(addr.to_string());
                }
            }
            Some(_) => {
                return Err(format!(
                    "ipfs-provider peering[{}] addrs must be an array of strings",
                    index
                ))
            }
        }

        peers.push(PeeringPeer {
            id: id.to_string(),
            addrs,
        });
    }

    Ok(peers)
}

/// kubo's repo config uses capitalised `ID`/`Addrs`, unlike our wire shape.
fn peering_peers_config_json(peers: &[PeeringPeer]) -> serde_json::Value {
    serde_json::Value::Array(
        peers
            .iter()
            .map(|peer| serde_json::json!({ "ID": peer.id, "Addrs": peer.addrs }))
            .collect(),
    )
}

/// Multiaddrs for `swarm/peering/add`; a bare `/p2p/<id>` is accepted by kubo
/// and is what an entry without addrs resolves to.
fn peering_multiaddrs(peer: &PeeringPeer) -> Vec<String> {
    if peer.addrs.is_empty() {
        return vec![format!("/p2p/{}", peer.id)];
    }
    peer.addrs
        .iter()
        .map(|addr| format!("{}/p2p/{}", addr, peer.id))
        .collect()
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
    peering: Vec<PeeringPeer>,
    elacity_peer_enabled: bool,
    /// Runtime peering adds are per-daemon, not per-op; this keeps the hot
    /// path from re-POSTing them on every request.
    peering_applied: bool,
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
            peering: Vec::new(),
            elacity_peer_enabled: true,
            peering_applied: false,
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
                ..
            } => self.add_bytes(&data, &filename, pin),
            Request::AddPath { path, pin } => self.add_path(&path, pin),
            Request::AddDirectory { files, pin, .. } => self.add_directory(files, pin),
            Request::Cat { cid, path, .. } => self.cat(&cid, path.as_deref()),
            Request::CatToPath { cid, path, dest } => {
                self.cat_to_path(&cid, path.as_deref(), &dest)
            }
            Request::GetBytes { cid, path } => self.cat(&cid, path.as_deref()),
            Request::Ls { cid } => self.ls(&cid),
            Request::DownloadDirectory { cid, dest } => self.download_directory(&cid, &dest),
            Request::Pin { cid, .. } => self.pin(&cid),
            Request::Unpin { cid, .. } => self.unpin(&cid),
            Request::EnsureStarted { .. } => self.ensure_started(),
            Request::Health => self.health(),
            Request::Status => self.status(),
            Request::Shutdown => self.shutdown(),
        }
    }

    // ── Init ────────────────────────────────────────────────────────

    fn init(&mut self, config: serde_json::Value) -> Response {
        let extra = config.get("extra").unwrap_or(&config);

        if let Some(base_path) = config
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
            self.data_dir = path;
            self.repo_dir = self.data_dir.join("ipfs-repo");
        }

        self.elacity_peer_enabled =
            match elacity_peer_enabled(std::env::var(ELACITY_PEER_ENABLED_ENV).ok().as_deref()) {
                Ok(enabled) => enabled,
                Err(err) => return Response::error("invalid_config", &err),
            };
        match parse_peering_config(extra.get("peering")) {
            Ok(peers) => {
                let peers = with_elacity_peer(peers, self.elacity_peer_enabled);
                if !peers.is_empty() {
                    eprintln!(
                        "ipfs-provider: peering configured for {} peer(s)",
                        peers.len()
                    );
                }
                self.peering = peers;
            }
            Err(e) => return Response::error("invalid_config", &e),
        }

        if extra.get("gateways").is_some() || std::env::var("ELASTOS_IPFS_GATEWAYS").is_ok() {
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
                    // This daemon may have been adopted (init() found it via the
                    // coord file), so its repo config predates our peering list.
                    self.apply_peering_to_running_kubo();
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
            self.peering_applied = false;
        }

        if self.state == KuboState::Starting || self.state == KuboState::Error {
            self.state = KuboState::Cold;
            self.peering_applied = false;
        }

        // Ensure Kubo binary exists
        if self.kubo_binary.is_none() {
            return Err("kubo not found. Run: elastos setup --with kubo".to_string());
        }

        // Use lockfile protocol to safely start Kubo
        let started = self.start_kubo_with_lock();
        if started.is_ok() {
            self.apply_peering_to_running_kubo();
        }
        started
    }

    /// Tag configured peers on the *live* daemon. kubo does not persist these
    /// ("not saved to the config" per its own help), which is why start_kubo
    /// also writes Peering.Peers into the repo. Never fatal: a peering add
    /// failing must not fail the operation that triggered the start.
    fn apply_peering_to_running_kubo(&mut self) {
        if self.peering_applied || self.peering.is_empty() || self.api_port == 0 {
            return;
        }

        let mut all_ok = true;
        for peer in &self.peering {
            for multiaddr in peering_multiaddrs(peer) {
                let url = format!(
                    "http://127.0.0.1:{}/api/v0/swarm/peering/add?arg={}",
                    self.api_port, multiaddr
                );
                match ureq::post(&url).timeout(Duration::from_secs(5)).call() {
                    Ok(resp) if resp.status() == 200 => {
                        eprintln!("ipfs-provider: peering add {}", multiaddr);
                    }
                    Ok(resp) => {
                        all_ok = false;
                        eprintln!(
                            "ipfs-provider: peering add {} -> HTTP {}",
                            multiaddr,
                            resp.status()
                        );
                    }
                    Err(e) => {
                        all_ok = false;
                        eprintln!("ipfs-provider: peering add {} -> {}", multiaddr, e);
                    }
                }
            }
        }

        // Only latch on a clean sweep. An adopted daemon has no Peering.Peers
        // in its repo config, so this runtime add is the sole mechanism there;
        // latching on failure would strand the node for the daemon's lifetime.
        self.peering_applied = all_ok;
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
        //
        // The read-only HTTP gateway binds 0.0.0.0, not loopback: inside a
        // container a loopback-bound gateway is unreachable from the host and
        // from sibling nodes, so co-operating nodes cannot serve content to
        // each other over it. The read-WRITE admin API below stays on
        // 127.0.0.1 -- that one must never leave the node.
        let gw_addr = format!("/ip4/0.0.0.0/tcp/{}", gw_port);
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

        // Persist peering in the repo so it survives the idle-watcher kill and
        // any later cold start; runtime swarm/peering/add calls do not.
        {
            // Write an empty list too, so disabling removes persisted defaults.
            let peers_json = peering_peers_config_json(&self.peering).to_string();
            let peering_cfg = Command::new(binary)
                .args(["config", "--json", "Peering.Peers", &peers_json])
                .env("IPFS_PATH", &self.repo_dir)
                .output()
                .map_err(|e| format!("Failed to set Kubo Peering.Peers: {}", e))?;
            if !peering_cfg.status.success() {
                let stderr = String::from_utf8_lossy(&peering_cfg.stderr);
                return Err(format!(
                    "kubo config Peering.Peers failed: {}",
                    stderr.trim()
                ));
            }
        }

        // Reconcile the optional Elacity peer while preserving other bootstrap peers.
        // Peering above also keeps its swarm connection protected and retries
        // it in the background when the peer is temporarily unavailable.
        // `bootstrap rm` rejects individual removals when `auto` is present.
        // Edit only our entries in the config list, keeping AutoConf enabled.
        let current_bootstrap = Command::new(binary)
            .args(["config", "Bootstrap"])
            .env("IPFS_PATH", &self.repo_dir)
            .output()
            .map_err(|e| format!("Failed to read Kubo bootstrap peers: {e}"))?;
        if !current_bootstrap.status.success() {
            return Err(format!(
                "kubo config Bootstrap failed: {}",
                String::from_utf8_lossy(&current_bootstrap.stderr).trim()
            ));
        }
        let peers: Vec<String> = serde_json::from_slice(&current_bootstrap.stdout)
            .map_err(|e| format!("Invalid Kubo bootstrap peers: {e}"))?;
        let peers = elacity_bootstrap_peers(peers, self.elacity_peer_enabled);
        let bootstrap = Command::new(binary)
            .args([
                "config",
                "--json",
                "Bootstrap",
                &serde_json::json!(peers).to_string(),
            ])
            .env("IPFS_PATH", &self.repo_dir)
            .output()
            .map_err(|e| format!("Failed to configure Kubo storage bootstrap peer: {e}"))?;
        if !bootstrap.status.success() {
            return Err(format!(
                "kubo config Bootstrap failed: {}",
                String::from_utf8_lossy(&bootstrap.stderr).trim()
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
        if let Err(e) = validate_dest_path(&self.data_dir, dest_path) {
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
        let started = Instant::now();
        // Dropped when this returns, which stops the probe.
        let _probe = KuboPinProbe::start(self.api_url(), cid.to_string());
        let outcome = ureq::post(&url).timeout(LARGE_HTTP_TIMEOUT).call();
        let elapsed_ms = started.elapsed().as_millis();
        // The timeout is reported next to the elapsed time on purpose: a pin
        // that ends a hair either side of its own deadline should never be
        // mistaken for one that simply took that long.
        match outcome {
            Ok(resp) if resp.status() == 200 => {
                eprintln!(
                    "ipfs-provider: pin settled cid={} outcome=ok elapsed_ms={} timeout_ms={}",
                    cid,
                    elapsed_ms,
                    LARGE_HTTP_TIMEOUT.as_millis()
                );
                Response::ok_empty()
            }
            Ok(resp) => {
                eprintln!(
                    "ipfs-provider: pin settled cid={} outcome=http_{} elapsed_ms={} timeout_ms={}",
                    cid,
                    resp.status(),
                    elapsed_ms,
                    LARGE_HTTP_TIMEOUT.as_millis()
                );
                Response::error("pin_failed", &format!("HTTP {}", resp.status()))
            }
            Err(e) => {
                eprintln!(
                    "ipfs-provider: pin settled cid={} outcome=error elapsed_ms={} timeout_ms={} error={}",
                    cid,
                    elapsed_ms,
                    LARGE_HTTP_TIMEOUT.as_millis(),
                    e
                );
                Response::error("pin_failed", &e.to_string())
            }
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

    /// Peer id + swarm addrs of the running daemon, so the runtime can build
    /// other nodes' peering lists. Returns None rather than starting kubo:
    /// status is a supported cold call.
    fn kubo_identity(&self) -> Option<(String, Vec<String>)> {
        if self.state != KuboState::Ready || self.api_port == 0 {
            return None;
        }

        let url = format!("{}/api/v0/id", self.api_url());
        let resp = ureq::post(&url)
            .timeout(Duration::from_secs(5))
            .call()
            .ok()?;
        if resp.status() != 200 {
            return None;
        }
        let json: serde_json::Value = resp.into_json().ok()?;
        let id = json.get("ID")?.as_str()?.to_string();
        let addrs = json
            .get("Addresses")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Some((id, addrs))
    }

    /// Force the daemon up (or adopt a running one) and report its identity.
    ///
    /// The provider host calls this at startup so peering is established long
    /// before a mint needs replicas: peering only protects connections that
    /// exist, and a kubo first spawned seconds before a publish has no DHT
    /// provider record and no peers that have ever met it. Unlike `status`,
    /// which must stay cold-safe, this deliberately starts kubo.
    fn ensure_started(&mut self) -> Response {
        if let Err(e) = self.ensure_kubo() {
            return Response::error("kubo_unavailable", &e);
        }

        match self.kubo_identity() {
            Some((peer_id, swarm_addrs)) => Response::ok(serde_json::json!({
                "state": self.state,
                "peer_id": peer_id,
                "swarm_addrs": swarm_addrs,
            })),
            // Reporting success with null identity would let the caller write a
            // readiness receipt no other node can peer with.
            None => Response::error(
                "kubo_unavailable",
                "kubo is running but /api/v0/id returned no peer identity",
            ),
        }
    }

    fn status(&self) -> Response {
        let identity = self.kubo_identity();
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
            "peer_id": identity.as_ref().map(|(id, _)| id.clone()),
            "swarm_addrs": identity.as_ref().map(|(_, addrs)| addrs.clone()),
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
        #[cfg(target_os = "macos")]
        {
            PathBuf::from(home).join("Library/Application Support/elastos")
        }
        #[cfg(not(target_os = "macos"))]
        {
            PathBuf::from(home).join(".local/share/elastos")
        }
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
    fn test_ensure_started_request_deserialization() {
        let json = r#"{"op":"ensure_started"}"#;
        let req: Request = serde_json::from_str(json).expect("Should parse ensure_started");
        assert!(matches!(req, Request::EnsureStarted { .. }));

        let with_metadata = r#"{"op":"ensure_started","_runtime_invocation":{"caller":"host"}}"#;
        let req: Request =
            serde_json::from_str(with_metadata).expect("Should parse runtime envelope");
        assert!(matches!(req, Request::EnsureStarted { .. }));
    }

    #[test]
    fn test_ensure_started_rejects_unknown_fields() {
        let json = r#"{"op":"ensure_started","admin":true}"#;
        let err = serde_json::from_str::<Request>(json).expect_err("Should reject unknown fields");
        assert!(err.to_string().contains("unknown field `admin`"));
    }

    #[test]
    fn test_init_parses_peering_list() {
        let tmp = tempfile::tempdir().unwrap();
        let mut provider = IpfsProvider::new();
        let response = provider.init(serde_json::json!({
            "base_path": tmp.path().to_string_lossy(),
            "extra": {
                "peering": [
                    {
                        "id": "12D3KooWAlpha",
                        "addrs": ["/ip4/172.19.0.1/tcp/4001", "/ip4/172.19.0.1/udp/4001/quic-v1"]
                    },
                    { "id": "12D3KooWBeta" }
                ]
            }
        }));

        assert!(matches!(response, Response::Ok { .. }));
        assert_eq!(
            provider.peering,
            vec![
                PeeringPeer {
                    id: "12D3KooWAlpha".to_string(),
                    addrs: vec![
                        "/ip4/172.19.0.1/tcp/4001".to_string(),
                        "/ip4/172.19.0.1/udp/4001/quic-v1".to_string(),
                    ],
                },
                PeeringPeer {
                    id: "12D3KooWBeta".to_string(),
                    addrs: vec![],
                },
                default_elacity_storage_peer(),
            ]
        );
    }

    #[test]
    fn test_elacity_peer_switch_defaults_on_and_can_disable_automatic_peering() {
        assert!(elacity_peer_enabled(None).unwrap());
        assert!(elacity_peer_enabled(Some("true")).unwrap());
        assert!(!elacity_peer_enabled(Some("false")).unwrap());
        assert!(elacity_peer_enabled(Some("flase")).is_err());
        assert!(with_elacity_peer(Vec::new(), false).is_empty());
        let operator = PeeringPeer {
            id: "12D3KooWOperator".to_string(),
            addrs: vec!["/ip4/127.0.0.1/tcp/4001".to_string()],
        };
        assert_eq!(
            with_elacity_peer(vec![operator.clone()], false),
            vec![operator]
        );
    }

    #[test]
    fn test_elacity_bootstrap_toggle_preserves_auto_and_other_peers() {
        let existing = vec![
            "auto".to_string(),
            "/dns4/other.example/tcp/4001/p2p/12D3KooWOther".to_string(),
        ];
        let enabled = elacity_bootstrap_peers(existing.clone(), true);
        assert_eq!(enabled.len(), existing.len() + 2);
        assert_eq!(elacity_bootstrap_peers(enabled.clone(), true), enabled);
        assert_eq!(elacity_bootstrap_peers(enabled, false), existing);
        assert_eq!(elacity_bootstrap_peers(existing.clone(), false), existing);
    }

    #[test]
    fn test_init_without_peering_includes_storage_peer() {
        let tmp = tempfile::tempdir().unwrap();
        let mut provider = IpfsProvider::new();
        let response = provider.init(serde_json::json!({
            "base_path": tmp.path().to_string_lossy(),
            "extra": {}
        }));

        assert!(matches!(response, Response::Ok { .. }));
        assert_eq!(provider.peering, vec![default_elacity_storage_peer()]);
        assert_eq!(
            peering_multiaddrs(&provider.peering[0]),
            vec![
                "/ip4/34.77.31.164/tcp/4001/p2p/12D3KooWNieM3HRBJdVqaQucZEJdqA3oWKrKf3Gx3hp2cmtR9GNK",
                "/ip4/34.77.31.164/udp/4001/quic-v1/p2p/12D3KooWNieM3HRBJdVqaQucZEJdqA3oWKrKf3Gx3hp2cmtR9GNK",
            ]
        );
    }

    #[test]
    fn test_init_merges_storage_peer_without_duplicate_addresses() {
        let tmp = tempfile::tempdir().unwrap();
        let mut provider = IpfsProvider::new();
        let peer = default_elacity_storage_peer();
        let response = provider.init(serde_json::json!({
            "base_path": tmp.path().to_string_lossy(),
            "extra": {"peering": [{
                "id": peer.id,
                "addrs": [peer.addrs[0], "/dns4/storage.example/tcp/4001"]
            }]}
        }));
        assert!(matches!(response, Response::Ok { .. }));
        assert_eq!(provider.peering.len(), 1);
        assert_eq!(
            provider.peering[0].addrs,
            vec![
                peer.addrs[0].clone(),
                "/dns4/storage.example/tcp/4001".to_string(),
                peer.addrs[1].clone(),
            ]
        );
    }

    #[test]
    fn test_init_rejects_malformed_peering() {
        let cases = [
            serde_json::json!({"extra": {"peering": "12D3KooWAlpha"}}),
            serde_json::json!({"extra": {"peering": ["12D3KooWAlpha"]}}),
            serde_json::json!({"extra": {"peering": [{"addrs": []}]}}),
            serde_json::json!({"extra": {"peering": [{"id": ""}]}}),
            serde_json::json!({"extra": {"peering": [{"id": "12D3Koo/../W"}]}}),
            serde_json::json!({"extra": {"peering": [{"id": "12D3KooWAlpha", "addrs": "/ip4/1.2.3.4/tcp/4001"}]}}),
            serde_json::json!({"extra": {"peering": [{"id": "12D3KooWAlpha", "addrs": [4001]}]}}),
        ];

        for case in cases {
            let mut provider = IpfsProvider::new();
            match provider.init(case.clone()) {
                Response::Error { code, .. } => assert_eq!(code, "invalid_config", "{case}"),
                other => panic!("Expected error for {case}, got {other:?}"),
            }
            assert!(provider.peering.is_empty(), "{case}");
        }
    }

    #[test]
    fn test_peering_peers_config_json_uses_kubo_field_names() {
        let peers = vec![
            PeeringPeer {
                id: "12D3KooWAlpha".to_string(),
                addrs: vec!["/ip4/172.19.0.1/tcp/4001".to_string()],
            },
            PeeringPeer {
                id: "12D3KooWBeta".to_string(),
                addrs: vec![],
            },
        ];

        assert_eq!(
            peering_peers_config_json(&peers).to_string(),
            r#"[{"Addrs":["/ip4/172.19.0.1/tcp/4001"],"ID":"12D3KooWAlpha"},{"Addrs":[],"ID":"12D3KooWBeta"}]"#
        );
    }

    #[test]
    fn test_peering_multiaddrs() {
        let with_addrs = PeeringPeer {
            id: "12D3KooWAlpha".to_string(),
            addrs: vec![
                "/ip4/172.19.0.1/tcp/4001".to_string(),
                "/ip4/172.19.0.1/udp/4001/quic-v1".to_string(),
            ],
        };
        assert_eq!(
            peering_multiaddrs(&with_addrs),
            vec![
                "/ip4/172.19.0.1/tcp/4001/p2p/12D3KooWAlpha".to_string(),
                "/ip4/172.19.0.1/udp/4001/quic-v1/p2p/12D3KooWAlpha".to_string(),
            ]
        );

        let bare = PeeringPeer {
            id: "12D3KooWBeta".to_string(),
            addrs: vec![],
        };
        assert_eq!(
            peering_multiaddrs(&bare),
            vec!["/p2p/12D3KooWBeta".to_string()]
        );
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
}
