use std::fs::{self, OpenOptions};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context as _};
use serde::{Deserialize, Serialize};

/// An exclusive flock held for this guard's scope.
#[derive(Debug)]
pub struct FileLock {
    file: Option<fs::File>,
    /// Set while this guard is the host lock of a data directory.
    held_host: Option<u64>,
}

impl FileLock {
    fn held(file: fs::File) -> Self {
        Self {
            file: Some(file),
            held_host: None,
        }
    }

    /// Waits until this open file holds the exclusive lock.
    pub(crate) fn exclusive(file: fs::File) -> std::io::Result<Self> {
        file.lock()?;
        Ok(Self::held(file))
    }

    /// Takes the exclusive lock now or fails with `WouldBlock`.
    pub(crate) fn try_exclusive(file: fs::File) -> std::io::Result<Self> {
        Self::exclusive_within(file, Duration::ZERO)
    }

    /// Retries a busy lock until `wait` passes, then fails with `WouldBlock`.
    pub(crate) fn exclusive_within(file: fs::File, wait: Duration) -> std::io::Result<Self> {
        let deadline = Instant::now() + wait;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self::held(file)),
                Err(fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    std::thread::sleep(remaining.min(Duration::from_millis(5)));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// Unlocks now and reports the result; dropping instead discards it.
    pub(crate) fn release(mut self) -> std::io::Result<()> {
        forget_held_host_lock(self.held_host.take());
        self.file.take().map_or(Ok(()), |file| file.unlock())
    }
}

impl std::ops::Deref for FileLock {
    type Target = fs::File;

