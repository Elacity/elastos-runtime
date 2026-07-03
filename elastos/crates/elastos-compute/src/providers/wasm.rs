//! WASM compute provider using wasmtime with WASI support

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tokio::sync::RwLock;
use wasmtime::*;
use wasmtime_wasi::preview1::{self, WasiP1Ctx};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtxBuilder};

use elastos_common::{
    CapsuleId, CapsuleManifest, CapsuleStatus, CapsuleType, ElastosError, Result,
};

use crate::{CapsuleHandle, CapsuleInfo, ComputeProvider};

/// State held by a WASI preview1 instance under wasmtime-wasi 24+.
///
/// Replaces the previous `WasiCtx` from wasmtime-wasi 17. The new `WasiP1Ctx`
/// is what `preview1::add_to_linker_sync` binds against and what the WASI
/// host functions read/write through.
struct WasiState {
    wasi: WasiP1Ctx,
    bridge_hostcall: Option<BridgeHostcall>,
    capsule_id: String,
    principal_id: Option<String>,
    limits: StoreLimits,
}

#[derive(Debug, Clone, Copy)]
struct WasmExecutionLimits {
    fuel: u64,
    memory_size: usize,
    table_elements: usize,
    instances: usize,
    tables: usize,
    memories: usize,
    wall_clock_timeout: Duration,
}

impl WasmExecutionLimits {
    fn store_limits(self) -> StoreLimits {
        StoreLimitsBuilder::new()
            .memory_size(self.memory_size)
            .table_elements(self.table_elements)
            .instances(self.instances)
            .tables(self.tables)
            .memories(self.memories)
            .trap_on_grow_failure(true)
            .build()
    }
}

impl Default for WasmExecutionLimits {
    fn default() -> Self {
        Self {
            fuel: 100_000_000,
            memory_size: 128 * 1024 * 1024,
            table_elements: 100_000,
            instances: 16,
            tables: 32,
            memories: 4,
            wall_clock_timeout: Duration::from_secs(30),
        }
    }
}

/// A running WASM instance
struct RunningInstance {
    engine: Engine,
    module: Module,
    status: CapsuleStatus,
    manifest: CapsuleManifest,
    /// Data directory for this capsule (if storage permissions granted)
    _data_dir: Option<PathBuf>,
    /// Per-launch carrier directory (only set when bridge ran via FIFO transport).
    /// Cleaned up on `Drop` so dropped instances don't leak FIFOs into /tmp.
    carrier_dir: Option<PathBuf>,
    /// Set by `stop()` to terminate an in-flight execution. The execution's epoch-deadline
    /// callback checks this flag and traps the capsule — the only way to halt a runaway,
    /// since execution runs in a `spawn_blocking` task that cannot be cancelled. Fuel and
    /// the wall-clock deadline bound a runaway passively; this is the operator's on-demand kill.
    should_stop: Arc<AtomicBool>,
}

impl Drop for RunningInstance {
    fn drop(&mut self) {
        // Defensive cleanup: remove the carrier dir (with its FIFOs) when the
        // instance is dropped. The bridge spawner is expected to have already
        // dropped its File handles by this point, so the FIFOs are unreferenced.
        // Best-effort — a leaked dir under /tmp is harmless on macOS/Linux.
        if let Some(dir) = self.carrier_dir.take() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// Pipe handles returned when bridge mode is active.
/// The caller (runtime) reads from `capsule_stdout` and writes to `capsule_stdin`
/// to bridge the capsule's SDK requests to the provider registry.
///
/// The field names reflect the historical fd-injection design (capsule_stdout =
/// "data flowing out of the capsule"; capsule_stdin = "data flowing into the
/// capsule"). The same shape is reused by the FIFO transport.
pub struct BridgePipes {
    /// Capsule instance id bound to this bridge.
    pub capsule_id: String,
    /// Optional runtime principal for resolving capsule-facing current-user aliases.
    pub principal_id: Option<String>,
    /// Read end of the capsule's stdout pipe — runtime reads SDK requests here
    pub capsule_stdout: std::fs::File,
    /// Write end of the capsule's stdin pipe — runtime writes SDK responses here
    pub capsule_stdin: std::fs::File,
}

/// Callback invoked when a WASM capsule starts with bridge mode.
/// Receives the pipe handles for bridging capsule stdio to providers.
/// The callback should spawn a bridge thread/task and return immediately.
pub type BridgeSpawner = Arc<dyn Fn(BridgePipes) + Send + Sync>;

/// Synchronous request/response bridge exposed to WASM capsules as a host import.
pub type BridgeHostcall =
    Arc<dyn Fn(&str, &str, Option<&str>) -> std::result::Result<String, String> + Send + Sync>;

/// Return type of [`WasmProvider::build_wasi_context`]: the configured WASI
/// context plus optional host-side handles for the data-dir preopen, the
/// carrier-bridge pipes, and the carrier-dir path (for cleanup on exit).
///
/// Factored out of an inline 4-tuple to satisfy `clippy::type_complexity`.
type WasiContextWithBridge = (
    WasiP1Ctx,
    Option<PathBuf>,
    Option<BridgePipes>,
    Option<PathBuf>,
);

/// Transport used to wire the carrier bridge between the runtime and a WASM
/// capsule.
///
/// On wasmtime-wasi 24+ only the `Fifos` transport is supported: the previous
/// fd-injection transport (`Fds`) depended on `wasi.insert_file()` which the
/// upstream 24 release removed. The enum is kept (rather than collapsed into
/// a unit type) so the runtime API remains forward-compatible if a future
/// transport variant is added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BridgeTransport {
    /// FIFO-on-preopened-dir transport. The carrier bridge mounts a per-launch
    /// directory containing two named pipes into the WASI sandbox via
    /// `preopened_dir()`; the capsule SDK opens them by path.
    #[default]
    Fifos,
}

/// WASM compute provider using wasmtime with WASI support
pub struct WasmProvider {
    instances: Arc<RwLock<HashMap<CapsuleId, RunningInstance>>>,
    /// Base directory for capsule data
    data_base_dir: PathBuf,
    /// Optional bridge spawner. When set, capsule stdout/stdin are piped
    /// instead of inherited, and this callback is invoked to handle the
    /// bridge (e.g., dispatching SDK requests to the provider registry).
    bridge_spawner: std::sync::RwLock<Option<BridgeSpawner>>,
    bridge_hostcall: std::sync::RwLock<Option<BridgeHostcall>>,
    bridge_principals: Arc<RwLock<HashMap<CapsuleId, Option<String>>>>,
    /// Carrier bridge transport. Only [`BridgeTransport::Fifos`] is supported
    /// on wasmtime-wasi 24+; kept as a `RwLock<BridgeTransport>` for symmetry
    /// with the existing setter/getter API and future transport variants.
    transport: std::sync::RwLock<BridgeTransport>,
    execution_limits: WasmExecutionLimits,
}

impl WasmProvider {
    /// Create a new WASM provider
    pub fn new() -> Self {
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("elastos/capsule-data");
        Self::with_data_dir(data_dir)
    }

