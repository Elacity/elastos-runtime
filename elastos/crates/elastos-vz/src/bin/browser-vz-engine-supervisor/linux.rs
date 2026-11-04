//! Linux substrate for the shared Browser transport/lifecycle supervisor.
//! Only this module launches crosvm. The parent owns bootstrap, streams, TURN
//! policy, profile durability and the exact terminal settlement.
use super::{LockFile, SpawnGated};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, AtomicU8, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

use elastos_common::{CapsuleId, CapsuleManifest};
use elastos_compute::CapsuleHandle;
use elastos_vz::{VmConfig, VzConfig};

const LOADED: u8 = 0;
const RUNNING: u8 = 1;
const STOPPED: u8 = 2;
const INDETERMINATE: u8 = 3;
const STARTING: u8 = 4;
const SETUP_ROOT: &str = "/run/elastos-browser";
const SETUP_STATE: &str = "/var/lib/elastos-browser";
const SETUP_HELPER: &str = "/usr/local/lib/elastos/browser-vm-linux-network.py";
const CONNECT_BUDGET: Duration = Duration::from_millis(200);
/// One vsock vring size for both vhost-user peers. crosvm's frontend offers
/// its 32768 maximum when `max-queue-size` is unset; vhost-device-vsock
/// refuses any ring above its `--queue-size`, drops the device, and crosvm
/// then stops the guest. 1024 is the backend's default and was proven on the
/// Orin (2048 fails the same way).
const VSOCK_QUEUE_SIZE: u16 = 1024;
/// Bytes of the crosvm log (which carries the guest serial console) and of
/// the vsock backend log kept after the VM exits on its own.
const RETAINED_CROSVM_LOG_BYTES: u64 = 64 * 1024;
const RETAINED_BACKEND_LOG_BYTES: u64 = 16 * 1024;
/// Kept in the Browser VM state root, outside the per-VM directory that
/// launch cleanup removes. Each unrequested exit replaces it.
const RETAINED_EXIT_LOG: &str = "last-vm-exit.log";
/// crosvm writes a launch overlay's header and tables in milliseconds.
const ROOTFS_OVERLAY_CREATE_BUDGET: Duration = Duration::from_secs(30);
const QCOW2_MAGIC: &[u8; 4] = b"QFI\xfb";

#[derive(Clone, Debug)]
enum Transport {
    Kernel,
    Userspace { binary: PathBuf, sandbox: PathBuf },
}

impl Transport {
    fn name(&self) -> &'static str {
        match self {
            Self::Kernel => "kernel-vsock",
            Self::Userspace { .. } => "userspace-vsock",
        }
    }
}

fn trusted_root_path(path: &Path) -> Result<(), String> {
    for entry in path.ancestors() {
        let metadata = fs::symlink_metadata(entry).map_err(|e| e.to_string())?;
        if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 || metadata.file_type().is_symlink()
        {
            return Err("Browser host setup requires an intact root-owned path".into());
        }
    }
    Ok(())
}

fn executable(path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.mode() & 0o111 == 0 {
        return Err("Browser host program must be an executable regular file".into());
    }
    Ok(())
}

/// Bytes an unprivileged writer can still use on the filesystem of `path`.
pub(super) fn available_bytes(path: &Path) -> Result<u64, String> {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: a NUL-terminated path and a statvfs the call fills on success.
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(format!("free space: {}", io::Error::last_os_error()));
    }
    // SAFETY: statvfs returned 0, so it initialized `stats`.
    let stats = unsafe { stats.assume_init() };
    Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
}

/// The crosvm this owner runs: the admitted one under the Home data root.
pub(super) fn crosvm_binary() -> PathBuf {
    super::env_path("ELASTOS_BROWSER_VM_CROSVM_BIN")
        .unwrap_or_else(|| super::browser_data_dir().join("bin/crosvm"))
}

/// Creates a launch's qcow2 overlay of `seed` with crosvm's own writer
/// (version 3, 16-bit refcounts, the absolute seed path as backing file and
/// `raw` as its format), the form crosvm boots. The overlay starts private
/// and empty; crosvm opens the seed read-only, now and when it boots.
pub(super) fn create_rootfs_overlay(
    crosvm: &Path,
    seed: &Path,
    overlay: &Path,
) -> Result<(), String> {
    // crosvm resolves a relative backing path against its working directory.
    let seed = fs::canonicalize(seed).map_err(|e| format!("rootfs seed: {e}"))?;
    if !fs::metadata(&seed).map_err(|e| e.to_string())?.is_file() {
        return Err("rootfs seed is not a regular file".into());
    }
    // create_qcow2 truncates the file it is given and keeps its mode.
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(overlay)
        .map_err(|e| format!("create {}: {e}", overlay.display()))?;
    let mut command = Command::new(crosvm);
    command
        .arg("create_qcow2")
        .arg("--backing-file")
        .arg(&seed)
        .arg(overlay)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    own_process_group(&mut command, Vec::new());
    let mut child = command.spawn_gated().map_err(|e| e.to_string())?;
    let deadline = Instant::now() + ROOTFS_OVERLAY_CREATE_BUDGET;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(5));
    };
    let mut detail = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut detail);
    }
    let detail = detail.lines().last().unwrap_or_default().to_string();
    match status {
        None => return Err("crosvm create_qcow2 timed out".into()),
        Some(status) if !status.success() => {
            return Err(format!("crosvm create_qcow2 failed ({status}): {detail}"))
        }
        Some(_) => {}
    }
    let mut file = File::open(overlay).map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    let mut header = [0_u8; 8];
    file.read_exact(&mut header)
        .map_err(|e| format!("read overlay header: {e}"))?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.mode() & 0o777 != 0o600
        || &header[..4] != QCOW2_MAGIC
        || header[4..] != 3_u32.to_be_bytes()
    {
        return Err("crosvm create_qcow2 left no private qcow2 v3 overlay".into());
    }
    Ok(())
}

fn trusted_program(path: &Path) -> Result<PathBuf, String> {
    // Distribution Python and bwrap may have a root-owned symlink at the leaf.
    if let Some(parent) = path.parent() {
        trusted_root_path(parent)?;
    }
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if metadata.uid() != 0 || (!metadata.file_type().is_symlink() && metadata.mode() & 0o022 != 0) {
        return Err("Browser host setup program requires root ownership".into());
    }
    let resolved = fs::canonicalize(path).map_err(|e| e.to_string())?;
    trusted_root_path(&resolved)?;
    executable(&resolved)?;
    Ok(resolved)
}

fn setup_json(path: &Path) -> Result<serde_json::Value, String> {
    trusted_root_path(path)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != 0
        || metadata.mode() & 0o022 != 0
    {
        return Err("Browser host setup receipt must be a root-owned regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("Browser host setup receipt is too large".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Browser host setup receipt is invalid".into())
}

fn setup_transport() -> Result<Transport, String> {
    let uid = unsafe { libc::getuid() };
    if uid == 0 {
        return Err("Run Browser as its ordinary Home user".into());
    }
    let receipt = setup_json(&Path::new(SETUP_ROOT).join(format!("{uid}.json")))?;
    let state = setup_json(&Path::new(SETUP_STATE).join(format!("{uid}.json")))?;
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").map_err(|e| e.to_string())?;
    transport_for_mode(Some(setup_mode(&receipt, &state, uid, boot.trim())?))
}

fn setup_mode<'a>(
    receipt: &'a serde_json::Value,
    state: &serde_json::Value,
    uid: u32,
    boot: &str,
) -> Result<&'a str, String> {
    let mode = receipt.get("transport").and_then(serde_json::Value::as_str);
    if receipt.get("uid").and_then(serde_json::Value::as_u64) != Some(uid as u64)
        || receipt.get("boot_id").and_then(serde_json::Value::as_str) != Some(boot)
        || mode != state.get("transport").and_then(serde_json::Value::as_str)
    {
        return Err("Browser host setup receipt requires repair for this user and boot".into());
    }
    match mode {
        Some("kernel-vsock" | "userspace-vsock") => Ok(mode.expect("checked setup mode")),
        _ => Err(
            "Repeat Browser host setup with an explicit kernel-vsock or userspace-vsock selection"
                .into(),
        ),
    }
}

fn transport_for_mode(mode: Option<&str>) -> Result<Transport, String> {
    match mode {
        Some("kernel-vsock") => Ok(Transport::Kernel),
        Some("userspace-vsock") => Ok(Transport::Userspace {
            binary: super::env_path("ELASTOS_BROWSER_VM_VHOST_DEVICE_VSOCK_BIN")
                .unwrap_or_else(|| super::browser_data_dir().join("bin/vhost-device-vsock")),
            sandbox: trusted_program(Path::new("/usr/bin/bwrap"))?,
        }),
        _ => Err("Browser host setup must select kernel-vsock or userspace-vsock".into()),
    }
}