    fn deref(&self) -> &fs::File {
        self.file.as_ref().expect("only release takes the file")
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // This process stops claiming the host lock before it unlocks.
        forget_held_host_lock(self.held_host.take());
        // A command spawned meanwhile by another thread can retain this
        // description until its CLOEXEC fd closes at exec. The guard's scope
        // owns the lock, so release it before closing our fd.
        if let Some(file) = &self.file {
            let _ = file.unlock();
        }
    }
}

/// Host locks that this process holds now. `docs/LAUNCHER_CONTRACT.md`
/// ("Readiness and Home URL") states what proves ownership. The JSON record
/// in the lock file is not proof: any process of the same user can rewrite it.
static HELD_HOST_LOCKS: std::sync::Mutex<Vec<HeldHostLock>> = std::sync::Mutex::new(Vec::new());
static NEXT_HELD_HOST_LOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[derive(Debug)]
struct HeldHostLock {
    id: u64,
    file: LockedFileId,
    role: String,
}

/// The file a lock is on. While this process keeps the file open, no other
/// file can have the same device and inode.
type LockedFileId = (u64, u64);

#[cfg(unix)]
fn locked_file_id(metadata: &fs::Metadata) -> Option<LockedFileId> {
    use std::os::unix::fs::MetadataExt as _;
    Some((metadata.dev(), metadata.ino()))
}

/// Without a device and inode, this process cannot prove the path names the
/// file it locked, so it claims no host lock.
#[cfg(not(unix))]
fn locked_file_id(_metadata: &fs::Metadata) -> Option<LockedFileId> {
    None
}

fn held_host_locks() -> std::sync::MutexGuard<'static, Vec<HeldHostLock>> {
    HELD_HOST_LOCKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn remember_held_host_lock(lock: &mut FileLock, role: &str) -> std::io::Result<()> {
    let Some(file) = locked_file_id(&lock.metadata()?) else {
        return Ok(());
    };
    let id = NEXT_HELD_HOST_LOCK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    held_host_locks().push(HeldHostLock {
        id,
        file,
        role: role.to_string(),
    });
    lock.held_host = Some(id);
    Ok(())
}

fn forget_held_host_lock(id: Option<u64>) {
    if let Some(id) = id {
        held_host_locks().retain(|held| held.id != id);
    }
}

/// The role under which this process holds the host lock of `data_dir`.
///
/// The proof is the flock itself: this process holds an exclusive lock on an
/// open file, `{data}/host-process.lock` names that same file now, and a new
/// open of it cannot take the lock. The JSON record is not read.
pub fn held_host_role(data_dir: &Path) -> Option<String> {
    let probe = fs::File::open(host_lock_path(data_dir)).ok()?;
    let file = locked_file_id(&probe.metadata().ok()?)?;
    let role = held_host_locks()
        .iter()
        .find(|held| held.file == file)
        .map(|held| held.role.clone())?;
    match probe.try_lock() {
        Err(fs::TryLockError::WouldBlock) => Some(role),
        Ok(()) => {
            // The lock was free, so this process does not hold it.
            let _ = probe.unlock();
            None
        }
        Err(fs::TryLockError::Error(_)) => None,
    }
}

/// Whether this process holds the host lock of `data_dir` as a gateway.
pub fn holds_gateway_host_lock(data_dir: &Path) -> bool {
    matches!(
        held_host_role(data_dir).as_deref(),
        Some("gateway" | "gateway-public")
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostProcessInfo {
    pub pid: u32,
    pub role: String,
    pub addr: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct HostProcessMeta {
    pid: u32,
    role: String,
    addr: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    generation: String,
}

/// The gateway server takes the host lock as `gateway` unless this process
/// already holds it. `gateway` and `gateway-public` take it before the server
/// starts. `serve` also holds it when its supervisor starts a gateway in the
/// same process. That gateway keeps the `serve` lock, so its `/healthz` answers 503.
/// A record that only names this pid is not a lock this process holds.
pub fn publish_gateway_lock_if_unowned(
    data_dir: &Path,
    addr: &str,
) -> anyhow::Result<Option<FileLock>> {
    if held_host_role(data_dir).is_some() {
        return Ok(None);
    }
    acquire_host_process_lock(data_dir, "gateway", addr).map(Some)
}

pub fn acquire_host_process_lock(
    data_dir: &Path,
    role: &str,
    addr: &str,
) -> anyhow::Result<FileLock> {
    if !matches!(role, "update" | "update-recovery") {
        authorize_host_process_start(data_dir)?;
    }
    acquire_host_process_lock_inner(data_dir, role, addr)
}

pub(crate) fn acquire_principal_root_update_lock(
    data_dir: &Path,
    activation: &crate::install_transaction::SupportActivation<'_>,
) -> anyhow::Result<FileLock> {
    activation.authorize_principal_root_migration(data_dir)?;
    acquire_host_process_lock_inner(data_dir, "principal-root-upgrade", "offline")
}

fn acquire_host_process_lock_inner(
    data_dir: &Path,
    role: &str,
    addr: &str,
) -> anyhow::Result<FileLock> {
    fs::create_dir_all(data_dir)?;
    let lock_path = host_lock_path(data_dir);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("open host lock {}", lock_path.display()))?;

    let mut lock = match FileLock::try_exclusive(file) {
        Ok(lock) => lock,
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
            let holder = read_lock_metadata(&lock_path);
            let detail = holder
                .map(|meta| format!("pid {}, role '{}', addr {}", meta.pid, meta.role, meta.addr))
                .unwrap_or_else(|| "another ElastOS host process".to_string());
            return Err(anyhow!(
                "another ElastOS host already owns {} ({detail}). Stop it before starting a second live host with the same runtime identity. Use exactly one live host for this home, typically `elastos serve` or `elastos gateway`.",
                lock_path.display()
            ));
        }
        Err(err) => {
            return Err(err)
                .with_context(|| format!("lock host process file {}", lock_path.display()));
        }
    };

    let meta = HostProcessMeta {
        pid: std::process::id(),
        role: role.to_string(),
        addr: addr.to_string(),
        generation: std::env::var("ELASTOS_UPDATE_GENERATION").unwrap_or_default(),
    };
    let mut file: &fs::File = &lock;
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    serde_json::to_writer_pretty(&mut file, &meta)?;
    writeln!(file)?;
    file.sync_data()?;
    remember_held_host_lock(&mut lock, role)?;

    Ok(lock)
}

pub fn authorize_host_process_start(data_dir: &Path) -> anyhow::Result<()> {
    crate::install_transaction::authorize_host_start(data_dir, &std::env::current_exe()?)
}

pub fn active_host_process(data_dir: &Path) -> anyhow::Result<Option<HostProcessInfo>> {
    let lock_path = host_lock_path(data_dir);
    if !lock_path.exists() {
        return Ok(None);
    }

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("open host lock {}", lock_path.display()))?;

    match FileLock::try_exclusive(file) {
        Ok(free) => {
            free.release()?;
            Ok(None)
        }
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
            Ok(read_lock_metadata(&lock_path).map(Into::into))
        }
        Err(err) => Err(err).with_context(|| format!("inspect host lock {}", lock_path.display())),
    }
}