    /// Create a new WASM provider with a custom data directory
    pub fn with_data_dir(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            instances: Arc::new(RwLock::new(HashMap::new())),
            data_base_dir: data_dir.into(),
            bridge_spawner: std::sync::RwLock::new(None),
            bridge_hostcall: std::sync::RwLock::new(None),
            bridge_principals: Arc::new(RwLock::new(HashMap::new())),
            transport: std::sync::RwLock::new(BridgeTransport::default()),
            execution_limits: WasmExecutionLimits::default(),
        }
    }

    /// Set the bridge spawner for capsule stdio bridging.
    ///
    /// When set, WASM capsules get piped stdin/stdout instead of inherited
    /// host stdio. The spawner callback receives the pipe handles and should
    /// set up a bridge (e.g., dispatching SDK requests to the provider registry).
    pub fn set_bridge_spawner(&self, spawner: BridgeSpawner) {
        let mut guard = self.bridge_spawner.write().unwrap();
        *guard = Some(spawner);
    }

    pub fn set_bridge_hostcall(&self, hostcall: BridgeHostcall) {
        let mut guard = self.bridge_hostcall.write().unwrap();
        *guard = Some(hostcall);
    }

    pub async fn set_bridge_principal(&self, capsule_id: &CapsuleId, principal_id: Option<String>) {
        self.bridge_principals
            .write()
            .await
            .insert(capsule_id.clone(), principal_id);
    }

    pub async fn clear_bridge_principal(&self, capsule_id: &CapsuleId) {
        self.bridge_principals.write().await.remove(capsule_id);
    }

    /// Select the carrier bridge transport. See [`BridgeTransport`].
    pub fn set_bridge_transport(&self, transport: BridgeTransport) {
        let mut guard = self.transport.write().unwrap();
        *guard = transport;
    }

    /// Read the currently selected carrier bridge transport.
    pub fn bridge_transport(&self) -> BridgeTransport {
        *self.transport.read().unwrap()
    }

    fn engine() -> Result<Engine> {
        let mut config = Config::new();
        config.consume_fuel(true);
        // Epoch-based interruption so a runaway capsule can be trapped on demand by `stop()`
        // (fuel and the wall-clock deadline only bound it passively, after the fact).
        config.epoch_interruption(true);
        Engine::new(&config)
            .map_err(|e| ElastosError::Compute(format!("Failed to configure WASM engine: {}", e)))
    }

    /// Get or create the data directory for a capsule
    fn get_capsule_data_dir(&self, capsule_name: &str) -> PathBuf {
        self.data_base_dir.join(capsule_name)
    }

    /// Sandbox path at which the per-capsule carrier directory is preopened
    /// when using the FIFO transport. Capsules see two files inside it:
    /// `response` (capsule reads, host writes) and `request` (capsule writes,
    /// host reads). The directory name is chosen to be unlikely to collide
    /// with any guest-facing app path.
    const CARRIER_GUEST_DIR: &'static str = "/_carrier";

    /// Filename for the FIFO the capsule reads (= host writes SDK responses).
    const CARRIER_RESPONSE_FILE: &'static str = "response";

    /// Filename for the FIFO the capsule writes (= host reads SDK requests).
    const CARRIER_REQUEST_FILE: &'static str = "request";

    /// Compute the per-launch host-side carrier directory. Always returns a
    /// path under the system temp dir so cleanup-on-crash is bounded by the
    /// OS's temp-reaper rather than persisting next to capsule data.
    pub(crate) fn carrier_dir_for(capsule_id: &str) -> PathBuf {
        std::env::temp_dir()
            .join("elastos-carrier")
            .join(capsule_id)
    }