fn setup_check() -> Result<(), String> {
    trusted_root_path(Path::new(SETUP_HELPER))?;
    let python = trusted_program(Path::new("/usr/bin/python3"))?;
    let mut command = Command::new(python);
    command
        .args(["-I", SETUP_HELPER, "check"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    run_setup_check(command, Duration::from_secs(5))
}

fn run_setup_check(mut command: Command, budget: Duration) -> Result<(), String> {
    own_process_group(&mut command, Vec::new());
    let mut child = command.spawn_gated().map_err(|e| e.to_string())?;
    let pgid = child.id() as i32;
    let deadline = Instant::now() + budget;
    let timed_out = loop {
        if observe_exit(pgid).unwrap_or(false) {
            break false;
        }
        if Instant::now() >= deadline {
            break true;
        }
        thread::sleep(Duration::from_millis(10));
    };
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
    // The caller awaits this operation in the shared startup owner. If kernel
    // retirement remains unresolved, that owner keeps this child and any held
    // slot lease instead of reporting absence from the readiness deadline.
    loop {
        if reap_owned_group_workers(Path::new("/proc"), pgid).unwrap_or(false)
            && observe_exit(pgid).unwrap_or(false)
        {
            if let Ok(status) = child.wait() {
                return if timed_out {
                    Err("Browser host setup check timed out".into())
                } else if status.success() {
                    Ok(())
                } else {
                    Err("Browser host setup check failed; repair the selected host setup".into())
                };
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn acquire_slot(transport: &Transport) -> Result<LockFile, String> {
    setup_check()?;
    let uid = unsafe { libc::getuid() };
    let root = Path::new(SETUP_ROOT);
    trusted_root_path(root)?;
    for slot in 0..4 {
        let path = root.join(format!("ebv{uid:x}s{slot}.lock"));
        let file = LockFile::new(
            OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)
                .map_err(|e| e.to_string())?,
        );
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != uid
            || metadata.mode() & 0o077 != 0
        {
            return Err("Browser host slot requires a private regular lease file".into());
        }
        if !super::try_lock_exclusive(&file)? {
            continue;
        }
        // Setup and removal acquire every slot. Recheck after acquiring this
        // one so a replaced setup cannot leave a lease on an old inode.
        setup_check()?;
        let current = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if current.dev() != metadata.dev()
            || current.ino() != metadata.ino()
            || !current.is_file()
            || current.nlink() != 1
            || current.uid() != uid
            || current.mode() & 0o077 != 0
            || setup_transport()?.name() != transport.name()
        {
            return Err("Browser host setup changed while acquiring its slot".into());
        }
        // A legacy TAP guest may outlive its old launcher. The canonical check
        // proves this TAP's identity and policy; observe its occupancy while we
        // hold the corresponding slot lease.
        if carrier_active(fs::read_to_string(format!(
            "/sys/class/net/ebv{uid:x}s{slot}/carrier"
        )))? {
            continue;
        }
        let fd = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 64) };
        if fd < 0 {
            return Err(io::Error::last_os_error().to_string());
        }
        return Ok(LockFile::new(unsafe { File::from_raw_fd(fd) }));
    }
    Err("All four Browser host slots are in use. Close a Browser session and retry.".into())
}

fn carrier_active(observation: io::Result<String>) -> Result<bool, String> {
    match observation {
        Ok(value) if value.trim() == "0" => Ok(false),
        Ok(value) if value.trim() == "1" => Ok(true),
        // Linux returns EINVAL for an unattached TAP. The setup check above
        // already requires this exact link to be UP.
        Err(error) if error.raw_os_error() == Some(libc::EINVAL) => Ok(false),
        _ => Err("Browser host slot carrier observation is unavailable".into()),
    }
}

pub(super) fn host_supported() -> bool {
    host_unavailable_reason().is_none()
}

#[derive(Clone, Copy)]
enum KvmStatus {
    Missing,
    Inaccessible,
    Unsupported,
    Ready,
}

fn kvm_status() -> KvmStatus {
    match fs::metadata("/dev/kvm") {
        Ok(metadata) if metadata.file_type().is_char_device() => {}
        Ok(_) => return KvmStatus::Unsupported,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return KvmStatus::Missing,
        Err(_) => return KvmStatus::Inaccessible,
    }
    let Ok(kvm) = OpenOptions::new().read(true).write(true).open("/dev/kvm") else {
        return KvmStatus::Inaccessible;
    };
    if unsafe {
        libc::ioctl(
            kvm.as_raw_fd(),
            0xAE00,
            std::ptr::null_mut::<libc::c_void>(),
        )
    } == 12
    {
        KvmStatus::Ready
    } else {
        KvmStatus::Unsupported
    }
}

fn capability_reason(kvm: KvmStatus, prepared: bool) -> Option<&'static str> {
    match kvm {
        KvmStatus::Missing | KvmStatus::Unsupported => Some("host_unsupported"),
        KvmStatus::Inaccessible => Some("preparation_required"),
        KvmStatus::Ready if !prepared => Some("preparation_required"),
        KvmStatus::Ready => None,
    }
}

pub(super) fn host_unavailable_reason() -> Option<&'static str> {
    capability_reason(
        kvm_status(),
        setup_transport().is_ok_and(|transport| transport_ready(&transport)),
    )
}

fn transport_ready(transport: &Transport) -> bool {
    match transport {
        Transport::Kernel => OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/vhost-vsock")
            .and_then(|file| file.metadata())
            .is_ok_and(|metadata| metadata.file_type().is_char_device()),
        Transport::Userspace { binary, sandbox } => {
            executable(binary).is_ok() && executable(sandbox).is_ok()
        }
    }
}

fn host_supports_transport(transport: &Transport) -> bool {
    capability_reason(kvm_status(), transport_ready(transport)).is_none()
}

struct Vm {
    config: VmConfig,
    cid: u32,
    transport: Transport,
    socket_dir: PathBuf,
    state: AtomicU8,
    stop: AtomicBool,
    writer_locks: Mutex<Vec<LockFile>>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
    /// Set before STOPPED when crosvm or its backend exited unrequested.
    exit: Mutex<Option<String>>,
}

pub(super) struct CrosvmBrowserProvider {
    config: VzConfig,
    binary: PathBuf,
    socket_dir: PathBuf,
    transport: Transport,
    vms: Mutex<BTreeMap<String, Arc<Vm>>>,
}

impl CrosvmBrowserProvider {
    pub(super) fn new(config: VzConfig, socket_dir: PathBuf) -> Result<Self, String> {
        let binary = crosvm_binary();
        let transport = setup_transport()?;
        if executable(&binary).is_err() || !host_supports_transport(&transport) {
            return Err(
                "Browser requires admitted host programs and access to its selected KVM transport"
                    .into(),
            );
        }
        Ok(Self {
            config,
            binary,
            socket_dir,
            transport,
            vms: Mutex::new(BTreeMap::new()),
        })
    }

    pub(super) async fn init(&self) -> Result<(), String> {
        fs::create_dir_all(&self.config.state_dir).map_err(|e| e.to_string())?;
        // Adopt this supervisor's orphaned workers so terminal proof can reap
        // the owned process group, rather than treating its first PID as the VM.
        if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
            return Err(io::Error::last_os_error().to_string());
        }
        Ok(())
    }

    pub(super) async fn load_with_vm_config(
        &self,
        config: VmConfig,
        manifest: CapsuleManifest,
    ) -> Result<CapsuleHandle, String> {
        let transport = self.transport.clone();
        let lease = tokio::task::spawn_blocking(move || acquire_slot(&transport))
            .await
            .map_err(|error| error.to_string())??;
        self.load_reserved_vm(config, manifest, lease)
    }

    fn load_reserved_vm(
        &self,
        config: VmConfig,
        manifest: CapsuleManifest,
        lease: LockFile,
    ) -> Result<CapsuleHandle, String> {
        if !config.network_disabled
            || config.network.is_some()
            || config.carrier_socket_path.is_some()
            || config.interactive_stdio
            || config.rootfs_readonly
        {
            return Err(
                "Browser crosvm uses fixed vsock channels and zero guest network devices".into(),
            );
        }
        let id = CapsuleId::new(format!("browser-crosvm-{}", uuid::Uuid::new_v4()));
        let bytes = *uuid::Uuid::new_v4().as_bytes();
        let cid = (u32::from_ne_bytes(bytes[..4].try_into().expect("four CID bytes"))
            & 0x7fff_ffff)
            .max(3);
        // Kernel registration refuses duplicate CIDs; userspace has one backend
        // per owned VM. Both use a nonreserved CID for the guest device.
        let vm = Arc::new(Vm {
            config,
            cid,
            transport: self.transport.clone(),
            socket_dir: self.socket_dir.clone(),
            state: AtomicU8::new(LOADED),
            stop: AtomicBool::new(false),
            writer_locks: Mutex::new(vec![lease]),
            worker: Mutex::new(None),
            exit: Mutex::new(None),
        });
        self.vms
            .lock()
            .map_err(|e| e.to_string())?
            .insert(id.0.clone(), vm);
        Ok(CapsuleHandle {
            id,
            manifest,
            args: vec![],
        })
    }

    fn vm(&self, handle: &CapsuleHandle) -> Result<Arc<Vm>, String> {
        self.vms
            .lock()
            .map_err(|e| e.to_string())?
            .get(&handle.id.0)
            .cloned()
            .ok_or_else(|| "Browser crosvm ownership record is absent".into())
    }

    pub(super) fn retain_writer_locks(
        &self,
        handle: &CapsuleHandle,
        locks: Vec<LockFile>,
    ) -> Result<(), String> {
        let vm = self.vm(handle)?;
        if vm.state.load(Ordering::Acquire) != LOADED {
            return Err("Browser writer locks must precede crosvm start".into());
        }
        let mut retained = Vec::new();
        for lock in locks {
            // Keep source descriptors outside the child handoff range. dup2
            // then cannot overwrite the next lock's source descriptor.
            let fd = unsafe { libc::fcntl(lock.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 64) };
            if fd < 0 {
                return Err(io::Error::last_os_error().to_string());
            }
            retained.push(LockFile::new(unsafe { File::from_raw_fd(fd) }));
        }
        vm.writer_locks
            .lock()
            .map_err(|e| e.to_string())?
            .extend(retained);
        Ok(())
    }

    pub(super) async fn start(&self, handle: &CapsuleHandle) -> Result<(), String> {
        let vm = self.vm(handle)?;
        let binary = self.binary.clone();
        let socket = self.socket_dir.join("v.sock");
        let state_dir = self.config.state_dir.join(&vm.config.vm_id);
        let (ready, started) = tokio::sync::oneshot::channel();
        let owned = Arc::clone(&vm);
        {
            let mut worker = vm.worker.lock().map_err(|e| e.to_string())?;
            if worker.is_some() || vm.state.load(Ordering::Acquire) != LOADED {
                return Err("Browser crosvm has already started".into());
            }
            vm.state.store(STARTING, Ordering::Release);
            let result = thread::Builder::new()
                .name("browser-crosvm-owner".into())
                .spawn(move || {
                    run_owned_vm(owned, binary, socket, state_dir, ready);
                });
            match result {
                Ok(thread) => *worker = Some(thread),
                Err(error) => {
                    vm.state.store(STOPPED, Ordering::Release);
                    return Err(error.to_string());
                }
            }
        }
        started.await.map_err(|e| e.to_string())?
    }

    pub(super) async fn stop(&self, handle: &CapsuleHandle) -> Result<(), String> {
        let vm = self.vm(handle)?;
        vm.stop.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(10);
        while matches!(vm.state.load(Ordering::Acquire), STARTING | RUNNING)
            && Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        if vm.state.load(Ordering::Acquire) == STOPPED {
            Ok(())
        } else {
            Err("Browser crosvm writer retirement remains unresolved".into())
        }
    }

    pub(super) async fn guest_state_label(&self, handle: &CapsuleHandle) -> String {
        match self.vm(handle).map(|vm| vm.state.load(Ordering::Acquire)) {
            Ok(LOADED) => "Stopped", // load has no process or writer effects.
            Ok(STARTING | RUNNING) => "Running",
            Ok(STOPPED) => "Stopped",
            _ => "Error",
        }
        .into()
    }

    pub(super) async fn wait_for_guest_exit(&self, handle: &CapsuleHandle) -> String {
        loop {
            let state = self.guest_state_label(handle).await;
            if state != "Running" {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub(super) async fn release_terminal_native_vm(&self, handle: &CapsuleHandle) -> bool {
        let Ok(vm) = self.vm(handle) else {
            return false;
        };
        let worker = match vm.worker.lock() {
            Ok(mut worker) => {
                // Start checks LOADED under this same mutex. Retire a loaded
                // VM here so a retained reference cannot start after release.
                if !matches!(vm.state.load(Ordering::Acquire), LOADED | STOPPED) {
                    return false;
                }
                vm.state.store(STOPPED, Ordering::Release);
                worker.take()
            }
            Err(_) => return false,
        };
        if let Some(worker) = worker {
            if !matches!(
                tokio::task::spawn_blocking(move || worker.join()).await,
                Ok(Ok(()))
            ) {
                return false;
            }
        }
        // Terminal worker settlement owns lease release. A stream operation
        // or state observer can retain Arc<Vm> after this call returns.
        match vm.writer_locks.lock() {
            Ok(mut locks) => locks.clear(),
            Err(_) => return false,
        }
        match self.vms.lock() {
            Ok(mut vms) => {
                vms.remove(&handle.id.0);
                true
            }
            Err(_) => false,
        }
    }

    pub(super) async fn vsock_connect(
        &self,
        handle: &CapsuleHandle,
        port: u32,
    ) -> Result<OwnedFd, String> {
        let vm = self.vm(handle)?;
        if vm.state.load(Ordering::Acquire) != RUNNING {
            return Err(
                vm_exit_detail(&vm).unwrap_or_else(|| "Browser crosvm is not running".into())
            );
        }
        let cid = vm.cid;
        tokio::task::spawn_blocking(move || match &vm.transport {
            Transport::Kernel => connect_vsock(cid, port),
            Transport::Userspace { .. } => connect_userspace(&vm.socket_dir.join("u/h.sock"), port),
        })
        .await
        .map_err(|e| e.to_string())?
    }

    /// Why crosvm or its backend exited on its own, with the retained log.
    pub(super) fn exit_detail(&self, handle: &CapsuleHandle) -> Option<String> {
        self.vm(handle).ok().and_then(|vm| vm_exit_detail(&vm))
    }
}

fn vm_exit_detail(vm: &Vm) -> Option<String> {
    vm.exit.lock().ok().and_then(|exit| exit.clone())
}

fn command(
    vm: &Vm,
    binary: PathBuf,
    socket: PathBuf,
    state_dir: PathBuf,
) -> Result<Command, String> {
    let config = &vm.config;
    let lock_count = vm.writer_locks.lock().map_err(|e| e.to_string())?.len();
    if !(2..=3).contains(&lock_count) {
        return Err("Browser crosvm requires its slot and profile leases before launch".into());
    }
    // The managed block devices retain these descriptors in their sandboxed
    // workers, including after the crosvm leader or this supervisor exits.
    let lifecycle_fds = (3..3 + lock_count)
        .map(|fd| fd.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let root = socket
        .parent()
        .ok_or("Browser crosvm state root is absent")?;
    let pivot = root.join("empty");
    fs::create_dir_all(&pivot).map_err(|e| e.to_string())?;
    fs::create_dir_all(&state_dir).map_err(|e| e.to_string())?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(state_dir.join("crosvm.log"))
        .map_err(|e| e.to_string())?;
    let mut command = Command::new(binary);
    command
        .args(["run", "--socket"])
        .arg(&socket)
        .arg("--mem")
        .arg(config.mem_size_mib.to_string())
        .arg("--cpus")
        .arg(config.vcpu_count.to_string())
        .args(["--serial", "type=stdout,hardware=serial,num=1"])
        .arg("--block")
        .arg(format!(
            "path={},root=true,lock=true,lifecycle-fds=[{lifecycle_fds}]",
            config.rootfs_path.display()
        ))
        .arg("--pivot-root")
        .arg(&pivot);
    match &vm.transport {
        Transport::Kernel => {
            command
                .arg("--vsock")
                .arg(format!("cid={},device=/dev/vhost-vsock", vm.cid));
        }
        Transport::Userspace { .. } => {
            command
                .arg("--vhost-user")
                .arg(format!(
                    "type=vsock,socket={},max-queue-size={VSOCK_QUEUE_SIZE}",
                    vm.socket_dir.join("u/v.sock").display()
                ))
                .args(["--vhost-user-connect-timeout-ms", "2000"]);
        }
    }
    if let Some(profile) = &config.data_disk_path {
        command.arg("--block").arg(format!(
            "path={},lock=true,lifecycle-fds=[{lifecycle_fds}]",
            profile.display()
        ));
    }
    if let Some(initrd) = &config.initramfs_path {
        command.arg("--initrd").arg(initrd);
    }
    command
        .arg("-p")
        .arg(&config.boot_args)
        .arg(&config.kernel_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(log));
    own_command(&mut command, vm)?;
    Ok(command)
}

fn own_command(command: &mut Command, vm: &Vm) -> Result<(), String> {
    let locks = vm
        .writer_locks
        .lock()
        .map_err(|e| e.to_string())?
        .iter()
        .map(AsRawFd::as_raw_fd)
        .collect::<Vec<_>>();
    own_process_group(command, locks);
    Ok(())
}

fn own_process_group(command: &mut Command, locks: Vec<i32>) {
    let parent = unsafe { libc::getpid() };
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() < 0
                || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0
            {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                return Err(io::Error::other("Browser supervisor exited during spawn"));
            }
            for (index, fd) in locks.iter().enumerate() {
                if libc::dup2(*fd, 3 + index as i32) < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
}

fn backend_command(vm: &Vm, state_dir: &Path) -> Result<Option<Command>, String> {
    let Transport::Userspace { binary, sandbox } = &vm.transport else {
        return Ok(None);
    };
    let sockets = vm.socket_dir.join("u");
    fs::create_dir(&sockets).map_err(|e| e.to_string())?;
    fs::set_permissions(&sockets, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    fs::create_dir_all(state_dir).map_err(|e| e.to_string())?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(state_dir.join("vsock-backend.log"))
        .map_err(|e| e.to_string())?;
    if vm
        .writer_locks
        .lock()
        .map_err(|e| e.to_string())?
        .is_empty()
    {
        return Err("Browser backend requires its slot lease before launch".into());
    }
    let mut command = Command::new(sandbox);
    command
        .env_clear()
        .args([
            "--unshare-all",
            "--die-with-parent",
            "--clearenv",
            "--cap-drop",
            "ALL",
        ])
        .args(["--ro-bind"])
        .arg(binary)
        .arg("/backend")
        .args(["--bind"])
        .arg(sockets)
        .arg("/s")
        .args(["--proc", "/proc", "--dev", "/dev", "--chdir", "/s"])
        // own_command puts the slot lease first, at FD 3. Bubblewrap's
        // sandbox init retains --sync-fd until every sandbox process exits;
        // the backend receives the remaining writer locks at FD 4 onward.
        // Its children keep the outer session/group created by own_command.
        .args(["--sync-fd", "3"])
        .args(["--", "/backend", "--guest-cid"])
        .arg(vm.cid.to_string())
        .args([
            "--socket",
            "/s/v.sock",
            "--uds-path",
            "/s/h.sock",
            "--queue-size",
        ])
        .arg(VSOCK_QUEUE_SIZE.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(log));
    own_command(&mut command, vm)?;
    Ok(Some(command))
}

fn run_owned_vm(
    vm: Arc<Vm>,
    binary: PathBuf,
    socket: PathBuf,
    state_dir: PathBuf,
    ready: tokio::sync::oneshot::Sender<Result<(), String>>,
) {
    let backend = backend_command(&vm, &state_dir).and_then(|command| {
        command
            .map(|mut command| command.spawn_gated().map_err(|e| e.to_string()))
            .transpose()
    });
    let mut backend = match backend {
        Ok(child) => child,
        Err(error) => {
            vm.state.store(STOPPED, Ordering::Release);
            let _ = ready.send(Err(error));
            return;
        }
    };
    let child = command(&vm, binary, socket, state_dir.clone())
        .and_then(|mut command| command.spawn_gated().map_err(|e| e.to_string()));
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            if let Some(backend) = backend.as_mut() {
                let pgid = backend.id() as i32;
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
                retire_owned_child(&vm, backend, Duration::from_secs(5), || {
                    reap_owned_group_workers(Path::new("/proc"), pgid)
                });
            }
            vm.state.store(STOPPED, Ordering::Release);
            let _ = ready.send(Err(error));
            return;
        }
    };
    let pgid = child.id() as i32;
    let backend_pgid = backend.as_ref().map(|child| child.id() as i32);
    vm.state.store(RUNNING, Ordering::Release);
    let _ = ready.send(Ok(()));
    let mut stop_at = None;
    let crosvm_exited_first = loop {
        // WNOWAIT keeps the exact leader PID reserved. Group signaling never
        // follows a reap that could expose this numeric PGID to reuse.
        match observe_exit(pgid) {
            Ok(true) => break true,
            Err(_) => {
                vm.state.store(INDETERMINATE, Ordering::Release);
                thread::sleep(Duration::from_millis(20));
                continue;
            }
            Ok(false) => {}
        }
        if let Some(backend_pgid) = backend_pgid {
            match observe_exit(backend_pgid) {
                Ok(true) => break false,
                Err(_) => vm.state.store(INDETERMINATE, Ordering::Release),
                Ok(false) => {}
            }
        }
        if vm.stop.load(Ordering::Acquire) {
            let start = *stop_at.get_or_insert_with(|| {
                unsafe {
                    libc::kill(-pgid, libc::SIGTERM);
                    if let Some(backend_pgid) = backend_pgid {
                        libc::kill(-backend_pgid, libc::SIGTERM);
                    }
                }
                Instant::now()
            });
            if start.elapsed() >= Duration::from_secs(2) {
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                    if let Some(backend_pgid) = backend_pgid {
                        libc::kill(-backend_pgid, libc::SIGKILL);
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(20));
    };
    let requested = vm.stop.load(Ordering::Acquire);
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
        if let Some(backend_pgid) = backend_pgid {
            libc::kill(-backend_pgid, libc::SIGKILL);
        }
    }
    retire_owned_child(&vm, &mut child, Duration::from_secs(5), || {
        reap_owned_group_workers(Path::new("/proc"), pgid)
    });
    if let (Some(backend), Some(backend_pgid)) = (backend.as_mut(), backend_pgid) {
        retire_owned_child(&vm, backend, Duration::from_secs(5), || {
            reap_owned_group_workers(Path::new("/proc"), backend_pgid)
        });
    }
    if !requested {
        // Both groups are reaped, so the logs are complete. std keeps the
        // reaped status, so try_wait reports the first exit exactly.
        let status = if crosvm_exited_first {
            child.try_wait().ok().flatten()
        } else {
            backend
                .as_mut()
                .and_then(|child| child.try_wait().ok().flatten())
        };
        let who = if crosvm_exited_first {
            "crosvm"
        } else {
            "the vsock backend"
        };
        let status = status
            .map(|status| status.to_string())
            .unwrap_or_else(|| "status unknown".into());
        let kept = match retain_exit_log(&state_dir, who, &status) {
            Ok(path) => format!(
                "its last log and guest console output are kept in {}",
                path.display()
            ),
            Err(error) => format!("its log could not be kept: {error}"),
        };
        if let Ok(mut exit) = vm.exit.lock() {
            *exit = Some(format!(
                "Browser VM stopped on its own: {who} exited ({status}); {kept}"
            ));
        }
    }
    vm.state.store(STOPPED, Ordering::Release);
}

/// Copies the tails of this VM's crosvm log (serial console included) and
/// backend log next to the per-VM directory, which launch cleanup removes.
fn retain_exit_log(state_dir: &Path, who: &str, status: &str) -> Result<PathBuf, String> {
    let root = state_dir
        .parent()
        .ok_or("Browser VM state root is absent")?;
    let mut bytes = format!("{who} exited on its own ({status})\n").into_bytes();
    for (name, limit) in [
        ("crosvm.log", RETAINED_CROSVM_LOG_BYTES),
        ("vsock-backend.log", RETAINED_BACKEND_LOG_BYTES),
    ] {
        match log_tail(&state_dir.join(name), limit) {
            Ok(Some(tail)) => {
                bytes.extend_from_slice(
                    format!("==> {name} (last {} bytes) <==\n", tail.len()).as_bytes(),
                );
                bytes.extend_from_slice(&tail);
                if !tail.ends_with(b"\n") {
                    bytes.push(b'\n');
                }
            }
            Ok(None) => {}
            Err(error) => bytes.extend_from_slice(format!("==> {name}: {error} <==\n").as_bytes()),
        }
    }
    let target = root.join(RETAINED_EXIT_LOG);
    let partial = root.join(format!(".{RETAINED_EXIT_LOG}.{}", std::process::id()));
    let _ = fs::remove_file(&partial);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&partial)
        .map_err(|e| e.to_string())?;
    let written = file
        .write_all(&bytes)
        .and_then(|_| file.sync_all())
        .and_then(|_| fs::rename(&partial, &target));
    if let Err(error) = written {
        let _ = fs::remove_file(&partial);
        return Err(error.to_string());
    }
    Ok(target)
}

fn log_tail(path: &Path, limit: u64) -> io::Result<Option<Vec<u8>>> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let length = file.metadata()?.len();
    io::Seek::seek(&mut file, io::SeekFrom::Start(length.saturating_sub(limit)))?;
    let mut tail = Vec::with_capacity(length.min(limit) as usize);
    file.take(limit).read_to_end(&mut tail)?;
    Ok(Some(tail))
}

fn retire_owned_child(
    vm: &Vm,
    child: &mut std::process::Child,
    observation_budget: Duration,
    mut reap_workers: impl FnMut() -> io::Result<bool>,
) {
    let deadline = Instant::now() + observation_budget;
    loop {
        // A timeout changes the observed state; it does not abandon this
        // reserved leader or the writer locks. Delayed workers can still settle.
        match reap_workers() {
            Ok(true)
                if observe_exit(child.id() as i32).unwrap_or(false) && child.wait().is_ok() =>
            {
                return;
            }
            Err(_) => vm.state.store(INDETERMINATE, Ordering::Release),
            _ if Instant::now() >= deadline => {
                vm.state.store(INDETERMINATE, Ordering::Release);
            }
            _ => {}
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn observe_exit(pid: i32) -> io::Result<bool> {
    let mut information: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as _,
            &mut information,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { information.si_pid() } == pid)
}

/// Proves no process but the unreaped leader remains in `pgid`. `proc_root`
/// only lists candidate PIDs: with hidepid=1 another user's entry stays listed
/// while its records refuse reads, so membership comes from getpgid, which
/// needs no access to those records.
fn reap_owned_group_workers(proc_root: &Path, pgid: i32) -> io::Result<bool> {
    let mut others = false;
    for entry in fs::read_dir(proc_root)? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<i32>().ok())
        else {
            continue;
        };
        if pid == pgid || process_group(pid)? != Some(pgid) {
            continue;
        }
        // Only this supervisor reaps its exited child, so that PID still names
        // the same group member when checked again; the still-unreaped leader
        // keeps the group ID from reuse.
        if observe_exit(pid).unwrap_or(false) && process_group(pid)? == Some(pgid) {
            let mut status = 0;
            if unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) } == pid {
                continue;
            }
        }
        others = true;
    }
    Ok(!others)
}

/// The process group of `pid`, or None once no such process exists.
fn process_group(pid: i32) -> io::Result<Option<i32>> {
    let group = unsafe { libc::getpgid(pid) };
    if group >= 0 {
        return Ok(Some(group));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(None)
    } else {
        Err(error)
    }
}

fn connect_vsock(cid: u32, port: u32) -> Result<OwnedFd, String> {
    let raw = unsafe {
        libc::socket(
            libc::AF_VSOCK,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let address = libc::sockaddr_vm {
        svm_family: libc::AF_VSOCK as _,
        svm_reserved1: 0,
        svm_port: port,
        svm_cid: cid,
        svm_zero: [0; 4],
    };
    let connected = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_vm).cast(),
            std::mem::size_of_val(&address) as _,
        )
    };
    if connected != 0 {
        if io::Error::last_os_error().raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(io::Error::last_os_error().to_string());
        }
        let mut poll = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        if unsafe { libc::poll(&mut poll, 1, 200) } <= 0 {
            return Err("Browser vsock connection attempt timed out".into());
        }
        let mut error: i32 = 0;
        let mut length = std::mem::size_of_val(&error) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut error as *mut i32).cast(),
                &mut length,
            )
        } != 0
        {
            return Err(io::Error::last_os_error().to_string());
        }
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error).to_string());
        }
    }
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error().to_string());
    }
    Ok(fd)
}