fn host_lock_path(data_dir: &Path) -> PathBuf {
    data_dir.join("host-process.lock")
}

fn read_lock_metadata(lock_path: &Path) -> Option<HostProcessMeta> {
    serde_json::from_slice(&fs::read(lock_path).ok()?).ok()
}

impl From<HostProcessMeta> for HostProcessInfo {
    fn from(value: HostProcessMeta) -> Self {
        Self {
            pid: value.pid,
            role: value.role,
            addr: value.addr,
        }
    }
}

#[derive(Debug, Clone)]
struct BinarySupersessionWatch {
    current_exe_path: PathBuf,
    current_exe_stamp: BinaryFileStamp,
    current_binary_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BinaryFileStamp {
    len: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    device_id: u64,
    #[cfg(unix)]
    inode: u64,
}

trait BinarySnapshotReader {
    fn current_exe_path_raw(&self) -> anyhow::Result<PathBuf>;
    fn metadata_stamp(&self, path: &Path) -> anyhow::Result<BinaryFileStamp>;
    fn digest_snapshot(&self, path: &Path) -> anyhow::Result<BinarySnapshot>;
}

#[derive(Debug, Clone, Copy)]
struct RealBinarySnapshotReader;

#[derive(Debug, Clone, PartialEq, Eq)]
struct BinarySnapshot {
    stamp: BinaryFileStamp,
    sha256: String,
}

pub fn spawn_installed_binary_supersession_watch(data_dir: &Path, role: &str) {
    let Some(mut watch) = BinarySupersessionWatch::from_data_dir(data_dir) else {
        return;
    };
    let role = role.to_string();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Some(reason) = watch.superseded_reason() {
                eprintln!("[{}] {}", role, reason);
                std::process::exit(75);
            }
        }
    });
}

impl BinarySupersessionWatch {
    fn from_data_dir(data_dir: &Path) -> Option<Self> {
        Self::from_reader(data_dir, &RealBinarySnapshotReader).ok()
    }

    fn from_reader(data_dir: &Path, reader: &impl BinarySnapshotReader) -> anyhow::Result<Self> {
        let current_exe = normalize_deleted_exe_path(&reader.current_exe_path_raw()?);
        let snapshot = reader.digest_snapshot(&current_exe)?;
        let _ = data_dir;
        Ok(Self {
            current_exe_path: current_exe,
            current_exe_stamp: snapshot.stamp,
            current_binary_sha256: snapshot.sha256,
        })
    }

    fn superseded_reason(&mut self) -> Option<String> {
        self.superseded_reason_with(&RealBinarySnapshotReader)
    }