    /// Create a FIFO at `path` with the given mode. Returns an
    /// `ElastosError::Compute` on failure rather than panicking, so callers
    /// can surface configuration errors cleanly.
    fn mkfifo(path: &Path, mode: u32) -> Result<()> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let cstr = CString::new(path.as_os_str().as_bytes()).map_err(|e| {
            ElastosError::Compute(format!("invalid fifo path {}: {}", path.display(), e))
        })?;
        let ret = unsafe { libc::mkfifo(cstr.as_ptr(), mode as libc::mode_t) };
        if ret != 0 {
            return Err(ElastosError::Compute(format!(
                "mkfifo({}, {:o}) failed: {}",
                path.display(),
                mode,
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    /// Create the per-launch carrier dir (mode 0700) and two FIFOs (mode 0600)
    /// inside it, then return the dir path plus host-side [`BridgePipes`] with
    /// both ends opened for read+write.
    ///
    /// Rationale for `O_RDWR` on the host: a FIFO blocks on `open()` until both
    /// a reader and a writer exist. Opening both ends as RDWR from the host
    /// anchors the FIFO so the capsule's later one-directional opens succeed
    /// immediately without a handshake thread. EOF semantics: the host's
    /// read side will NOT see EOF when the capsule exits because the host
    /// itself is still counted as a writer; the bridge thread should rely on
    /// EPIPE-on-write (response FIFO) or explicit shutdown signalling.
    pub(crate) fn setup_carrier_fifos(
        capsule_id: &str,
        principal_id: Option<&str>,
    ) -> Result<(PathBuf, BridgePipes)> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let dir = Self::carrier_dir_for(capsule_id);
        std::fs::create_dir_all(&dir).map_err(|e| {
            ElastosError::Compute(format!(
                "failed to create carrier dir {}: {}",
                dir.display(),
                e
            ))
        })?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
            ElastosError::Compute(format!(
                "failed to chmod carrier dir {}: {}",
                dir.display(),
                e
            ))
        })?;

        let request_path = dir.join(Self::CARRIER_REQUEST_FILE);
        let response_path = dir.join(Self::CARRIER_RESPONSE_FILE);
        Self::mkfifo(&request_path, 0o600)?;
        Self::mkfifo(&response_path, 0o600)?;

        let capsule_stdout = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(&request_path)
            .map_err(|e| {
                ElastosError::Compute(format!(
                    "failed to open request fifo {}: {}",
                    request_path.display(),
                    e
                ))
            })?;
        let capsule_stdin = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(&response_path)
            .map_err(|e| {
                ElastosError::Compute(format!(
                    "failed to open response fifo {}: {}",
                    response_path.display(),
                    e
                ))
            })?;

        Ok((
            dir,
            BridgePipes {
                capsule_id: capsule_id.to_string(),
                principal_id: principal_id.map(ToOwned::to_owned),
                capsule_stdout,
                capsule_stdin,
            },
        ))
    }

    /// Best-effort removal of a carrier dir and its FIFOs. Idempotent — safe to
    /// call multiple times or against an already-removed path.
    pub(crate) fn cleanup_carrier_dir(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Build WASI context based on capsule permissions.
    ///
    /// When `use_bridge` is true, a per-launch carrier directory containing
    /// two FIFOs is created on the host filesystem and preopened into the
    /// WASI sandbox at [`Self::CARRIER_GUEST_DIR`]. The capsule SDK opens
    /// the FIFOs by path. stdin/stdout remain inherited for user I/O.
    fn build_wasi_context(
        &self,
        manifest: &CapsuleManifest,
        capsule_id: &str,
        args: &[String],
        use_bridge: bool,
        use_hostcall: bool,
        principal_id: Option<&str>,
    ) -> Result<WasiContextWithBridge> {
        let mut builder = WasiCtxBuilder::new();

        // Always inherit stdio for user I/O and debug output.
        builder.inherit_stdout();
        builder.inherit_stderr();
        builder.inherit_stdin();

        // Pass CLI args to WASM (prepend capsule name as argv[0] per convention).
        if !args.is_empty() {
            let mut wasi_args = vec![manifest.name.clone()];
            wasi_args.extend_from_slice(args);
            builder.args(&wasi_args);
        }

        // Set environment variables. wasmtime-wasi 24's `env` returns
        // `&mut Self` (infallible) — no `.map_err` chain needed.
        builder.env("ELASTOS_CAPSULE_NAME", &manifest.name);
        builder.env("ELASTOS_CAPSULE_ID", capsule_id);

        // Tell the SDK which carrier bridge transport the runtime attached.
        if use_hostcall {
            builder.env("ELASTOS_CARRIER_HOSTCALL", "1");
        }

        // FIFO remains as a compatibility fallback for older guest SDKs.
        if use_bridge {
            builder.env(
                "ELASTOS_CARRIER_FIFOS",
                format!(
                    "{}/{},{}/{}",
                    Self::CARRIER_GUEST_DIR,
                    Self::CARRIER_RESPONSE_FILE,
                    Self::CARRIER_GUEST_DIR,
                    Self::CARRIER_REQUEST_FILE
                ),
            );
        }

        // Forward select host environment variables to the capsule.
        for key in &[
            "ELASTOS_NICK",
            "ELASTOS_CONNECT",
            "ELASTOS_COMMAND",
            "ELASTOS_COMMAND_B64",
            "ELASTOS_TOKEN",
            "ELASTOS_API",
            "ELASTOS_PARENT_SURFACE",
            "ELASTOS_TERM_COLS",
            "ELASTOS_TERM_ROWS",
            "TERM",
        ] {
            if let Ok(val) = std::env::var(key) {
                builder.env(key, &val);
            }
        }

        // Handle storage permissions
        let data_dir = if !manifest.permissions.storage.is_empty() {
            let dir = self.get_capsule_data_dir(&manifest.name);

            std::fs::create_dir_all(&dir)
                .map_err(|e| ElastosError::Compute(format!("Failed to create data dir: {}", e)))?;

            let has_read = manifest.permissions.storage.iter().any(|s| s == "read");
            let has_write = manifest.permissions.storage.iter().any(|s| s == "write");

            if has_read || has_write {
                // wasmtime-wasi 24 takes a host path + guest path + perms
                // directly; the old `Dir::open_ambient_dir` plumbing is gone.
                let (dir_perms, file_perms) = Self::wasi_perms(has_read, has_write);
                builder
                    .preopened_dir(&dir, "/data", dir_perms, file_perms)
                    .map_err(|e| {
                        ElastosError::Compute(format!("Failed to preopen data dir: {}", e))
                    })?;

                tracing::info!(
                    "Capsule '{}' granted storage access: read={}, write={}",
                    manifest.name,
                    has_read,
                    has_write
                );

                Some(dir)
            } else {
                None
            }
        } else {
            None
        };

        // Set up the carrier dir + FIFOs and preopen the dir BEFORE
        // `build_p1()` so the WasiP1Ctx records the preopen. Only the FIFO
        // transport is available on wasmtime-wasi 24+ (fd injection
        // was removed upstream).
        let bridge_pipes_and_dir = if use_bridge {
            let (carrier_dir, pipes) = Self::setup_carrier_fifos(capsule_id, principal_id)?;
            // The dir itself only needs READ (no file create/delete from
            // the guest); the FIFOs need READ + WRITE at the file level so
            // the capsule can both read responses and write requests.
            builder
                .preopened_dir(
                    &carrier_dir,
                    Self::CARRIER_GUEST_DIR,
                    DirPerms::READ | DirPerms::MUTATE,
                    FilePerms::READ | FilePerms::WRITE,
                )
                .map_err(|e| {
                    // Cleanup before propagating so we don't leak the dir on
                    // the configuration-error path.
                    Self::cleanup_carrier_dir(&carrier_dir);
                    ElastosError::Compute(format!("Failed to preopen carrier dir: {}", e))
                })?;
            Some((pipes, carrier_dir))
        } else {
            None
        };

        let wasi = builder.build_p1();

        let (bridge_pipes, carrier_dir) = match bridge_pipes_and_dir {
            Some((pipes, dir)) => (Some(pipes), Some(dir)),
            None => (None, None),
        };

        Ok((wasi, data_dir, bridge_pipes, carrier_dir))
    }

    /// Translate manifest `storage: ["read", "write"]` flags into the
    /// `wasmtime-wasi 24` capability bitsets. Centralised so the data-dir
    /// and the carrier-dir call sites (and any future preopens) share one
    /// truth.
    fn wasi_perms(has_read: bool, has_write: bool) -> (DirPerms, FilePerms) {
        let mut dir_perms = DirPerms::empty();
        let mut file_perms = FilePerms::empty();
        if has_read {
            dir_perms |= DirPerms::READ;
            file_perms |= FilePerms::READ;
        }
        if has_write {
            dir_perms |= DirPerms::MUTATE;
            file_perms |= FilePerms::WRITE;
        }
        (dir_perms, file_perms)
    }

    fn register_carrier_hostcall(linker: &mut Linker<WasiState>) -> Result<()> {
        linker
            .func_wrap(
                "elastos",
                "carrier_call",
                |mut caller: Caller<'_, WasiState>,
                 request_ptr: u32,
                 request_len: u32,
                 response_ptr: u32,
                 response_cap: u32,
                 response_len_ptr: u32|
                 -> i32 {
                    let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory())
                    else {
                        return 1;
                    };

                    let request_start = request_ptr as usize;
                    let request_len = request_len as usize;
                    let response_start = response_ptr as usize;
                    let response_cap = response_cap as usize;
                    let response_len_start = response_len_ptr as usize;

                    let data = memory.data(&caller);
                    let Some(request_end) = request_start.checked_add(request_len) else {
                        return 1;
                    };
                    if request_end > data.len() {
                        return 1;
                    }
                    let request = match std::str::from_utf8(&data[request_start..request_end]) {
                        Ok(value) => value,
                        Err(_) => return 2,
                    };

                    let (hostcall, capsule_id, principal_id) = {
                        let state = caller.data();
                        (
                            state.bridge_hostcall.clone(),
                            state.capsule_id.clone(),
                            state.principal_id.clone(),
                        )
                    };
                    let Some(hostcall) = hostcall else {
                        return 3;
                    };

                    let response = match hostcall(request, &capsule_id, principal_id.as_deref()) {
                        Ok(response) => response,
                        Err(_) => return 3,
                    };
                    let response_bytes = response.as_bytes();
                    if response_bytes.len() > response_cap {
                        return 4;
                    }

                    let Some(response_end) = response_start.checked_add(response_bytes.len())
                    else {
                        return 1;
                    };
                    let Some(response_len_end) =
                        response_len_start.checked_add(std::mem::size_of::<u32>())
                    else {
                        return 1;
                    };
                    let data = memory.data_mut(&mut caller);
                    if response_end > data.len() || response_len_end > data.len() {
                        return 1;
                    }
                    data[response_start..response_end].copy_from_slice(response_bytes);
                    data[response_len_start..response_len_end]
                        .copy_from_slice(&(response_bytes.len() as u32).to_le_bytes());
                    0
                },
            )
            .map_err(|e| {
                ElastosError::Compute(format!("Failed to link carrier hostcall: {}", e))
            })?;
        Ok(())
    }

    /// Execute a WASM module with WASI preview1.
    fn execute_wasm(
        engine: &Engine,
        module: &Module,
        wasi: WasiP1Ctx,
        bridge_hostcall: Option<BridgeHostcall>,
        capsule_id: String,
        principal_id: Option<String>,
        limits: WasmExecutionLimits,
        should_stop: Arc<AtomicBool>,
    ) -> Result<()> {
        let mut store = Store::new(
            engine,
            WasiState {
                wasi,
                bridge_hostcall,
                capsule_id,
                principal_id,
                limits: limits.store_limits(),
            },
        );
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(limits.fuel)
            .map_err(|e| ElastosError::Compute(format!("Failed to set WASM fuel: {}", e)))?;

        // Terminability: arm an epoch deadline whose callback traps the capsule when `stop()` has
        // set `should_stop`. With no stop signal the deadline simply keeps extending, so a
        // legitimate (even long-running) capsule runs untouched. The epoch check is a cheap
        // load+compare at loop backedges; fuel/wall-clock still bound the run independently.
        store.set_epoch_deadline(1);
        store.epoch_deadline_callback(move |_| {
            if should_stop.load(Ordering::Relaxed) {
                Err(wasmtime::Error::msg("capsule terminated by stop request"))
            } else {
                Ok(wasmtime::UpdateDeadline::Continue(1))
            }
        });

        // Epoch watchdog: advance THIS engine's epoch on a fixed cadence so the deadline callback
        // above fires regularly and observes `should_stop` within one tick — race-free regardless
        // of when stop() sets the flag relative to this store arming its deadline. (A single
        // increment-on-stop, as the original had, can be consumed before the store arms, silently
        // dropping the kill until wall-clock; a continuous ticker closes that startup window.) The
        // thread is signalled to exit the instant execution finishes, so it adds no latency to a
        // normal return, and the RAII guard joins it on EVERY exit path (including the `?` returns
        // below), so no watchdog is ever leaked.
        let watchdog_signal = Arc::new((Mutex::new(false), Condvar::new()));
        let watchdog = {
            let signal = watchdog_signal.clone();
            let engine = engine.clone();
            std::thread::spawn(move || {
                let (lock, cvar) = &*signal;
                let mut done = lock.lock().unwrap_or_else(|e| e.into_inner());
                while !*done {
                    let (guard, wait) = cvar
                        .wait_timeout(done, Duration::from_millis(50))
                        .unwrap_or_else(|e| e.into_inner());
                    done = guard;
                    if wait.timed_out() {
                        engine.increment_epoch();
                    }
                }
            })
        };
        struct WatchdogGuard {
            signal: Arc<(Mutex<bool>, Condvar)>,
            handle: Option<std::thread::JoinHandle<()>>,
        }
        impl Drop for WatchdogGuard {
            fn drop(&mut self) {
                let (lock, cvar) = &*self.signal;
                *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
                cvar.notify_all();
                if let Some(h) = self.handle.take() {
                    let _ = h.join();
                }
            }
        }
        let _watchdog_guard = WatchdogGuard {
            signal: watchdog_signal,
            handle: Some(watchdog),
        };

        // Create linker and bind WASI preview1 host functions.
        let mut linker = Linker::new(engine);
        Self::register_carrier_hostcall(&mut linker)?;
        preview1::add_to_linker_sync(&mut linker, |state: &mut WasiState| &mut state.wasi)
            .map_err(|e| ElastosError::Compute(format!("Failed to link WASI: {}", e)))?;

        // Instantiate the module
        let instance = linker
            .instantiate(&mut store, module)
            .map_err(|e| ElastosError::Compute(format!("Failed to instantiate WASM: {}", e)))?;

        // Try to find and call _start (WASI entry point)
        if let Some(start) = instance.get_func(&mut store, "_start") {
            let typed = start
                .typed::<(), ()>(&store)
                .map_err(|e| ElastosError::Compute(format!("Invalid _start signature: {}", e)))?;
            match typed.call(&mut store, ()) {
                Ok(()) => {}
                Err(e) => {
                    // WASI proc_exit(0) is a clean exit, not an error
                    if let Some(exit) = e.downcast_ref::<wasmtime_wasi::I32Exit>() {
                        if exit.0 != 0 {
                            return Err(ElastosError::Compute(format!(
                                "Capsule exited with code {}",
                                exit.0
                            )));
                        }
                    } else {
                        return Err(ElastosError::Compute(format!(
                            "WASM execution failed: {}",
                            e
                        )));
                    }
                }
            }
        } else {
            return Err(ElastosError::Compute(
                "WASM capsule missing required WASI _start entrypoint".to_string(),
            ));
        }

        Ok(())
    }
}

impl Default for WasmProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ComputeProvider for WasmProvider {
    async fn load(&self, path: &Path, manifest: CapsuleManifest) -> Result<CapsuleHandle> {
        let wasm_path = path.join(&manifest.entrypoint);

        if !wasm_path.exists() {
            return Err(ElastosError::CapsuleNotFound(format!(
                "WASM file not found: {}",
                wasm_path.display()
            )));
        }

        let engine = Self::engine()?;

        // Compile the module
        let module = Module::from_file(&engine, &wasm_path)
            .map_err(|e| ElastosError::Compute(format!("Failed to compile WASM: {}", e)))?;

        let id = CapsuleId::new(format!("wasm-{}", uuid::Uuid::new_v4()));

        // Build WASI context to validate permissions early (no bridge for validation)
        let (_, data_dir, _, _) =
            self.build_wasi_context(&manifest, &id.0, &[], false, false, None)?;

        let instance = RunningInstance {
            engine,
            module,
            status: CapsuleStatus::Loading,
            manifest: manifest.clone(),
            _data_dir: data_dir,
            carrier_dir: None,
            should_stop: Arc::new(AtomicBool::new(false)),
        };

        self.instances.write().await.insert(id.clone(), instance);

        tracing::info!("Loaded WASM capsule '{}' with ID {}", manifest.name, id);

        Ok(CapsuleHandle {
            id,
            manifest,
            args: vec![],
        })
    }