fn wait_socket(fd: i32, events: i16, deadline: Instant) -> Result<(), String> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("Browser userspace vsock connection attempt timed out".into());
        }
        let mut poll = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        let millis = remaining
            .as_millis()
            .saturating_add(1)
            .min(i32::MAX as u128) as i32;
        let result = unsafe { libc::poll(&mut poll, 1, millis) };
        if result > 0 {
            return Ok(());
        }
        if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return Err(io::Error::last_os_error().to_string());
        }
    }
}

fn connect_userspace(path: &Path, port: u32) -> Result<OwnedFd, String> {
    connect_userspace_with_budget(path, port, CONNECT_BUDGET)
}

fn connect_userspace_with_budget(
    path: &Path,
    port: u32,
    budget: Duration,
) -> Result<OwnedFd, String> {
    let deadline = Instant::now() + budget;
    let bytes = path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.is_empty() || bytes.len() >= address.sun_path.len() || bytes.contains(&0) || port == 0
    {
        return Err("Browser userspace vsock endpoint is invalid".into());
    }
    address.sun_family = libc::AF_UNIX as _;
    for (target, source) in address.sun_path.iter_mut().zip(bytes) {
        *target = *source as _;
    }
    let raw = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let result = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            std::mem::size_of_val(&address) as _,
        )
    };
    if result != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(error.to_string());
        }
        wait_socket(fd.as_raw_fd(), libc::POLLOUT, deadline)?;
        let mut error: i32 = 0;
        let mut length = std::mem::size_of_val(&error) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut error as *mut i32).cast(),
                &mut length,
            )
        } != 0
        {
            return Err(io::Error::last_os_error().to_string());
        }
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error).to_string());
        }
    }
    let mut stream = UnixStream::from(fd);
    let request = format!("CONNECT {port}\n");
    let mut pending = request.as_bytes();
    while !pending.is_empty() {
        wait_socket(stream.as_raw_fd(), libc::POLLOUT, deadline)?;
        match stream.write(pending) {
            Ok(0) => return Err("Browser userspace vsock request closed".into()),
            Ok(count) => pending = &pending[count..],
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    // One byte at a time keeps an immediately following guest payload in the
    // stream. An exact response also bounds the line to at most 14 bytes.
    for expected in format!("OK {port}\n").bytes() {
        let mut byte = [0];
        loop {
            wait_socket(stream.as_raw_fd(), libc::POLLIN, deadline)?;
            match stream.read(&mut byte) {
                Ok(1) if byte[0] == expected => break,
                Ok(_) => return Err("Browser userspace vsock receipt is invalid".into()),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    ) => {}
                Err(error) => return Err(error.to_string()),
            }
        }
    }
    stream.set_nonblocking(false).map_err(|e| e.to_string())?;
    Ok(stream.into())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    fn lock_objects() -> &'static Mutex<std::collections::BTreeMap<i32, String>> {
        static OBJECTS: std::sync::OnceLock<Mutex<std::collections::BTreeMap<i32, String>>> =
            std::sync::OnceLock::new();
        OBJECTS.get_or_init(Mutex::default)
    }

    pub(crate) fn record_lock_open(file: &File, caller: &std::panic::Location<'_>) {
        if std::env::var_os("ELASTOS_TEST_LOCK_TRACE").is_none() {
            return;
        }
        let fd = file.as_raw_fd();
        let owner = format!(
            "LockFile opened at {caller}\n{}",
            std::backtrace::Backtrace::force_capture()
        );
        eprintln!("LockFile open pid={} fd={fd} {owner}", std::process::id());
        lock_objects()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(fd, owner);
    }

    pub(crate) fn record_lock_drop(file: &File) {
        if std::env::var_os("ELASTOS_TEST_LOCK_TRACE").is_none() {
            return;
        }
        let fd = file.as_raw_fd();
        let owner = lock_objects()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&fd);
        eprintln!("LockFile drop pid={} fd={fd} {owner:?}", std::process::id());
    }

    pub(crate) fn slot_lock_holders(slot: &File) -> String {
        fn process_identity(pid: i32) -> String {
            let root = PathBuf::from(format!("/proc/{pid}"));
            let stat = fs::read_to_string(root.join("stat")).unwrap_or_default();
            let ppid = stat
                .rsplit_once(") ")
                .and_then(|(_, fields)| fields.split_whitespace().nth(1))
                .unwrap_or("unavailable");
            let cmdline = fs::read(root.join("cmdline"))
                .map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', " "))
                .unwrap_or_else(|error| format!("unavailable: {error}"));
            format!(
                "pid={pid} ppid={ppid} this_test_process={} cmdline={cmdline:?}",
                pid == std::process::id() as i32
            )
        }

        let metadata = slot.metadata().unwrap();
        let dev = metadata.dev();
        let inode = metadata.ino();
        let mut report = format!("slot.lock dev={dev} inode={inode}\n");
        let locks = fs::read_to_string("/proc/locks");
        for process in fs::read_dir("/proc").unwrap().flatten() {
            let Some(pid) = process
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<i32>().ok())
            else {
                continue;
            };
            let Ok(fds) = fs::read_dir(process.path().join("fd")) else {
                continue;
            };
            for fd in fds.flatten() {
                if fs::metadata(fd.path())
                    .is_ok_and(|metadata| metadata.dev() == dev && metadata.ino() == inode)
                {
                    report.push_str(&format!(
                        "slot holder fd={} {}\n",
                        fd.file_name().to_string_lossy(),
                        process_identity(pid)
                    ));
                    if pid == std::process::id() as i32 {
                        let objects = lock_objects()
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        let fd_number = fd.file_name().to_string_lossy().parse::<i32>().unwrap();
                        report.push_str(&format!(
                            "slot object fd={fd_number} owner={:?}\n",
                            objects.get(&fd_number)
                        ));
                    }
                    if let Ok(info) =
                        fs::read_to_string(process.path().join("fdinfo").join(fd.file_name()))
                    {
                        for line in info.lines().filter(|line| line.starts_with("lock:")) {
                            report.push_str(&format!(
                                "slot fdinfo pid={pid} fd={} {line}\n",
                                fd.file_name().to_string_lossy()
                            ));
                        }
                    }
                }
            }
        }
        let lock_identity = format!("{:02x}:{:02x}:{inode}", libc::major(dev), libc::minor(dev));
        match locks {
            Ok(locks) => {
                for line in locks.lines() {
                    let fields = line.split_whitespace().collect::<Vec<_>>();
                    if let Some(index) = fields.iter().position(|field| *field == lock_identity) {
                        let pid = fields[index - 1].parse::<i32>().unwrap_or(-1);
                        report.push_str(&format!(
                            "slot /proc/locks {line} {}\n",
                            process_identity(pid)
                        ));
                    }
                }
            }
            Err(error) => report.push_str(&format!("/proc/locks unavailable: {error}\n")),
        }
        report
    }

    fn fixture(root: &std::path::Path) -> (CrosvmBrowserProvider, VmConfig) {
        let config = VzConfig {
            state_dir: root.join("state"),
            ..VzConfig::default()
        };
        let provider = CrosvmBrowserProvider {
            config,
            binary: root.join("fake-crosvm"),
            socket_dir: root.join("sockets"),
            transport: Transport::Kernel,
            vms: Mutex::new(BTreeMap::new()),
        };
        let manifest = super::super::browser_vm_manifest(2048, 2);
        let mut vm = VmConfig::from_manifest(&manifest, root, &root.join("kernel"));
        vm.vm_id = "test-vm".into();
        vm.rootfs_path = root.join("root.ext4");
        vm.data_disk_path = Some(root.join("profile.ext4"));
        vm.initramfs_path = Some(root.join("initrd"));
        vm.boot_args =
            "console=ttyS0 root=/dev/vda elastos.browser_width=1920 elastos.browser_height=1080"
                .into();
        vm.network_disabled = true;
        (provider, vm)
    }

    fn load_fixture(provider: &CrosvmBrowserProvider, config: VmConfig) -> CapsuleHandle {
        fs::create_dir_all(&provider.socket_dir).unwrap();
        let lease =
            super::super::open_lifetime_lock(&provider.socket_dir.join("slot.lock"), true).unwrap();
        assert!(super::super::try_lock_exclusive(&lease).unwrap());
        let fd = unsafe { libc::fcntl(lease.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 64) };
        assert!(fd >= 64);
        let handle = provider
            .load_reserved_vm(config, super::super::browser_vm_manifest(2048, 2), unsafe {
                LockFile::new(File::from_raw_fd(fd))
            })
            .unwrap();
        let profile =
            super::super::open_lifetime_lock(&provider.socket_dir.join("profile.lock"), true)
                .unwrap();
        assert!(super::super::try_lock_exclusive(&profile).unwrap());
        provider
            .retain_writer_locks(&handle, vec![profile])
            .unwrap();
        handle
    }

    #[tokio::test]
    async fn crosvm_command_has_locked_disks_and_only_fixed_vsock() {
        let root = tempfile::tempdir().unwrap();
        let (provider, config) = fixture(root.path());
        let handle = load_fixture(&provider, config);
        let vm = provider.vm(&handle).unwrap();
        let command = command(
            &vm,
            provider.binary.clone(),
            root.path().join("sockets/v.sock"),
            root.path().join("state/test-vm"),
        )
        .unwrap();
        let args = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args
            .iter()
            .any(|a| a.ends_with("root.ext4,root=true,lock=true,lifecycle-fds=[3,4]")));
        assert!(args
            .iter()
            .any(|a| a.ends_with("profile.ext4,lock=true,lifecycle-fds=[3,4]")));
        assert!(args.contains(&"--vsock".into()));
        assert!(args.iter().any(|a| a.ends_with("device=/dev/vhost-vsock")));
        assert!(args
            .iter()
            .all(|a| !a.contains("tap") && !a.contains("--net") && !a.contains("netns")));
        assert!(provider.release_terminal_native_vm(&handle).await);
        assert!(!provider.release_terminal_native_vm(&handle).await);
    }

    #[tokio::test]
    async fn crosvm_command_binds_optional_rootfs_lease_and_refuses_incomplete_or_excess_leases() {
        for count in [0, 1, 3, 4] {
            let root = tempfile::tempdir().unwrap();
            let (provider, config) = fixture(root.path());
            let handle = load_fixture(&provider, config);
            let vm = provider.vm(&handle).unwrap();
            for index in 2..count {
                let lock = super::super::open_lifetime_lock(
                    &root.path().join(format!("extra-{index}.lock")),
                    true,
                )
                .unwrap();
                assert!(super::super::try_lock_exclusive(&lock).unwrap());
                provider.retain_writer_locks(&handle, vec![lock]).unwrap();
            }
            vm.writer_locks.lock().unwrap().truncate(count);
            let result = command(
                &vm,
                provider.binary.clone(),
                root.path().join("sockets/v.sock"),
                root.path().join("state/test-vm"),
            );
            if count == 3 {
                let command = result.unwrap();
                let args = command.get_args().collect::<Vec<_>>();
                let blocks = args
                    .windows(2)
                    .filter(|pair| pair[0] == "--block")
                    .map(|pair| pair[1].to_string_lossy())
                    .collect::<Vec<_>>();
                assert_eq!(blocks.len(), 2);
                assert!(blocks
                    .iter()
                    .all(|block| block.ends_with(",lifecycle-fds=[3,4,5]")));
            } else {
                assert!(result.unwrap_err().contains("slot and profile leases"));
            }
            assert!(provider.release_terminal_native_vm(&handle).await);
        }
    }

    #[tokio::test]
    async fn owned_crosvm_exit_reaps_worker_before_releasing_writer_lock() {
        let root = tempfile::tempdir().unwrap();
        let (provider, config) = fixture(root.path());
        fs::write(&provider.binary, "#!/bin/sh\nsleep 30 &\nprintf '%s' \"$!\" > \"$3.child\"\nprintf '%s' \"$$\" > \"$3.pid\"\nwait\n").unwrap();
        fs::set_permissions(&provider.binary, fs::Permissions::from_mode(0o700)).unwrap();
        provider.init().await.unwrap();
        let handle = load_fixture(&provider, config);
        let lock_path = root.path().join("writer.lock");
        let lock = super::super::open_lifetime_lock(&lock_path, true).unwrap();
        assert!(super::super::try_lock_exclusive(&lock).unwrap());
        provider.retain_writer_locks(&handle, vec![lock]).unwrap();
        provider.start(&handle).await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let pid_file = provider.socket_dir.join("v.sock.pid");
        while !pid_file.is_file() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let leader: i32 = fs::read_to_string(pid_file).unwrap().parse().unwrap();
        let worker: i32 = fs::read_to_string(provider.socket_dir.join("v.sock.child"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(!provider.release_terminal_native_vm(&handle).await);
        let contender = super::super::open_lifetime_lock(&lock_path, false).unwrap();
        assert!(!super::super::try_lock_exclusive(&contender).unwrap());
        // A different process group survives exact cleanup of this child.
        let mut unrelated_command = Command::new("/bin/sleep");
        unrelated_command.arg("30");
        unsafe {
            unrelated_command.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let mut unrelated = unrelated_command.spawn_gated().unwrap();
        unsafe {
            libc::kill(leader, libc::SIGKILL);
        }
        assert_eq!(provider.wait_for_guest_exit(&handle).await, "Stopped");
        assert!(provider.release_terminal_native_vm(&handle).await);
        assert_eq!(unsafe { libc::kill(worker, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        assert_eq!(unsafe { libc::kill(leader, 0) }, -1);
        assert!(unrelated.try_wait().unwrap().is_none());
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
        assert!(super::super::try_lock_exclusive(&contender).unwrap());
    }

    #[tokio::test]
    async fn vm_release_waits_for_sibling_exec_before_closing_cloexec_leases() {
        let root = tempfile::tempdir().unwrap();
        let (provider, config) = fixture(root.path());
        let handle = load_fixture(&provider, config);
        let lease_fd = provider.vm(&handle).unwrap().writer_locks.lock().unwrap()[0].as_raw_fd();
        let contender =
            super::super::open_lifetime_lock(&provider.socket_dir.join("slot.lock"), false)
                .unwrap();
        let (mut parent, child) = UnixStream::pair().unwrap();
        let parent_fd = parent.as_raw_fd();
        let child_fd = child.as_raw_fd();
        let spawn = thread::spawn(move || {
            let mut command = Command::new("/bin/cat");
            command.stdin(Stdio::piped()).stdout(Stdio::null());
            unsafe {
                command.pre_exec(move || {
                    // Only async-signal-safe calls between fork and exec.
                    // Close the parent's endpoint so a failed assertion in the
                    // parent also releases this child's blocking read.
                    libc::close(parent_fd);
                    let flags = libc::fcntl(lease_fd, libc::F_GETFD);
                    let inherited = (flags >= 0 && flags & libc::FD_CLOEXEC != 0) as u8;
                    if libc::write(child_fd, (&inherited as *const u8).cast(), 1) != 1 {
                        return Err(io::Error::last_os_error());
                    }
                    let mut proceed = 0u8;
                    if libc::read(child_fd, (&mut proceed as *mut u8).cast(), 1) != 1 {
                        return Err(io::Error::from_raw_os_error(libc::EPIPE));
                    }
                    Ok(())
                });
            }
            let spawned = command.spawn_gated();
            drop(child);
            spawned.unwrap()
        });
        let mut inherited = [0];
        parent.read_exact(&mut inherited).unwrap();
        // The child holds an inherited CLOEXEC lease until exec. Release on
        // another thread: it must wait at the gate rather than report success.
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (released_tx, released_rx) = std::sync::mpsc::channel();
        let release = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            started_tx.send(()).unwrap();
            let released = runtime.block_on(provider.release_terminal_native_vm(&handle));
            released_tx.send(released).unwrap();
        });
        started_rx.recv().unwrap();
        let before_exec = released_rx.recv_timeout(Duration::from_millis(50));
        let held_before_exec = !super::super::try_lock_exclusive(&contender).unwrap();
        // Always unblock the child before assertions so failures can settle.
        parent.write_all(&[1]).unwrap();
        let released = released_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        release.join().unwrap();
        let mut sibling = spawn.join().unwrap();
        let alive_after_exec = sibling.try_wait().unwrap().is_none();
        let free_after_exec = super::super::try_lock_exclusive(&contender).unwrap();
        // EOF settles cat after checking the lock while it is still alive.
        drop(sibling.stdin.take());
        assert!(sibling.wait().unwrap().success());
        assert_eq!(inherited, [1], "sibling inherited the CLOEXEC lease fd");
        assert!(released);
        assert!(
            matches!(before_exec, Err(std::sync::mpsc::RecvTimeoutError::Timeout)),
            "release must wait for the sibling to exec"
        );
        assert!(held_before_exec, "fork retained the same flock description");
        assert!(alive_after_exec);
        assert!(free_after_exec, "CLOEXEC closed the sibling's lease fd");
    }

    #[test]
    fn exit_observation_keeps_leader_identity_until_final_reap() {
        let mut child = Command::new("/bin/true").spawn_gated().unwrap();
        let pid = child.id() as i32;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !observe_exit(pid).unwrap() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(observe_exit(pid).unwrap());
        // A second WNOWAIT sees the same zombie; cleanup still owns this PID.
        assert!(observe_exit(pid).unwrap());
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
        child.wait().unwrap();
        assert!(observe_exit(pid).is_err());
    }

    #[test]
    fn group_cleanup_skips_unreadable_foreign_entries_and_reaps_owned_workers() {
        let proc_root = tempfile::tempdir().unwrap();
        // A worker can join only a group in its own session.
        let mut leader = Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn_gated()
            .unwrap();
        let pgid = leader.id() as i32;
        // Cleanup, not std, reaps this worker.
        let worker = Command::new("/bin/sleep")
            .arg("30")
            .process_group(pgid)
            .spawn_gated()
            .unwrap()
            .id() as i32;
        // With hidepid=1, another user's live (PID 1) or exited (i32::MAX)
        // process stays listed while its records refuse reads.
        for pid in [pgid, worker, 1, i32::MAX] {
            fs::create_dir(proc_root.path().join(pid.to_string())).unwrap();
        }
        for pid in [1, i32::MAX] {
            let record = proc_root.path().join(format!("{pid}/stat"));
            fs::write(&record, b"permission denied").unwrap();
            fs::set_permissions(&record, fs::Permissions::from_mode(0o000)).unwrap();
        }
        assert!(!reap_owned_group_workers(proc_root.path(), pgid).unwrap());
        unsafe {
            libc::kill(worker, libc::SIGKILL);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while !reap_owned_group_workers(proc_root.path(), pgid).unwrap() {
            assert!(
                Instant::now() < deadline,
                "the exited owned worker stays unproven"
            );
            thread::sleep(Duration::from_millis(10));
        }
        // The worker is reaped; the leader keeps its identity for the caller.
        assert_eq!(unsafe { libc::kill(worker, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        assert!(leader.try_wait().unwrap().is_none());
        leader.kill().unwrap();
        leader.wait().unwrap();
    }

    #[tokio::test]
    async fn terminal_release_frees_leases_before_last_vm_observer_drops() {
        let root = tempfile::tempdir().unwrap();
        let (provider, config) = fixture(root.path());
        let handle = load_fixture(&provider, config);
        let vm = provider.vm(&handle).unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let observer = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            finish_rx.recv().unwrap();
            let state = vm.state.load(Ordering::Acquire);
            drop(vm);
            state
        });
        ready_rx.recv().unwrap();
        let slot = super::super::open_lifetime_lock(&provider.socket_dir.join("slot.lock"), false)
            .unwrap();
        let profile =
            super::super::open_lifetime_lock(&provider.socket_dir.join("profile.lock"), false)
                .unwrap();
        let released = provider.release_terminal_native_vm(&handle).await;
        let slot_free = super::super::try_lock_exclusive(&slot).unwrap();
        let profile_free = super::super::try_lock_exclusive(&profile).unwrap();
        let holders = if slot_free && profile_free {
            String::new()
        } else {
            format!(
                "{}{}",
                slot_lock_holders(&slot),
                slot_lock_holders(&profile)
            )
        };
        // Settle the observer even when an assertion will fail.
        finish_tx.send(()).unwrap();
        let state = observer.join().unwrap();
        assert!(released);
        assert_eq!(state, STOPPED, "retired VM reference can still start");
        assert!(
            slot_free && profile_free,
            "release retained VM leases:\n{holders}"
        );
    }

    #[tokio::test]
    async fn delayed_retirement_keeps_owner_and_writer_lock_until_settlement() {
        let root = tempfile::tempdir().unwrap();
        let (provider, config) = fixture(root.path());
        let handle = load_fixture(&provider, config);
        let vm = provider.vm(&handle).unwrap();
        let lock_path = root.path().join("writer.lock");
        let lock = super::super::open_lifetime_lock(&lock_path, true).unwrap();
        assert!(super::super::try_lock_exclusive(&lock).unwrap());
        provider.retain_writer_locks(&handle, vec![lock]).unwrap();
        vm.state.store(RUNNING, Ordering::Release);
        let settle = Arc::new(AtomicBool::new(false));
        let ready = settle.clone();
        let owned = vm.clone();
        let mut child = Command::new("/bin/true").spawn_gated().unwrap();
        *vm.worker.lock().unwrap() = Some(thread::spawn(move || {
            retire_owned_child(&owned, &mut child, Duration::ZERO, || {
                if ready.load(Ordering::Acquire) {
                    Ok(true)
                } else {
                    Err(io::Error::other("temporary worker observation failure"))
                }
            });
            owned.state.store(STOPPED, Ordering::Release);
        }));
        let deadline = Instant::now() + Duration::from_secs(5);
        while vm.state.load(Ordering::Acquire) != INDETERMINATE {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!provider.release_terminal_native_vm(&handle).await);
        let contender = super::super::open_lifetime_lock(&lock_path, false).unwrap();
        assert!(!super::super::try_lock_exclusive(&contender).unwrap());
        let slot = super::super::open_lifetime_lock(&provider.socket_dir.join("slot.lock"), false)
            .unwrap();
        assert!(!super::super::try_lock_exclusive(&slot).unwrap());
        settle.store(true, Ordering::Release);
        while vm.state.load(Ordering::Acquire) != STOPPED {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(provider.release_terminal_native_vm(&handle).await);
        assert!(super::super::try_lock_exclusive(&contender).unwrap());
        assert!(super::super::try_lock_exclusive(&slot).unwrap());
    }

    #[test]
    fn setup_mode_requires_matching_explicit_user_boot_and_saved_transport() {
        for mode in ["kernel-vsock", "userspace-vsock"] {
            let receipt =
                serde_json::json!({"uid": 1000, "boot_id": "this-boot", "transport": mode});
            let state = serde_json::json!({"transport": mode});
            assert_eq!(
                setup_mode(&receipt, &state, 1000, "this-boot").unwrap(),
                mode
            );
            assert!(setup_mode(&receipt, &state, 1001, "this-boot").is_err());
            assert!(setup_mode(&receipt, &state, 1000, "old-boot").is_err());
            assert!(setup_mode(&receipt, &serde_json::json!({}), 1000, "this-boot").is_err());
        }
        for mode in [
            serde_json::Value::Null,
            serde_json::json!("automatic"),
            serde_json::json!("unknown"),
        ] {
            let receipt =
                serde_json::json!({"uid": 1000, "boot_id": "this-boot", "transport": mode});
            let state = serde_json::json!({"transport": mode});
            assert!(setup_mode(&receipt, &state, 1000, "this-boot").is_err());
        }
    }

    #[test]
    fn slot_occupancy_rejects_unreadable_or_malformed_carrier() {
        assert!(!carrier_active(Ok("0\n".into())).unwrap());
        assert!(carrier_active(Ok("1\n".into())).unwrap());
        assert!(!carrier_active(Err(io::Error::from_raw_os_error(libc::EINVAL))).unwrap());
        for value in ["", "2", "unavailable", "1 0"] {
            assert!(carrier_active(Ok(value.into())).is_err());
        }
        assert!(carrier_active(Err(io::Error::from_raw_os_error(libc::EACCES))).is_err());
        assert!(carrier_active(Err(io::Error::from_raw_os_error(libc::ENOENT))).is_err());
    }

    #[test]
    fn host_capability_separates_host_support_from_preparation() {
        for prepared in [false, true] {
            assert_eq!(
                capability_reason(KvmStatus::Missing, prepared),
                Some("host_unsupported")
            );
            assert_eq!(
                capability_reason(KvmStatus::Unsupported, prepared),
                Some("host_unsupported")
            );
            assert_eq!(
                capability_reason(KvmStatus::Inaccessible, prepared),
                Some("preparation_required")
            );
        }
        assert_eq!(
            capability_reason(KvmStatus::Ready, false),
            Some("preparation_required")
        );
        assert_eq!(capability_reason(KvmStatus::Ready, true), None);
    }

    #[tokio::test]
    async fn hanging_setup_check_reaps_its_exact_group_at_deadline() {
        let root = tempfile::tempdir().unwrap();
        let (provider, _) = fixture(root.path());
        provider.init().await.unwrap();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30 &\nprintf '%s' \"$!\" > \"$1/worker\"\nprintf '%s' \"$$\" > \"$1/leader\"\nwait", "fixture"])
            .arg(root.path()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let start = Instant::now();
        assert!(run_setup_check(command, Duration::from_millis(500))
            .unwrap_err()
            .contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(3));
        for name in ["leader", "worker"] {
            let pid: i32 = fs::read_to_string(root.path().join(name))
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        }
    }

    #[tokio::test]
    async fn userspace_command_confines_backend_and_keeps_crosvm_guest_network_empty() {
        let root = tempfile::tempdir().unwrap();
        let (mut provider, config) = fixture(root.path());
        provider.transport = Transport::Userspace {
            binary: root.path().join("backend"),
            sandbox: PathBuf::from("/usr/bin/bwrap"),
        };
        let handle = load_fixture(&provider, config);
        let vm = provider.vm(&handle).unwrap();
        let command = command(
            &vm,
            provider.binary.clone(),
            provider.socket_dir.join("v.sock"),
            root.path().join("state/test-vm"),
        )
        .unwrap();
        let args = command
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.contains(&"--vhost-user".into()));
        // Both vhost-user peers use one ring size; crosvm's unset default
        // (32768) makes vhost-device-vsock drop the device and stop the VM.
        assert_eq!(VSOCK_QUEUE_SIZE, 1024);
        assert!(args
            .iter()
            .any(|s| s.starts_with("type=vsock,socket=")
                && s.ends_with("/u/v.sock,max-queue-size=1024")));
        assert!(args.iter().all(|s| !s.contains("vhost-vsock")
            && s != "--vsock"
            && !s.contains("tap")
            && !s.contains("--net")));
        let backend = backend_command(&vm, &root.path().join("state/test-vm"))
            .unwrap()
            .unwrap();
        let args = backend
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        for flag in [
            "--unshare-all",
            "--die-with-parent",
            "--clearenv",
            "--cap-drop",
            "--sync-fd",
        ] {
            assert!(args.contains(&flag.into()), "missing {flag}");
        }
        assert!(args.windows(2).any(|p| p == ["--socket", "/s/v.sock"]));
        assert!(args.windows(2).any(|p| p == ["--uds-path", "/s/h.sock"]));
        assert!(args.windows(2).any(|p| p == ["--sync-fd", "3"]));
        assert!(args.windows(2).any(|p| p == ["--queue-size", "1024"]));
        assert!(args.iter().all(|s| !s.contains("auth_secret")
            && !s.contains("turn:")
            && !s.contains("--share-net")
            && s != "--preserve-fds"
            && s != "--new-session"));
        assert_eq!(args.iter().filter(|s| s.as_str() == "--bind").count(), 1);
        assert!(provider.release_terminal_native_vm(&handle).await);
    }

    #[tokio::test]
    async fn real_bwrap_retains_locks_and_one_group_until_sandbox_exit() {
        let sandbox = PathBuf::from("/usr/bin/bwrap");
        if !sandbox.exists() {
            eprintln!("real Bubblewrap lifecycle fixture requires /usr/bin/bwrap");
            return;
        }
        // Keep cleanup active through every assertion, including failures in
        // sandbox setup. An installed bwrap with unusable namespaces fails.
        struct ChildCleanup(Option<std::process::Child>);
        impl Drop for ChildCleanup {
            fn drop(&mut self) {
                let Some(child) = self.0.as_mut() else {
                    return;
                };
                let pgid = child.id() as i32;
                unsafe { libc::kill(-pgid, libc::SIGKILL) };
                let deadline = Instant::now() + Duration::from_secs(5);
                while Instant::now() < deadline {
                    if reap_owned_group_workers(Path::new("/proc"), pgid).unwrap_or(false)
                        && observe_exit(pgid).unwrap_or(false)
                    {
                        let _ = child.wait();
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
        let root = tempfile::tempdir().unwrap();
        let (mut provider, config) = fixture(root.path());
        let binary = root.path().join("backend");
        fs::write(&binary, "#!/bin/sh\n[ ! -e /proc/self/fd/3 ] || exit 10\n[ -e /proc/self/fd/4 ] || exit 11\nsleep 30 &\nprintf ready > /s/ready\nwait\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        provider.transport = Transport::Userspace { binary, sandbox };
        provider.init().await.unwrap();
        let handle = load_fixture(&provider, config);
        let disk_path = root.path().join("writer.lock");
        let disk = super::super::open_lifetime_lock(&disk_path, true).unwrap();
        assert!(super::super::try_lock_exclusive(&disk).unwrap());
        provider.retain_writer_locks(&handle, vec![disk]).unwrap();
        let vm = provider.vm(&handle).unwrap();
        let production = backend_command(&vm, &root.path().join("state/test-vm"))
            .unwrap()
            .unwrap();
        let args = production.get_args().collect::<Vec<_>>();
        let split = args.iter().position(|arg| *arg == "--").unwrap();
        let mut command = Command::new(production.get_program());
        command.env_clear().args(&args[..split]);
        // This fixture uses the host shell instead of a static backend. Only
        // its executable/library roots supplement the production arguments.
        for path in ["/usr", "/bin", "/lib", "/lib64"] {
            if Path::new(path).exists() {
                command.args(["--ro-bind", path, path]);
            }
        }
        command.args(&args[split..]).stdin(Stdio::null());
        own_command(&mut command, &vm).unwrap();
        let mut child = ChildCleanup(Some(command.spawn_gated().unwrap()));
        let pgid = child.0.as_ref().unwrap().id() as i32;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !provider.socket_dir.join("u/ready").exists() {
            assert!(
                Instant::now() < deadline,
                "Bubblewrap backend did not start"
            );
            assert!(
                !observe_exit(pgid).unwrap(),
                "Bubblewrap exited during setup"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Jetson kernels can omit task/PID/children. The unreaped leader
        // reserves this group, which is also the production cleanup boundary.
        let descendants = fs::read_dir("/proc")
            .unwrap()
            .filter_map(|entry| entry.unwrap().file_name().to_str()?.parse::<i32>().ok())
            .filter(|pid| process_group(*pid).unwrap() == Some(pgid))
            .collect::<Vec<_>>();
        assert!(
            descendants.len() >= 4,
            "monitor, init, backend and worker must exist"
        );
        for pid in &descendants {
            assert_eq!(process_group(*pid).unwrap(), Some(pgid));
        }
        // Simulate loss of every supervisor-held descriptor: only the real
        // sandbox init/backend can now keep these two flock leases alive.
        vm.writer_locks.lock().unwrap().clear();
        let slot = super::super::open_lifetime_lock(&provider.socket_dir.join("slot.lock"), false)
            .unwrap();
        let disk = super::super::open_lifetime_lock(&disk_path, false).unwrap();
        assert!(!super::super::try_lock_exclusive(&slot).unwrap());
        assert!(!super::super::try_lock_exclusive(&disk).unwrap());
        unsafe { libc::kill(pgid, libc::SIGKILL) };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !reap_owned_group_workers(Path::new("/proc"), pgid).unwrap()
            || !observe_exit(pgid).unwrap()
        {
            assert!(Instant::now() < deadline, "sandbox outlived its monitor");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        child.0.take().unwrap().wait().unwrap();
        for pid in descendants {
            assert_eq!(process_group(pid).unwrap(), None);
        }
        assert!(super::super::try_lock_exclusive(&slot).unwrap());
        assert!(super::super::try_lock_exclusive(&disk).unwrap());
        assert!(provider.release_terminal_native_vm(&handle).await);
    }

    fn uds_fixture(
        reply: Vec<u8>,
        byte_delay: Duration,
    ) -> (tempfile::TempDir, PathBuf, thread::JoinHandle<Vec<u8>>) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("h.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = vec![0; b"CONNECT 5001\n".len()];
            stream.read_exact(&mut request).unwrap();
            if byte_delay.is_zero() {
                let _ = stream.write_all(&reply);
            } else {
                for byte in reply {
                    thread::sleep(byte_delay);
                    if stream.write_all(&[byte]).is_err() {
                        break;
                    }
                }
            }
            request
        });
        (root, path, thread)
    }

    #[test]
    fn userspace_handshake_checks_port_and_preserves_guest_payload() {
        let (_root, path, server) = uds_fixture(b"OK 5001\nguest payload".to_vec(), Duration::ZERO);
        let fd = connect_userspace(&path, 5001).unwrap();
        let mut stream = File::from(fd);
        let mut payload = String::new();
        stream.read_to_string(&mut payload).unwrap();
        assert_eq!(payload, "guest payload");
        assert_eq!(server.join().unwrap(), b"CONNECT 5001\n");
        for response in [
            b"OK 5002\n".as_slice(),
            b"OK 5001".as_slice(),
            b"OK 5001111111111111111111111\n".as_slice(),
            b"OK -1\n".as_slice(),
        ] {
            let (_root, path, server) = uds_fixture(response.to_vec(), Duration::ZERO);
            assert!(connect_userspace(&path, 5001).is_err());
            server.join().unwrap();
        }
    }

    #[test]
    fn userspace_handshake_has_one_deadline_for_slow_reply() {
        let (_root, path, server) = uds_fixture(b"OK 5001\n".to_vec(), Duration::from_millis(40));
        let start = Instant::now();
        let error =
            connect_userspace_with_budget(&path, 5001, Duration::from_millis(100)).unwrap_err();
        assert!(error.contains("timed out"));
        assert!(start.elapsed() < Duration::from_millis(250));
        server.join().unwrap();
    }

    async fn wait_pid(path: &Path) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(text) = fs::read_to_string(path) {
                if let Ok(pid) = text.parse() {
                    return pid;
                }
            }
            assert!(
                Instant::now() < deadline,
                "owned child did not report its PID"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn either_userspace_child_exit_retires_both_groups_before_releasing_slot() {
        for exit_backend in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let (mut provider, config) = fixture(root.path());
            let sandbox = root.path().join("fake-sandbox");
            fs::write(&sandbox, "#!/bin/sh\nfor arg do\ncase \"$arg\" in --new-session|--preserve-fds) exit 90;; esac\ndone\ncase \" $* \" in *' --sync-fd 3 '*) ;; *) exit 91;; esac\nwhile [ \"$1\" != '--bind' ]; do shift; done\nprintf 'single-group sync-fd=3\\n' > \"$2/ownership\"\nsleep 30 &\nprintf '%s' \"$!\" > \"$2/worker.pid\"\nprintf '%s' \"$$\" > \"$2/owner.pid\"\nwait\n").unwrap();
            fs::set_permissions(&sandbox, fs::Permissions::from_mode(0o700)).unwrap();
            fs::write(&provider.binary, "#!/bin/sh\nsleep 30 &\nprintf '%s' \"$!\" > \"$3.child\"\nprintf '%s' \"$$\" > \"$3.pid\"\nwait\n").unwrap();
            fs::set_permissions(&provider.binary, fs::Permissions::from_mode(0o700)).unwrap();
            provider.transport = Transport::Userspace {
                binary: root.path().join("unused-backend"),
                sandbox,
            };
            provider.init().await.unwrap();
            let handle = load_fixture(&provider, config);
            provider.start(&handle).await.unwrap();
            let vm = wait_pid(&provider.socket_dir.join("v.sock.pid")).await;
            let vm_worker = wait_pid(&provider.socket_dir.join("v.sock.child")).await;
            let backend = wait_pid(&provider.socket_dir.join("u/owner.pid")).await;
            let backend_worker = wait_pid(&provider.socket_dir.join("u/worker.pid")).await;
            assert_eq!(
                fs::read_to_string(provider.socket_dir.join("u/ownership")).unwrap(),
                "single-group sync-fd=3\n"
            );
            assert_eq!(process_group(backend_worker).unwrap(), Some(backend));
            let slot =
                super::super::open_lifetime_lock(&provider.socket_dir.join("slot.lock"), false)
                    .unwrap();
            assert!(!super::super::try_lock_exclusive(&slot).unwrap());
            unsafe {
                libc::kill(if exit_backend { backend } else { vm }, libc::SIGKILL);
            }
            assert_eq!(
                tokio::time::timeout(
                    Duration::from_secs(5),
                    provider.wait_for_guest_exit(&handle)
                )
                .await
                .unwrap(),
                "Stopped"
            );
            assert!(provider.release_terminal_native_vm(&handle).await);
            for pid in [vm, vm_worker, backend, backend_worker] {
                assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
                assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
            }
            assert!(super::super::try_lock_exclusive(&slot).unwrap());
        }
    }

    #[tokio::test]
    async fn failed_crosvm_spawn_settles_backend_before_releasing_slot() {
        let root = tempfile::tempdir().unwrap();
        let (mut provider, config) = fixture(root.path());
        provider.transport = Transport::Userspace {
            binary: PathBuf::from("/unused"),
            sandbox: PathBuf::from("/bin/sleep"),
        };
        provider.init().await.unwrap();
        let handle = load_fixture(&provider, config);
        let slot = super::super::open_lifetime_lock(&provider.socket_dir.join("slot.lock"), false)
            .unwrap();
        assert!(provider.start(&handle).await.is_err());
        assert_eq!(provider.guest_state_label(&handle).await, "Stopped");
        assert!(!super::super::try_lock_exclusive(&slot).unwrap());
        assert!(provider.release_terminal_native_vm(&handle).await);
        assert!(
            super::super::try_lock_exclusive(&slot).unwrap(),
            "{}",
            slot_lock_holders(&slot)
        );
    }

    #[tokio::test]
    async fn early_crosvm_exit_ends_vsock_wait_at_once_and_keeps_its_log_tail() {
        let root = tempfile::tempdir().unwrap();
        let (provider, config) = fixture(root.path());
        // More than the retained 64 KiB: the first line must be cut, the
        // guest console's last lines kept.
        fs::write(
            &provider.binary,
            "#!/bin/sh\nprintf 'FIRST-LINE\\n'\nhead -c 70000 /dev/zero | tr '\\0' x\nprintf '\\n[    1.9] browser-vm-init: last console line\\n'\nprintf 'vu-vsock: Backend device disconnected early\\n' >&2\nexit 0\n",
        )
        .unwrap();
        fs::set_permissions(&provider.binary, fs::Permissions::from_mode(0o700)).unwrap();
        provider.init().await.unwrap();
        let handle = load_fixture(&provider, config);
        let provider = Arc::new(provider);
        provider.start(&handle).await.unwrap();
        let started = Instant::now();
        let error = super::super::connect_vsock_with_retry(
            Arc::clone(&provider),
            &handle,
            5000,
            Duration::from_secs(30),
        )
        .await
        .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "waited {:?}",
            started.elapsed()
        );
        let kept = root.path().join("state").join(RETAINED_EXIT_LOG);
        assert!(error.contains("guest stopped (Stopped) before it answered on vsock port 5000"));
        assert!(error.contains("crosvm exited (exit status: 0)"), "{error}");
        assert!(error.contains(&kept.display().to_string()), "{error}");
        let metadata = fs::metadata(&kept).unwrap();
        assert_eq!(metadata.mode() & 0o777, 0o600);
        let text = fs::read_to_string(&kept).unwrap();
        assert!(text.starts_with("crosvm exited on its own (exit status: 0)\n"));
        assert!(text.contains("browser-vm-init: last console line"));
        assert!(text.contains("Backend device disconnected early"));
        assert!(!text.contains("FIRST-LINE"));
        assert!(metadata.len() <= RETAINED_CROSVM_LOG_BYTES + 256);
        // Launch cleanup removes the per-VM directory; the tail stays.
        assert!(provider.release_terminal_native_vm(&handle).await);
        fs::remove_dir_all(root.path().join("state/test-vm")).unwrap();
        assert!(kept.is_file());
    }

    #[tokio::test]
    async fn requested_stop_keeps_no_exit_log() {
        let root = tempfile::tempdir().unwrap();
        let (provider, config) = fixture(root.path());
        fs::write(&provider.binary, "#!/bin/sh\nexec sleep 30\n").unwrap();
        fs::set_permissions(&provider.binary, fs::Permissions::from_mode(0o700)).unwrap();
        provider.init().await.unwrap();
        let handle = load_fixture(&provider, config);
        provider.start(&handle).await.unwrap();
        provider.stop(&handle).await.unwrap();
        assert!(provider.exit_detail(&handle).is_none());
        assert!(!root.path().join("state").join(RETAINED_EXIT_LOG).exists());
        assert!(provider.release_terminal_native_vm(&handle).await);
    }
}
