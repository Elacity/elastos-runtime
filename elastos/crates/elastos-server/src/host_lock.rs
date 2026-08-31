use std::fs::{self, OpenOptions};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context as _};
use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub struct HostProcessGuard {
    _file: fs::File,
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
}

pub fn acquire_host_process_lock(
    data_dir: &Path,
    role: &str,
    addr: &str,
) -> anyhow::Result<HostProcessGuard> {
    fs::create_dir_all(data_dir)?;
    let lock_path = host_lock_path(data_dir);
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("open host lock {}", lock_path.display()))?;

    match try_flock_exclusive_nonblocking(&file) {
        Ok(()) => {}
        Err(err) if is_would_block(&err) => {
            let holder = read_lock_metadata(&mut file);
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
    }

    let meta = HostProcessMeta {
        pid: std::process::id(),
        role: role.to_string(),
        addr: addr.to_string(),
    };
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    serde_json::to_writer_pretty(&mut file, &meta)?;
    writeln!(&mut file)?;
    file.sync_data()?;

    Ok(HostProcessGuard { _file: file })
}

pub fn active_host_process(data_dir: &Path) -> anyhow::Result<Option<HostProcessInfo>> {
    let lock_path = host_lock_path(data_dir);
    if !lock_path.exists() {
        return Ok(None);
    }

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("open host lock {}", lock_path.display()))?;

    match try_flock_exclusive_nonblocking(&file) {
        Ok(()) => {
            unlock_flock(&file)?;
            Ok(None)
        }
        Err(err) if is_would_block(&err) => Ok(read_lock_metadata(&mut file).map(Into::into)),
        Err(err) => Err(err).with_context(|| format!("inspect host lock {}", lock_path.display())),
    }
}

fn host_lock_path(data_dir: &Path) -> PathBuf {
    data_dir.join("host-process.lock")
}

fn read_lock_metadata(file: &mut fs::File) -> Option<HostProcessMeta> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;
    serde_json::from_str(&buf).ok()
}

fn try_flock_exclusive_nonblocking(file: &fs::File) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            return Ok(());
        }
        Err(std::io::Error::last_os_error().into())
    }

    #[cfg(not(unix))]
    {
        let _ = file;
        Ok(())
    }
}

fn unlock_flock(file: &fs::File) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        if rc == 0 {
            return Ok(());
        }
        Err(std::io::Error::last_os_error().into())
    }

    #[cfg(not(unix))]
    {
        let _ = file;
        Ok(())
    }
}

fn is_would_block(err: &anyhow::Error) -> bool {
    err.downcast_ref::<std::io::Error>()
        .map(|io| matches!(io.kind(), std::io::ErrorKind::WouldBlock))
        .unwrap_or(false)
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