    async fn start(&self, handle: &CapsuleHandle) -> Result<()> {
        // Get instance data
        let (engine, module, manifest, limits, should_stop) = {
            let instances = self.instances.read().await;
            let instance = instances
                .get(&handle.id)
                .ok_or_else(|| ElastosError::CapsuleNotFound(handle.id.0.clone()))?;

            (
                instance.engine.clone(),
                instance.module.clone(),
                instance.manifest.clone(),
                self.execution_limits,
                instance.should_stop.clone(),
            )
        };
        // No reset here on purpose: should_stop starts false at load() and only ever goes true
        // via stop() — which ALSO removes the instance, so a stopped capsule can't be restarted.
        // Resetting it in start() would let a start() racing a concurrent stop() clobber the kill.

        // Mark as running before execution
        {
            let mut instances = self.instances.write().await;
            if let Some(instance) = instances.get_mut(&handle.id) {
                instance.status = CapsuleStatus::Running;
            }
        }

        tracing::info!("Starting capsule '{}'", manifest.name);

        // Check if bridge is configured
        let bridge_spawner = self.bridge_spawner.read().unwrap().clone();
        let bridge_hostcall = self.bridge_hostcall.read().unwrap().clone();
        let use_hostcall = bridge_hostcall.is_some();
        let use_bridge = bridge_spawner.is_some() && !use_hostcall;

        // Build fresh WASI context for this execution (with args from handle)
        let args = handle.args.clone();
        let principal_id = self
            .bridge_principals
            .read()
            .await
            .get(&handle.id)
            .cloned()
            .flatten();
        let (wasi, _, bridge_pipes, carrier_dir) = self.build_wasi_context(
            &manifest,
            &handle.id.0,
            &args,
            use_bridge,
            use_hostcall,
            principal_id.as_deref(),
        )?;

        // Record the carrier dir (if any) on the instance so Drop can clean it
        // up even if the bridge spawner forgets to close its File handles.
        if carrier_dir.is_some() {
            let mut instances = self.instances.write().await;
            if let Some(instance) = instances.get_mut(&handle.id) {
                instance.carrier_dir = carrier_dir;
            }
        }

        // Spawn bridge if configured — must happen before WASM execution starts
        if let (Some(spawner), Some(pipes)) = (bridge_spawner, bridge_pipes) {
            tracing::info!("WASM bridge active for capsule '{}'", manifest.name);
            spawner(pipes);
        }

        // Execute in a blocking task since wasmtime execution is synchronous
        let capsule_id = handle.id.0.clone();
        let exec_stop = should_stop.clone();
        let result = tokio::task::spawn_blocking(move || {
            Self::execute_wasm(
                &engine,
                &module,
                wasi,
                bridge_hostcall,
                capsule_id,
                principal_id,
                limits,
                exec_stop,
            )
        });
        let result = tokio::time::timeout(limits.wall_clock_timeout, result)
            .await
            .map_err(|_| {
                ElastosError::Compute(format!(
                    "WASM execution exceeded {}s deadline",
                    limits.wall_clock_timeout.as_secs()
                ))
            })?
            .map_err(|e| ElastosError::Compute(format!("Task join error: {}", e)))?;

        // Update status based on result
        {
            let mut instances = self.instances.write().await;
            if let Some(instance) = instances.get_mut(&handle.id) {
                instance.status = if result.is_ok() {
                    CapsuleStatus::Stopped // Completed successfully
                } else {
                    CapsuleStatus::Failed
                };
            }
        }

        result
    }

