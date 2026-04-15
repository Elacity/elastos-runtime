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
    current_binary_sha256: String,
}

pub fn spawn_installed_binary_supersession_watch(data_dir: &Path, role: &str) {
    let Some(watch) = BinarySupersessionWatch::from_data_dir(data_dir) else {
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
        let current_exe = normalized_current_exe_path().ok()?;
        let current_binary_sha256 = sha256_file(&current_exe).ok()?;
        let _ = data_dir;
        Some(Self {
            current_exe_path: current_exe,
            current_binary_sha256,
        })
    }

    fn superseded_reason(&self) -> Option<String> {
        let current_exe_raw = current_exe_path_raw().ok()?;
        let raw_text = current_exe_raw.to_string_lossy();
        let current_exe = normalize_deleted_exe_path(&current_exe_raw);

        if current_exe != self.current_exe_path {
            return Some(format!(
                "host executable moved from {} to {}. Exiting stale host.",
                self.current_exe_path.display(),
                current_exe.display()
            ));
        }

        if raw_text.ends_with(" (deleted)") {
            return Some(format!(
                "host executable {} was replaced on disk. Exiting stale host.",
                self.current_exe_path.display()
            ));
        }

        let current_sha = sha256_file(&self.current_exe_path).ok()?;
        if current_sha != self.current_binary_sha256 {
            return Some(format!(
                "host binary {} changed on disk. Exiting stale host so the newer binary can take over.",
                self.current_exe_path.display()
            ));
        }

        if !self.current_exe_path.exists() {
            return Some(format!(
                "host executable {} no longer exists on disk. Exiting stale host.",
                self.current_exe_path.display()
            ));
        }

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

fn normalized_current_exe_path() -> anyhow::Result<PathBuf> {
    Ok(normalize_deleted_exe_path(&current_exe_path_raw()?))
}

fn normalize_deleted_exe_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(stripped) = text.strip_suffix(" (deleted)") {
        return PathBuf::from(stripped);
    }
    path.to_path_buf()
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    use sha2::Digest as _;

    let bytes = fs::read(path)?;
    Ok(hex::encode(sha2::Sha256::digest(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
