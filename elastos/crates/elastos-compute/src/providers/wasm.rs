//! WASM compute provider using wasmtime with WASI support

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use wasi_common::pipe::{ReadPipe, WritePipe};
use wasmtime::*;
use wasmtime_wasi::sync::WasiCtxBuilder;
use wasmtime_wasi::WasiCtx;

use elastos_common::{
    CapsuleId, CapsuleManifest, CapsuleStatus, CapsuleType, ElastosError, Result,
};

use crate::{CapsuleHandle, CapsuleInfo, ComputeProvider};

/// State held by a WASI instance
struct WasiState {
    wasi: WasiCtx,
}

/// A running WASM instance
struct RunningInstance {
    engine: Engine,
    module: Module,
    status: CapsuleStatus,
    manifest: CapsuleManifest,
    /// Data directory for this capsule (if storage permissions granted)
    _data_dir: Option<PathBuf>,
}

/// Pipe handles returned when bridge mode is active.
/// The caller (runtime) reads from `capsule_stdout` and writes to `capsule_stdin`
/// to bridge the capsule's SDK requests to the provider registry.
pub struct BridgePipes {
    /// Read end of the capsule's stdout pipe — runtime reads SDK requests here
    pub capsule_stdout: std::fs::File,
    /// Write end of the capsule's stdin pipe — runtime writes SDK responses here
    pub capsule_stdin: std::fs::File,
}

/// Callback invoked when a WASM capsule starts with bridge mode.
/// Receives the pipe handles for bridging capsule stdio to providers.
/// The callback should spawn a bridge thread/task and return immediately.
pub type BridgeSpawner = Arc<dyn Fn(BridgePipes) + Send + Sync>;

/// WASM compute provider using wasmtime with WASI support
pub struct WasmProvider {
    instances: Arc<RwLock<HashMap<CapsuleId, RunningInstance>>>,
    /// Base directory for capsule data
    data_base_dir: PathBuf,
    /// Optional bridge spawner. When set, capsule stdout/stdin are piped
    /// instead of inherited, and this callback is invoked to handle the
    /// bridge (e.g., dispatching SDK requests to the provider registry).
    bridge_spawner: std::sync::RwLock<Option<BridgeSpawner>>,
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

    /// Get or create the data directory for a capsule
    fn get_capsule_data_dir(&self, capsule_name: &str) -> PathBuf {
        self.data_base_dir.join(capsule_name)
    }

    /// Carrier channel FD numbers inserted into the WASI context for bridge mode.
    /// The capsule reads responses from `CARRIER_READ_FD` and writes requests to
    /// `CARRIER_WRITE_FD`, keeping stdin/stdout free for user I/O.
    const CARRIER_READ_FD: u32 = 3;
    const CARRIER_WRITE_FD: u32 = 4;