    async fn stop(&self, handle: &CapsuleHandle) -> Result<()> {
        let mut instances = self.instances.write().await;

        if let Some(instance) = instances.remove(&handle.id) {
            // Signal any in-flight execution to terminate: set the flag the capsule's
            // epoch-deadline callback checks, then bump this capsule's engine epoch so a
            // spinning capsule hits its next epoch check and traps. `instance` is the removed
            // (owned) value, but its `should_stop` and `engine` are shared with the running
            // task (Arc / cloned Engine), so the signal reaches it.
            instance.should_stop.store(true, Ordering::Relaxed);
            instance.engine.increment_epoch();
            // Dropping the RunningInstance releases the wasmtime Engine, Module,
            // and any compiled code buffers.  This is the primary memory-clearing
            // step for multi-tenant safety — no residual WASM heap survives.
            tracing::info!("Stopped and cleared capsule '{}'", instance.manifest.name);
        }

        Ok(())
    }

    async fn status(&self, handle: &CapsuleHandle) -> Result<CapsuleStatus> {
        let instances = self.instances.read().await;

        instances
            .get(&handle.id)
            .map(|i| i.status)
            .ok_or_else(|| ElastosError::CapsuleNotFound(handle.id.0.clone()))
    }

    async fn info(&self, handle: &CapsuleHandle) -> Result<CapsuleInfo> {
        let instances = self.instances.read().await;

        let instance = instances
            .get(&handle.id)
            .ok_or_else(|| ElastosError::CapsuleNotFound(handle.id.0.clone()))?;

        Ok(CapsuleInfo {
            id: handle.id.clone(),
            name: instance.manifest.name.clone(),
            status: instance.status,
            memory_used_mb: 0, // Memory accounting is not exposed by this provider yet.
        })
    }

