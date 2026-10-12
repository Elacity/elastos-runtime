//! Linux substrate for the shared Browser transport/lifecycle supervisor.
//! Only this module launches crosvm. The parent owns bootstrap, streams, TURN
//! policy, profile durability and the exact terminal settlement.
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
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

pub(super) fn host_supported() -> bool {
    let Ok(kvm) = OpenOptions::new().read(true).write(true).open("/dev/kvm") else {
        return false;
    };
    let Ok(vsock) = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/vhost-vsock")
    else {
        return false;
    };
    [kvm.metadata(), vsock.metadata()]
        .into_iter()
        .all(|m| m.is_ok_and(|m| m.file_type().is_char_device()))
        && unsafe {
            libc::ioctl(
                kvm.as_raw_fd(),
                0xAE00,
                std::ptr::null_mut::<libc::c_void>(),
            )
        } == 12
}

struct Vm {
    config: VmConfig,
    cid: u32,
    state: AtomicU8,
    stop: AtomicBool,
    writer_locks: Mutex<Vec<File>>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

pub(super) struct CrosvmBrowserProvider {
    config: VzConfig,
    binary: PathBuf,
    socket_dir: PathBuf,
    vms: Mutex<BTreeMap<String, Arc<Vm>>>,
}

impl CrosvmBrowserProvider {
    pub(super) fn new(config: VzConfig, socket_dir: PathBuf) -> Result<Self, String> {
        let data = super::default_data_dir();
        let binary = super::env_path("ELASTOS_BROWSER_VM_CROSVM_BIN")
            .unwrap_or_else(|| data.join("bin/crosvm"));
        if !binary.is_file() || !host_supported() {
            return Err(
                "Browser requires the admitted crosvm and user access to KVM and vhost-vsock"
                    .into(),
            );
        }
        Ok(Self {
            config,
            binary,
            socket_dir,
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
        // The kernel's vhost-vsock registration refuses a duplicate live CID.
        // A random nonreserved CID avoids a shared fixed-CID launch path.
        let vm = Arc::new(Vm {
            config,
            cid,
            state: AtomicU8::new(LOADED),
            stop: AtomicBool::new(false),
            writer_locks: Mutex::new(Vec::new()),
            worker: Mutex::new(None),
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
        locks: Vec<File>,
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
            retained.push(unsafe { File::from_raw_fd(fd) });
        }
        *vm.writer_locks.lock().map_err(|e| e.to_string())? = retained;
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
        if !matches!(vm.state.load(Ordering::Acquire), LOADED | STOPPED) {
            return false;
        }
        let worker = match vm.worker.lock() {
            Ok(mut worker) => worker.take(),
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
            return Err("Browser crosvm is not running".into());
        }
        let cid = vm.cid;
        tokio::task::spawn_blocking(move || connect_vsock(cid, port))
            .await
            .map_err(|e| e.to_string())?
    }
}

fn command(
    vm: &Vm,
    binary: PathBuf,
    socket: PathBuf,
    state_dir: PathBuf,
) -> Result<Command, String> {
    let config = &vm.config;
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
            "path={},root=true,lock=true",
            config.rootfs_path.display()
        ))
        .arg("--vsock")
        .arg(format!("cid={},device=/dev/vhost-vsock", vm.cid))
        .arg("--pivot-root")
        .arg(&pivot);
    if let Some(profile) = &config.data_disk_path {
        command
            .arg("--block")
            .arg(format!("path={},lock=true", profile.display()));
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
    let parent = unsafe { libc::getpid() };
    let locks = vm
        .writer_locks
        .lock()
        .map_err(|e| e.to_string())?
        .iter()
        .map(AsRawFd::as_raw_fd)
        .collect::<Vec<_>>();
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
    Ok(command)
}

fn run_owned_vm(
    vm: Arc<Vm>,
    binary: PathBuf,
    socket: PathBuf,
    state_dir: PathBuf,
    ready: tokio::sync::oneshot::Sender<Result<(), String>>,
) {
    let child = command(&vm, binary, socket, state_dir)
        .and_then(|mut command| command.spawn().map_err(|e| e.to_string()));
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            vm.state.store(STOPPED, Ordering::Release);
            let _ = ready.send(Err(error));
            return;
        }
    };
    let pgid = child.id() as i32;
    vm.state.store(RUNNING, Ordering::Release);
    let _ = ready.send(Ok(()));
    let mut stop_at = None;
    loop {
        // WNOWAIT keeps the exact leader PID reserved. Group signaling never
        // follows a reap that could expose this numeric PGID to reuse.
        match observe_exit(pgid) {
            Ok(true) => break,
            Err(_) => {
                vm.state.store(INDETERMINATE, Ordering::Release);
                thread::sleep(Duration::from_millis(20));
                continue;
            }
            Ok(false) => {}
        }
        if vm.stop.load(Ordering::Acquire) {
            let start = *stop_at.get_or_insert_with(|| {
                unsafe {
                    libc::kill(-pgid, libc::SIGTERM);
                }
                Instant::now()
            });
            if start.elapsed() >= Duration::from_secs(2) {
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
    retire_owned_child(&vm, &mut child, Duration::from_secs(5), || {
        reap_owned_group_workers(pgid)
    });
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
            Ok(true) if child.wait().is_ok() => {
                vm.state.store(STOPPED, Ordering::Release);
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

fn reap_owned_group_workers(pgid: i32) -> io::Result<bool> {
    let parent = unsafe { libc::getpid() };
    let mut others = false;
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<i32>().ok())
        else {
            continue;
        };
        if pid == pgid {
            continue;
        }
        let stat = match fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let (_, fields) = stat
            .rsplit_once(')')
            .ok_or_else(|| io::Error::other("Invalid process stat"))?;
        let fields = fields.split_whitespace().collect::<Vec<_>>();
        if fields.get(2).and_then(|n| n.parse::<i32>().ok()) != Some(pgid) {
            continue;
        }
        // An adopted child cannot reuse its PID before its own parent reaps it.
        // Other groups, and the still-unreaped leader, retain their identities.
        if fields.get(1).and_then(|n| n.parse::<i32>().ok()) == Some(parent) {
            let mut status = 0;
            if unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) } == pid {
                continue;
            }
        }
        others = true;
    }
    Ok(!others)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fixture(root: &std::path::Path) -> (CrosvmBrowserProvider, VmConfig) {
        let config = VzConfig {
            state_dir: root.join("state"),
            ..VzConfig::default()
        };
        let provider = CrosvmBrowserProvider {
            config,
            binary: root.join("fake-crosvm"),
            socket_dir: root.join("sockets"),
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

    #[tokio::test]
    async fn crosvm_command_has_locked_disks_and_only_fixed_vsock() {
        let root = tempfile::tempdir().unwrap();
        let (provider, config) = fixture(root.path());
        let manifest = super::super::browser_vm_manifest(2048, 2);
        let handle = provider
            .load_with_vm_config(config, manifest)
            .await
            .unwrap();
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
            .any(|a| a.ends_with("root.ext4,root=true,lock=true")));
        assert!(args.iter().any(|a| a.ends_with("profile.ext4,lock=true")));
        assert!(args.contains(&"--vsock".into()));
        assert!(args.iter().any(|a| a.ends_with("device=/dev/vhost-vsock")));
        assert!(args
            .iter()
            .all(|a| !a.contains("tap") && !a.contains("--net") && !a.contains("netns")));
        assert!(provider.release_terminal_native_vm(&handle).await);
        assert!(!provider.release_terminal_native_vm(&handle).await);
    }

    #[tokio::test]
    async fn owned_crosvm_exit_reaps_worker_before_releasing_writer_lock() {
        let root = tempfile::tempdir().unwrap();
        let (provider, config) = fixture(root.path());
        fs::write(&provider.binary, "#!/bin/sh\nsleep 30 &\nprintf '%s' \"$!\" > \"$3.child\"\nprintf '%s' \"$$\" > \"$3.pid\"\nwait\n").unwrap();
        fs::set_permissions(&provider.binary, fs::Permissions::from_mode(0o700)).unwrap();
        provider.init().await.unwrap();
        let handle = provider
            .load_with_vm_config(config, super::super::browser_vm_manifest(2048, 2))
            .await
            .unwrap();
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
        let mut unrelated = unrelated_command.spawn().unwrap();
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

    #[test]
    fn exit_observation_keeps_leader_identity_until_final_reap() {
        let mut child = Command::new("/bin/true").spawn().unwrap();
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

    #[tokio::test]
    async fn delayed_retirement_keeps_owner_and_writer_lock_until_settlement() {
        let root = tempfile::tempdir().unwrap();
        let (provider, config) = fixture(root.path());
        let handle = provider
            .load_with_vm_config(config, super::super::browser_vm_manifest(2048, 2))
            .await
            .unwrap();
        let vm = provider.vm(&handle).unwrap();
        let lock_path = root.path().join("writer.lock");
        let lock = super::super::open_lifetime_lock(&lock_path, true).unwrap();
        assert!(super::super::try_lock_exclusive(&lock).unwrap());
        provider.retain_writer_locks(&handle, vec![lock]).unwrap();
        vm.state.store(RUNNING, Ordering::Release);
        let settle = Arc::new(AtomicBool::new(false));
        let ready = settle.clone();
        let owned = vm.clone();
        let mut child = Command::new("/bin/true").spawn().unwrap();
        *vm.worker.lock().unwrap() = Some(thread::spawn(move || {
            retire_owned_child(&owned, &mut child, Duration::ZERO, || {
                if ready.load(Ordering::Acquire) {
                    Ok(true)
                } else {
                    Err(io::Error::other("temporary worker observation failure"))
                }
            });
        }));
        let deadline = Instant::now() + Duration::from_secs(5);
        while vm.state.load(Ordering::Acquire) != INDETERMINATE {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!provider.release_terminal_native_vm(&handle).await);
        let contender = super::super::open_lifetime_lock(&lock_path, false).unwrap();
        assert!(!super::super::try_lock_exclusive(&contender).unwrap());
        settle.store(true, Ordering::Release);
        while vm.state.load(Ordering::Acquire) != STOPPED {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(provider.release_terminal_native_vm(&handle).await);
        drop(vm);
        assert!(super::super::try_lock_exclusive(&contender).unwrap());
    }
}