    fn superseded_reason_with(&mut self, reader: &impl BinarySnapshotReader) -> Option<String> {
        let current_exe_raw = match reader.current_exe_path_raw() {
            Ok(path) => path,
            Err(err) => {
                return Some(format!(
                    "host executable path could not be read for {}: {err:#}. Exiting stale host.",
                    self.current_exe_path.display()
                ));
            }
        };
        let raw_text = current_exe_raw.to_string_lossy();
        let current_exe = normalize_deleted_exe_path(&current_exe_raw);

        if raw_text.ends_with(" (deleted)") {
            return Some(format!(
                "host executable {} was replaced on disk. Exiting stale host.",
                self.current_exe_path.display()
            ));
        }

        if current_exe != self.current_exe_path {
            return Some(format!(
                "host executable moved from {} to {}. Exiting stale host.",
                self.current_exe_path.display(),
                current_exe.display()
            ));
        }

        let current_stamp = match reader.metadata_stamp(&self.current_exe_path) {
            Ok(stamp) => stamp,
            Err(err) => {
                return Some(format!(
                    "host executable {} could not be verified on disk: {err:#}. Exiting stale host.",
                    self.current_exe_path.display()
                ));
            }
        };

        if current_stamp == self.current_exe_stamp {
            return None;
        }

        let current_snapshot = match reader.digest_snapshot(&self.current_exe_path) {
            Ok(snapshot) => snapshot,
            Err(err) => {
                return Some(format!(
                    "host executable {} changed on disk and could not be re-verified: {err:#}. Exiting stale host.",
                    self.current_exe_path.display()
                ));
            }
        };
        if current_snapshot.sha256 != self.current_binary_sha256 {
            return Some(format!(
                "host binary {} changed on disk. Exiting stale host so the newer binary can take over.",
                self.current_exe_path.display()
            ));
        }

        self.current_exe_stamp = current_snapshot.stamp;
        None
    }
}

fn current_exe_path_raw() -> anyhow::Result<PathBuf> {
    #[cfg(unix)]
    {
        fs::read_link("/proc/self/exe")
            .or_else(|_| std::env::current_exe())
            .map_err(Into::into)
    }

    #[cfg(not(unix))]
    {
        std::env::current_exe().map_err(Into::into)
    }
}

fn normalize_deleted_exe_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(stripped) = text.strip_suffix(" (deleted)") {
        return PathBuf::from(stripped);
    }
    path.to_path_buf()
}

fn metadata_stamp(path: &Path) -> anyhow::Result<BinaryFileStamp> {
    let metadata = fs::metadata(path)?;
    binary_file_stamp_from_metadata(&metadata)
}

fn sha256_file_streaming(path: &Path) -> anyhow::Result<BinarySnapshot> {
    use sha2::Digest as _;

    let mut file = fs::File::open(path)?;
    let stamp = metadata_stamp_from_file(&file)?;
    let mut sha = sha2::Sha256::new();
    let mut buf = [0_u8; 16 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        sha.update(&buf[..read]);
    }
    Ok(BinarySnapshot {
        stamp,
        sha256: hex::encode(sha.finalize()),
    })
}

fn metadata_stamp_from_file(file: &fs::File) -> anyhow::Result<BinaryFileStamp> {
    let metadata = file.metadata()?;
    binary_file_stamp_from_metadata(&metadata)
}

fn binary_file_stamp_from_metadata(metadata: &fs::Metadata) -> anyhow::Result<BinaryFileStamp> {
    Ok(BinaryFileStamp {
        len: metadata.len(),
        modified: metadata.modified()?,
        #[cfg(unix)]
        device_id: {
            use std::os::unix::fs::MetadataExt as _;
            metadata.dev()
        },
        #[cfg(unix)]
        inode: {
            use std::os::unix::fs::MetadataExt as _;
            metadata.ino()
        },
    })
}

impl BinarySnapshotReader for RealBinarySnapshotReader {
    fn current_exe_path_raw(&self) -> anyhow::Result<PathBuf> {
        current_exe_path_raw()
    }

    fn metadata_stamp(&self, path: &Path) -> anyhow::Result<BinaryFileStamp> {
        metadata_stamp(path)
    }