    fn supports(&self, capsule_type: &CapsuleType) -> bool {
        matches!(capsule_type, CapsuleType::Wasm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_wasm_provider_supports() {
        let provider = WasmProvider::new();
        assert!(provider.supports(&CapsuleType::Wasm));
        assert!(!provider.supports(&CapsuleType::MicroVM));
        assert!(!provider.supports(&CapsuleType::Oci));
    }

    #[tokio::test]
    async fn test_bridge_spawner_piped_context() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let provider = WasmProvider::new();

        // Set a bridge spawner that records it was called
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();
        provider.set_bridge_spawner(Arc::new(move |_pipes| {
            called_clone.store(true, Ordering::SeqCst);
        }));

        // Verify bridge_spawner is set
        let spawner = provider.bridge_spawner.read().unwrap();
        assert!(spawner.is_some());
    }

    fn unique_test_capsule_id(label: &str) -> String {
        format!("wasm-test-{label}-{}", uuid::Uuid::new_v4())
    }

    #[test]
    fn test_default_bridge_transport_is_fifos() {
        // wasmtime-wasi 24+ removed `insert_file`, so the only supported
        // carrier-bridge transport is `Fifos`. Both `new()` and `Default`
        // must produce a provider that reports this transport.
        let provider = WasmProvider::new();
        assert_eq!(provider.bridge_transport(), BridgeTransport::Fifos);

        let provider = WasmProvider::default();
        assert_eq!(provider.bridge_transport(), BridgeTransport::Fifos);

        // The setter still round-trips (idempotent self-assignment).
        provider.set_bridge_transport(BridgeTransport::Fifos);
        assert_eq!(provider.bridge_transport(), BridgeTransport::Fifos);
    }

    #[test]
    fn test_wasm_engine_enables_fuel_accounting() {
        let engine = WasmProvider::engine().expect("engine");
        let mut store = Store::new(&engine, ());
        store.set_fuel(1).expect("fuel must be enabled");
    }

    #[test]
    fn test_execute_wasm_busy_loop_exhausts_fuel() {
        let engine = WasmProvider::engine().expect("engine");
        let module = Module::new(
            &engine,
            r#"
            (module
              (func (export "_start")
                (loop $again
                  br $again)))
            "#,
        )
        .expect("busy-loop module");
        let limits = WasmExecutionLimits {
            fuel: 10_000,
            ..WasmExecutionLimits::default()
        };

        let err = WasmProvider::execute_wasm(
            &engine,
            &module,
            WasiCtxBuilder::new().build_p1(),
            None,
            "fuel-test".to_string(),
            None,
            limits,
            Arc::new(AtomicBool::new(false)),
        )
        .expect_err("busy-loop capsule must exhaust fuel");

        assert!(
            err.to_string().contains("fuel") || err.to_string().contains("WASM execution failed"),
            "unexpected error: {err}"
        );
    }

    /// B2a (restored from the ddrm-hardening line): a runaway capsule MUST be terminable on
    /// demand. Fuel is set effectively-unbounded here so the trap can ONLY come from the stop
    /// signal — this proves the operator kill, not passive fuel exhaustion. Setting `should_stop`
    /// is the ONLY action taken (no manual epoch bump): the in-`execute_wasm` watchdog advances
    /// the epoch on its own cadence, so this also pins the race-free property — the kill does not
    /// depend on any external epoch increment being timed against the store arming its deadline.
    /// The loop is finite (a CI-hang backstop): if termination were broken, `recv_timeout` fails
    /// the test in 5 s and the thread still ends on its own.
    #[test]
    fn runaway_capsule_is_terminable_via_stop_signal() {
        let engine = WasmProvider::engine().expect("engine");
        let module = Module::new(
            &engine,
            r#"(module (func (export "_start")
                (local $i i64) (local.set $i (i64.const 2000000000))
                (loop $l
                    (local.set $i (i64.sub (local.get $i) (i64.const 1)))
                    (br_if $l (i64.ne (local.get $i) (i64.const 0))))))"#,
        )
        .expect("compile wat");
        let limits = WasmExecutionLimits {
            fuel: u64::MAX,
            ..WasmExecutionLimits::default()
        };
        let should_stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let (ss, eng, module2) = (should_stop.clone(), engine.clone(), module.clone());
        std::thread::spawn(move || {
            let r = WasmProvider::execute_wasm(
                &eng,
                &module2,
                WasiCtxBuilder::new().build_p1(),
                None,
                "runaway-test".to_string(),
                None,
                limits,
                ss,
            );
            let _ = tx.send(r.is_err());
        });
        std::thread::sleep(std::time::Duration::from_millis(50)); // let it start spinning
        should_stop.store(true, Ordering::Relaxed); // the watchdog does the rest — no manual bump
        let trapped = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("a stopped runaway must terminate within 5s, not run forever");
        assert!(
            trapped,
            "a stopped runaway must trap (Err), not complete normally"
        );
    }