    /// Build WASI context based on capsule permissions.
    ///
    /// When `use_bridge` is true, dedicated Carrier fds (3 and 4) are created
    /// via OS pipes and `BridgePipes` is returned for the caller to set up the
    /// bridge. stdin/stdout remain inherited for user I/O.
    /// When false, host stdio is inherited directly with no bridge.
    fn build_wasi_context(
        &self,
        manifest: &CapsuleManifest,
        capsule_id: &str,
        args: &[String],
        use_bridge: bool,
    ) -> Result<(WasiCtx, Option<PathBuf>, Option<BridgePipes>)> {
        let mut builder = WasiCtxBuilder::new();

        // Always inherit stdio for user I/O and debug output.
        builder.inherit_stdout();
        builder.inherit_stderr();
        builder.inherit_stdin();

        // Pass CLI args to WASM (prepend capsule name as argv[0] per convention)
        if !args.is_empty() {
            let mut wasi_args = vec![manifest.name.clone()];
            wasi_args.extend_from_slice(args);
            builder
                .args(&wasi_args)
                .map_err(|e| ElastosError::Compute(format!("Failed to set args: {}", e)))?;
        }

        // Set environment variables
        builder
            .env("ELASTOS_CAPSULE_NAME", &manifest.name)
            .map_err(|e| ElastosError::Compute(format!("Failed to set env: {}", e)))?;
        builder
            .env("ELASTOS_CAPSULE_ID", capsule_id)
            .map_err(|e| ElastosError::Compute(format!("Failed to set env: {}", e)))?;

        // Tell the SDK to use dedicated carrier fds when bridge is active
        if use_bridge {
            builder
                .env(
                    "ELASTOS_CARRIER_FDS",
                    &format!("{},{}", Self::CARRIER_READ_FD, Self::CARRIER_WRITE_FD),
                )
                .map_err(|e| ElastosError::Compute(format!("Failed to set env: {}", e)))?;
        }

        // Forward select host environment variables to the capsule
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
                builder
                    .env(key, &val)
                    .map_err(|e| ElastosError::Compute(format!("Failed to set env: {}", e)))?;
            }
        }

        // Handle storage permissions
        let data_dir = if !manifest.permissions.storage.is_empty() {
            let dir = self.get_capsule_data_dir(&manifest.name);

            // Create the directory if it doesn't exist
            std::fs::create_dir_all(&dir)
                .map_err(|e| ElastosError::Compute(format!("Failed to create data dir: {}", e)))?;

            // Determine permissions
            let has_read = manifest.permissions.storage.iter().any(|s| s == "read");
            let has_write = manifest.permissions.storage.iter().any(|s| s == "write");

            if has_read || has_write {
                // Open directory with cap-std
                let cap_dir = wasmtime_wasi::sync::Dir::open_ambient_dir(
                    &dir,
                    wasmtime_wasi::sync::ambient_authority(),
                )
                .map_err(|e| ElastosError::Compute(format!("Failed to open data dir: {}", e)))?;

                // Pre-open the data directory at /data in the guest
                builder.preopened_dir(cap_dir, "/data").map_err(|e| {
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

        let wasi = builder.build();

        // Insert dedicated carrier channel fds when bridge is active.
        // fd 3 = capsule reads SDK responses (ReadPipe)
        // fd 4 = capsule writes SDK requests (WritePipe)
        // The bridge thread reads from the other end of fd 4's pipe
        // and writes responses to the other end of fd 3's pipe.
        let bridge_pipes = if use_bridge {
            use wasi_common::file::FileAccessMode;

            // Pipe for capsule → bridge (SDK requests)
            let (bridge_request_read, capsule_request_write) = Self::create_pipe()?;
            // Pipe for bridge → capsule (SDK responses)
            let (capsule_response_read, bridge_response_write) = Self::create_pipe()?;

            wasi.insert_file(
                Self::CARRIER_READ_FD,
                Box::new(ReadPipe::new(capsule_response_read)),
                FileAccessMode::READ,
            );
            wasi.insert_file(
                Self::CARRIER_WRITE_FD,
                Box::new(WritePipe::new(capsule_request_write)),
                FileAccessMode::WRITE,
            );

            Some(BridgePipes {
                capsule_stdout: bridge_request_read,
                capsule_stdin: bridge_response_write,
            })
        } else {
            None
        };

        Ok((wasi, data_dir, bridge_pipes))
    }

    /// Create an OS pipe pair, returning (read_end, write_end) as `std::fs::File`.
    fn create_pipe() -> Result<(std::fs::File, std::fs::File)> {
        let mut fds = [0i32; 2];
        let ret = unsafe { libc::pipe(fds.as_mut_ptr()) };
        if ret != 0 {
            return Err(ElastosError::Compute(format!(
                "Failed to create pipe: {}",
                std::io::Error::last_os_error()
            )));
        }
        let read_end = unsafe { std::os::unix::io::FromRawFd::from_raw_fd(fds[0]) };
        let write_end = unsafe { std::os::unix::io::FromRawFd::from_raw_fd(fds[1]) };
        Ok((read_end, write_end))
    }

    /// Execute a WASM module with WASI
    fn execute_wasm(engine: &Engine, module: &Module, wasi: WasiCtx) -> Result<()> {
        let mut store = Store::new(engine, WasiState { wasi });

        // Create linker and add WASI functions
        let mut linker = Linker::new(engine);
        wasmtime_wasi::add_to_linker(&mut linker, |state: &mut WasiState| &mut state.wasi)
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
        } else if let Some(main) = instance.get_func(&mut store, "main") {
            // Try main() as fallback
            main.typed::<(), ()>(&store)
                .map_err(|e| ElastosError::Compute(format!("Invalid main signature: {}", e)))?
                .call(&mut store, ())
                .map_err(|e| ElastosError::Compute(format!("WASM execution failed: {}", e)))?;
        } else {
            tracing::warn!("No _start or main function found in WASM module");
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

        // Create engine with default config
        let engine = Engine::default();

        // Compile the module
        let module = Module::from_file(&engine, &wasm_path)
            .map_err(|e| ElastosError::Compute(format!("Failed to compile WASM: {}", e)))?;

        let id = CapsuleId::new(format!("wasm-{}", uuid::Uuid::new_v4()));

        // Build WASI context to validate permissions early (no bridge for validation)
        let (_, data_dir, _) = self.build_wasi_context(&manifest, &id.0, &[], false)?;

        let instance = RunningInstance {
            engine,
            module,
            status: CapsuleStatus::Loading,
            manifest: manifest.clone(),
            _data_dir: data_dir,
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
        let (engine, module, manifest) = {
            let instances = self.instances.read().await;
            let instance = instances
                .get(&handle.id)
                .ok_or_else(|| ElastosError::CapsuleNotFound(handle.id.0.clone()))?;

            (
                instance.engine.clone(),
                instance.module.clone(),
                instance.manifest.clone(),
            )
        };

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
        let use_bridge = bridge_spawner.is_some();

        // Build fresh WASI context for this execution (with args from handle)
        let args = handle.args.clone();
        let (wasi, _, bridge_pipes) =
            self.build_wasi_context(&manifest, &handle.id.0, &args, use_bridge)?;

        // Spawn bridge if configured — must happen before WASM execution starts
        if let (Some(spawner), Some(pipes)) = (bridge_spawner, bridge_pipes) {
            tracing::info!("WASM bridge active for capsule '{}'", manifest.name);
            spawner(pipes);
        }

        // Execute in a blocking task since wasmtime execution is synchronous
        let result =
            tokio::task::spawn_blocking(move || Self::execute_wasm(&engine, &module, wasi))
                .await
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
            memory_used_mb: 0, // TODO: Track actual memory usage
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

    #[test]
    fn test_create_pipe() {
        use std::io::{Read, Write};

        let (mut read_end, mut write_end) = WasmProvider::create_pipe().unwrap();
        write_end.write_all(b"hello\n").unwrap();
        write_end.flush().unwrap();
        drop(write_end); // Close write end so read gets EOF

        let mut buf = String::new();
        read_end.read_to_string(&mut buf).unwrap();
        assert_eq!(buf, "hello\n");
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
            capsule_type: CapsuleType::Wasm,
            entrypoint: "missing.wasm".into(),
            requires: Vec::new(),
            provides: None,
            capabilities: Vec::new(),
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