    fn digest_snapshot(&self, path: &Path) -> anyhow::Result<BinarySnapshot> {
        sha256_file_streaming(path)
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::os::fd::{FromRawFd as _, RawFd};
    use std::os::unix::fs::MetadataExt as _;
    use std::os::unix::process::CommandExt as _;
    use std::path::Path;
    use std::process::{Child, Command, Stdio};

    /// A command another thread spawns while this process has `path` open
    /// shares each open description of it until exec closes its CLOEXEC
    /// copies. This command keeps its copies across exec to hold that window
    /// open until the value drops.
    pub(crate) struct SpawnedWhileOpen(Child);

    impl SpawnedWhileOpen {
        pub(crate) fn new(path: &Path) -> Self {
            let target = std::fs::metadata(path).unwrap();
            let open: Vec<RawFd> = std::fs::read_dir("/dev/fd")
                .unwrap()
                .filter_map(|entry| {
                    let fd: RawFd = entry.ok()?.file_name().to_str()?.parse().ok()?;
                    // Inspect without owning: the descriptor is never closed here.
                    let open =
                        std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd) });
                    let open = open.metadata().ok()?;
                    (open.dev() == target.dev() && open.ino() == target.ino()).then_some(fd)
                })
                .collect();
            assert!(!open.is_empty(), "{} is not open", path.display());
            let mut command = Command::new("/bin/sleep");
            command
                .arg("30")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            unsafe {
                command.pre_exec(move || {
                    for &fd in &open {
                        if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                            return Err(std::io::Error::last_os_error());
                        }
                    }
                    Ok(())
                });
            }
            Self(command.spawn().unwrap())
        }
    }

    impl Drop for SpawnedWhileOpen {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// Whether another open description can take the exclusive lock on `path` now.
    pub(crate) fn lock_is_free(path: &Path) -> bool {
        super::FileLock::try_exclusive(std::fs::File::open(path).unwrap()).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[derive(Debug, Clone)]
    struct TestBinarySnapshotReader {
        state: Rc<TestBinarySnapshotReaderState>,
    }

    #[derive(Debug)]
    struct TestBinarySnapshotReaderState {
        current_exe_raw: RefCell<PathBuf>,
        digest_reads: Cell<usize>,
    }

    impl TestBinarySnapshotReader {
        fn new(path: PathBuf) -> Self {
            Self {
                state: Rc::new(TestBinarySnapshotReaderState {
                    current_exe_raw: RefCell::new(path),
                    digest_reads: Cell::new(0),
                }),
            }
        }

        fn set_current_exe_raw(&self, path: PathBuf) {
            *self.state.current_exe_raw.borrow_mut() = path;
        }

        fn digest_reads(&self) -> usize {
            self.state.digest_reads.get()
        }
    }

    impl BinarySnapshotReader for TestBinarySnapshotReader {
        fn current_exe_path_raw(&self) -> anyhow::Result<PathBuf> {
            Ok(self.state.current_exe_raw.borrow().clone())
        }

        fn metadata_stamp(&self, path: &Path) -> anyhow::Result<BinaryFileStamp> {
            metadata_stamp(path)
        }

        fn digest_snapshot(&self, path: &Path) -> anyhow::Result<BinarySnapshot> {
            self.state
                .digest_reads
                .set(self.state.digest_reads.get().saturating_add(1));
            sha256_file_streaming(path)
        }
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).expect("write file");
    }

    fn test_watch(path: &Path) -> (BinarySupersessionWatch, TestBinarySnapshotReader) {
        let reader = TestBinarySnapshotReader::new(path.to_path_buf());
        let watch =
            BinarySupersessionWatch::from_reader(Path::new("/unused"), &reader).expect("watch");
        (watch, reader)
    }

    #[test]
    fn second_host_lock_for_same_data_dir_is_rejected() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _first = acquire_host_process_lock(temp.path(), "gateway", "127.0.0.1:8090")
            .expect("first lock");
        let err = acquire_host_process_lock(temp.path(), "serve", "0.0.0.0:3000")
            .expect_err("second lock should fail");
        assert!(
            err.to_string()
                .contains("another ElastOS host already owns"),
            "unexpected error: {err}"
        );
    }

    /// A record naming this pid, written by any process of the same user, is
    /// not a lock this process holds. The gateway still takes the real lock.
    #[test]
    fn gateway_takes_the_lock_when_only_a_forged_record_names_this_process() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = host_lock_path(temp.path());
        fs::write(
            &path,
            format!(
                "{{\"pid\":{},\"role\":\"serve\",\"addr\":\"127.0.0.1:3000\"}}\n",
                std::process::id()
            ),
        )
        .expect("forged record");
        assert_eq!(held_host_role(temp.path()), None);
        let gateway = publish_gateway_lock_if_unowned(temp.path(), "localhost:8090")
            .expect("gateway start")
            .expect("the forged record does not stand in for the lock");
        assert!(!test_support::lock_is_free(&path));
        assert!(holds_gateway_host_lock(temp.path()));
        let err = acquire_host_process_lock(temp.path(), "serve", "127.0.0.1:3000")
            .expect_err("a second host is refused");
        assert!(err
            .to_string()
            .contains("another ElastOS host already owns"));
        drop(gateway);
        assert_eq!(held_host_role(temp.path()), None);
    }

    /// A gateway inside `serve` keeps the `serve` lock that this process holds.
    #[test]
    fn gateway_inside_a_held_serve_lock_keeps_it() {
        let temp = tempfile::tempdir().expect("tempdir");
        let serve =
            acquire_host_process_lock(temp.path(), "serve", "127.0.0.1:3000").expect("serve lock");
        assert!(
            publish_gateway_lock_if_unowned(temp.path(), "localhost:8090")
                .expect("gateway inside serve starts")
                .is_none()
        );
        assert_eq!(held_host_role(temp.path()).as_deref(), Some("serve"));
        assert!(!holds_gateway_host_lock(temp.path()));
        serve.release().expect("release");
        assert_eq!(held_host_role(temp.path()), None);
    }

    #[test]
    fn model_host_lock_is_data_root_scoped() {
        let temp = tempfile::tempdir().expect("tempdir");
        let first_root = temp.path().join("first-home");
        let second_root = temp.path().join("second-home");
        let first = acquire_host_process_lock(&first_root, "serve", "127.0.0.1:3100")
            .expect("first Home owns its data root");
        let second = acquire_host_process_lock(&second_root, "serve", "127.0.0.1:3101")
            .expect("a distinct Home in the same process owns a separate data root");
        for root in [&first_root, &second_root] {
            assert!(acquire_host_process_lock(root, "serve", "127.0.0.1:3102").is_err());
        }
        drop(first);
        let first = acquire_host_process_lock(&first_root, "serve", "127.0.0.1:3100")
            .expect("release affects only the first data root");
        assert!(acquire_host_process_lock(&second_root, "serve", "127.0.0.1:3102").is_err());
        drop(second);
        let _second = acquire_host_process_lock(&second_root, "serve", "127.0.0.1:3101")
            .expect("the second data root can take its own released lock");
        assert!(acquire_host_process_lock(&first_root, "serve", "127.0.0.1:3102").is_err());
        drop(first);
    }

    #[test]
    fn ended_host_lock_is_free_while_a_command_spawned_under_it_runs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let host = acquire_host_process_lock(temp.path(), "update-recovery", "offline")
            .expect("first lock");
        let _command = test_support::SpawnedWhileOpen::new(&host_lock_path(temp.path()));
        drop(host);
        acquire_host_process_lock(temp.path(), "update-recovery", "offline")
            .expect("the next host must not wait for a command spawned under the last");
    }

    #[test]
    fn released_lock_reports_ok_and_is_free_while_a_command_spawned_under_it_runs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("released.lock");
        let lock = FileLock::try_exclusive(fs::File::create(&path).expect("lock file"))
            .expect("first lock");
        let _command = test_support::SpawnedWhileOpen::new(&path);
        lock.release().expect("release reports its unlock");
        assert!(test_support::lock_is_free(&path));
    }

    #[test]
    fn active_host_process_reports_live_owner() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _first = acquire_host_process_lock(temp.path(), "gateway", "127.0.0.1:8090")
            .expect("first lock");
        let owner = active_host_process(temp.path())
            .expect("inspect owner")
            .expect("owner present");
        assert_eq!(owner.role, "gateway");
        assert_eq!(owner.addr, "127.0.0.1:8090");
    }

    #[test]
    fn normalize_deleted_exe_path_strips_linux_deleted_suffix() {
        let raw = PathBuf::from("/home/test/.local/bin/elastos (deleted)");
        assert_eq!(
            normalize_deleted_exe_path(&raw),
            PathBuf::from("/home/test/.local/bin/elastos")
        );
    }

    #[test]
    fn supersession_watch_startup_reads_one_digest() {
        let temp = tempfile::tempdir().expect("tempdir");
        let exe = temp.path().join("elastos");
        write_file(&exe, b"original");

        let (_watch, reader) = test_watch(&exe);
        assert_eq!(reader.digest_reads(), 1);
    }

    #[test]
    fn supersession_watch_skips_digest_when_stamp_is_unchanged() {
        let temp = tempfile::tempdir().expect("tempdir");
        let exe = temp.path().join("elastos");
        write_file(&exe, b"original");

        let (mut watch, reader) = test_watch(&exe);
        for _ in 0..100 {
            assert_eq!(watch.superseded_reason_with(&reader), None);
        }
        assert_eq!(reader.digest_reads(), 1);
    }

    #[test]
    fn supersession_watch_updates_stamp_after_identical_bytes_rewrite() {
        let temp = tempfile::tempdir().expect("tempdir");
        let exe = temp.path().join("elastos");
        write_file(&exe, b"same bytes");

        let (mut watch, reader) = test_watch(&exe);
        let initial_reads = reader.digest_reads();

        let replacement = temp.path().join("replacement");
        write_file(&replacement, b"same bytes");
        fs::rename(&replacement, &exe).expect("replace file");

        assert_eq!(watch.superseded_reason_with(&reader), None);
        assert_eq!(reader.digest_reads(), initial_reads + 1);

        for _ in 0..100 {
            assert_eq!(watch.superseded_reason_with(&reader), None);
        }
        assert_eq!(reader.digest_reads(), initial_reads + 1);
    }

    #[test]
    fn supersession_watch_detects_same_path_content_replacement() {
        let temp = tempfile::tempdir().expect("tempdir");
        let exe = temp.path().join("elastos");
        write_file(&exe, b"original");

        let (mut watch, reader) = test_watch(&exe);
        let initial_reads = reader.digest_reads();
        write_file(&exe, b"changed content with a different length");

        let reason = watch.superseded_reason_with(&reader).expect("superseded");
        assert!(reason.contains("changed on disk"), "{reason}");
        assert_eq!(reader.digest_reads(), initial_reads + 1);
    }

    #[test]
    fn supersession_watch_detects_atomic_inode_replacement() {
        let temp = tempfile::tempdir().expect("tempdir");
        let exe = temp.path().join("elastos");
        write_file(&exe, b"original");

        let (mut watch, reader) = test_watch(&exe);
        let initial_reads = reader.digest_reads();
        let replacement = temp.path().join("replacement");
        write_file(&replacement, b"modified");
        fs::rename(&replacement, &exe).expect("replace file");

        let reason = watch.superseded_reason_with(&reader).expect("superseded");
        assert!(reason.contains("changed on disk"), "{reason}");
        assert_eq!(reader.digest_reads(), initial_reads + 1);
    }

    #[test]
    fn supersession_watch_detects_moved_executable_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let exe = temp.path().join("elastos");
        write_file(&exe, b"original");

        let (mut watch, reader) = test_watch(&exe);
        let moved = temp.path().join("elastos-new");
        reader.set_current_exe_raw(moved.clone());

        let reason = watch.superseded_reason_with(&reader).expect("superseded");
        assert!(reason.contains("moved"), "{reason}");
        assert!(reason.contains(&moved.display().to_string()), "{reason}");
    }

    #[test]
    fn supersession_watch_detects_deleted_executable() {
        let temp = tempfile::tempdir().expect("tempdir");
        let exe = temp.path().join("elastos");
        write_file(&exe, b"original");

        let (mut watch, reader) = test_watch(&exe);
        fs::remove_file(&exe).expect("remove file");

        let reason = watch.superseded_reason_with(&reader).expect("superseded");
        assert!(reason.contains("could not be verified on disk"), "{reason}");
    }

    #[test]
    fn supersession_watch_detects_linux_deleted_suffix() {
        let temp = tempfile::tempdir().expect("tempdir");
        let exe = temp.path().join("elastos");
        write_file(&exe, b"original");

        let (mut watch, reader) = test_watch(&exe);
        reader.set_current_exe_raw(PathBuf::from(format!("{} (deleted)", exe.display())));

        let reason = watch.superseded_reason_with(&reader).expect("superseded");
        assert!(reason.contains("replaced on disk"), "{reason}");
    }
}