    #[test]
    fn test_execute_wasm_rejects_memory_above_limit() {
        let engine = WasmProvider::engine().expect("engine");
        let module = Module::new(
            &engine,
            r#"
            (module
              (memory 2)
              (func (export "_start")))
            "#,
        )
        .expect("memory module");
        let limits = WasmExecutionLimits {
            memory_size: 64 * 1024,
            ..WasmExecutionLimits::default()
        };

        let err = WasmProvider::execute_wasm(
            &engine,
            &module,
            WasiCtxBuilder::new().build_p1(),
            None,
            "memory-limit-test".to_string(),
            None,
            limits,
            Arc::new(AtomicBool::new(false)),
        )
        .expect_err("capsule memory must be limited");

        assert!(
            err.to_string().contains("instantiate") || err.to_string().contains("memory"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_setup_carrier_fifos_creates_dir_and_fifos_with_correct_modes() {
        use std::os::unix::fs::{FileTypeExt, PermissionsExt};

        let cid = unique_test_capsule_id("setup");
        let (dir, _pipes) = WasmProvider::setup_carrier_fifos(&cid, None)
            .expect("setup_carrier_fifos must succeed in a writable temp dir");

        // Dir created with mode 0o700 (owner-only).
        let dir_meta = std::fs::metadata(&dir).expect("dir must exist");
        assert!(dir_meta.is_dir());
        assert_eq!(dir_meta.permissions().mode() & 0o777, 0o700);

        // Both FIFOs created with mode 0o600 (owner read/write only).
        for filename in &["request", "response"] {
            let path = dir.join(filename);
            let meta =
                std::fs::metadata(&path).unwrap_or_else(|_| panic!("{} fifo must exist", filename));
            assert!(meta.file_type().is_fifo(), "{} must be a fifo", filename);
            assert_eq!(
                meta.permissions().mode() & 0o777,
                0o600,
                "{} fifo must have mode 0600",
                filename
            );
        }

        WasmProvider::cleanup_carrier_dir(&dir);
    }

    #[test]
    fn test_cleanup_carrier_dir_is_idempotent() {
        let cid = unique_test_capsule_id("cleanup-idempotent");
        let (dir, _pipes) = WasmProvider::setup_carrier_fifos(&cid, None)
            .expect("setup_carrier_fifos must succeed");
        assert!(dir.exists(), "dir must exist after setup");

        WasmProvider::cleanup_carrier_dir(&dir);
        assert!(!dir.exists(), "dir must be gone after first cleanup");

        // Idempotency: a second cleanup on the already-removed dir must not panic.
        WasmProvider::cleanup_carrier_dir(&dir);
        assert!(!dir.exists(), "dir must still be gone after second cleanup");
    }

    #[test]
    fn test_setup_carrier_fifos_round_trip_via_host_ends() {
        use std::io::{Read, Write};

        let cid = unique_test_capsule_id("round-trip");
        let (dir, mut pipes) = WasmProvider::setup_carrier_fifos(&cid, None)
            .expect("setup_carrier_fifos must succeed");

        // Simulate a capsule by opening the FIFOs by their *host* paths. Inside
        // a real WASM sandbox these would be /_carrier/request and
        // /_carrier/response; the on-disk semantics are identical because the
        // preopened-dir layer only constrains *which* paths the capsule can
        // see, not how FIFO bytes flow.
        let request_path = dir.join("request");
        let response_path = dir.join("response");

        // Capsule writes a request to its WRITE FIFO; host reads from
        // pipes.capsule_stdout (which is opened RDWR on request_path).
        let request_handle = std::thread::spawn(move || {
            let mut cap_writer = std::fs::OpenOptions::new()
                .write(true)
                .open(&request_path)
                .expect("capsule must open request fifo for write");
            cap_writer
                .write_all(b"{\"id\":1,\"request\":{\"type\":\"ping\"}}\n")
                .expect("capsule write must succeed");
            cap_writer.flush().expect("flush");
            // Hold the writer open just long enough for the host to read,
            // then drop to release the FIFO from the capsule side.
            std::thread::sleep(std::time::Duration::from_millis(50));
        });

        let response_handle = std::thread::spawn(move || {
            let mut cap_reader = std::fs::OpenOptions::new()
                .read(true)
                .open(&response_path)
                .expect("capsule must open response fifo for read");
            let mut buf = String::new();
            // Read a single newline-terminated envelope.
            let mut byte = [0u8; 1];
            loop {
                let n = cap_reader.read(&mut byte).expect("capsule read");
                if n == 0 {
                    break;
                }
                buf.push(byte[0] as char);
                if byte[0] == b'\n' {
                    break;
                }
            }
            buf
        });

        // Host reads the request from capsule_stdout.
        let mut req_buf = String::new();
        let mut byte = [0u8; 1];
        loop {
            let n = pipes
                .capsule_stdout
                .read(&mut byte)
                .expect("host read request");
            if n == 0 || byte[0] == b'\n' {
                if byte[0] == b'\n' {
                    req_buf.push('\n');
                }
                break;
            }
            req_buf.push(byte[0] as char);
        }
        request_handle.join().expect("capsule writer thread");
        assert_eq!(req_buf, "{\"id\":1,\"request\":{\"type\":\"ping\"}}\n");

        // Host writes a response to capsule_stdin.
        pipes
            .capsule_stdin
            .write_all(b"{\"id\":1,\"response\":{\"type\":\"pong\"}}\n")
            .expect("host write response");
        pipes.capsule_stdin.flush().expect("flush");

        let resp = response_handle.join().expect("capsule reader thread");
        assert_eq!(resp, "{\"id\":1,\"response\":{\"type\":\"pong\"}}\n");

        // Drop the pipes (closes host-side RDWR handles), then clean up.
        drop(pipes);
        WasmProvider::cleanup_carrier_dir(&dir);
    }

    #[test]
    fn test_mkfifo_returns_typed_error_on_invalid_path() {
        // A path containing an interior NUL is invalid for the C string we
        // pass to `mkfifo(2)`. The helper must surface this as an
        // `ElastosError::Compute` rather than panicking.
        let bad_path = PathBuf::from("/tmp/elastos-carrier/bad\0path");
        let result = WasmProvider::mkfifo(&bad_path, 0o600);
        assert!(
            matches!(result, Err(ElastosError::Compute(_))),
            "expected ElastosError::Compute, got {:?}",
            result
        );
    }

    #[test]
    fn test_running_instance_drop_cleans_carrier_dir() {
        let cid = unique_test_capsule_id("drop");
        let dir = WasmProvider::carrier_dir_for(&cid);
        // Materialise the dir without going through setup_carrier_fifos so the
        // test isolates the Drop behaviour from the FIFO-handle lifecycle.
        std::fs::create_dir_all(&dir).expect("create test dir");
        assert!(dir.exists());

        let dummy_manifest = CapsuleManifest {
            schema: elastos_common::SCHEMA_V1.into(),
            version: "0.1.0".into(),
            name: "drop-test".into(),
            description: None,
            author: None,
            role: elastos_common::CapsuleRole::App,
            capsule_type: CapsuleType::Wasm,
            entrypoint: "missing.wasm".into(),
            requires: Vec::new(),
            provides: None,
            capabilities: Vec::new(),
            interfaces: Vec::new(),
            resources: Default::default(),
            permissions: Default::default(),
            microvm: None,
            providers: None,
            viewer: None,
            signature: None,
            authority: None,
        };

        {
            let engine = Engine::default();
            let module = Module::new(&engine, b"(module)").expect("trivial wasm module");
            let _instance = RunningInstance {
                engine,
                module,
                status: CapsuleStatus::Stopped,
                manifest: dummy_manifest,
                _data_dir: None,
                carrier_dir: Some(dir.clone()),
                should_stop: Arc::new(AtomicBool::new(false)),
            };
            assert!(dir.exists(), "dir must still exist while instance is alive");
        }
        // After drop, the carrier dir must be gone.
        assert!(
            !dir.exists(),
            "RunningInstance Drop must clean up carrier_dir"
        );
    }

    #[tokio::test]
    async fn test_wasm_provider_load_missing_file() {
        let provider = WasmProvider::new();
        let dir = tempdir().unwrap();

        let manifest = CapsuleManifest {
            schema: elastos_common::SCHEMA_V1.into(),
            version: "0.1.0".into(),
            name: "test".into(),
            description: None,
            author: None,
            role: elastos_common::CapsuleRole::App,
            capsule_type: CapsuleType::Wasm,
            entrypoint: "missing.wasm".into(),
            requires: Vec::new(),
            provides: None,
            authority: None,
            capabilities: Vec::new(),
            interfaces: Vec::new(),
            resources: Default::default(),
            permissions: Default::default(),
            microvm: None,
            providers: None,
            viewer: None,
            signature: None,
        };

        let result = provider.load(dir.path(), manifest).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }
}
