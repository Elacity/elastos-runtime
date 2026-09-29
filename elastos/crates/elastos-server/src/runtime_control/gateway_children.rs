//! The gateway owns subordinate process groups, including launches from its PTY.
use super::*;
use std::process::{Child, Command};

const OWNER_ENV: &str = "ELASTOS_MANAGED_GATEWAY_OWNER";

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Owner {
    data_dir: PathBuf,
    pid: u32,
    generation: String,
}

impl Owner {
    pub(crate) fn read(data_dir: &Path) -> anyhow::Result<Self> {
        let coords = read_private_runtime_coords(&gateway_runtime_coord_path(data_dir))
            .ok_or_else(|| anyhow::anyhow!("gateway owner coordinates unavailable"))?;
        anyhow::ensure!(coords.is_gateway_runtime(), "gateway owner kind mismatch");
        Ok(Self {
            data_dir: data_dir.to_path_buf(),
            pid: coords.pid,
            generation: sha256_bytes(&serde_json::to_vec(&coords)?),
        })
    }

    fn active(&self) -> bool {
        read_private_runtime_coords(&gateway_runtime_coord_path(&self.data_dir)).is_some_and(
            |coords| {
                coords.pid == self.pid
                    && sha256_bytes(
                        &serde_json::to_vec(&coords).expect("runtime coordinates serialize"),
                    ) == self.generation
            },
        ) && pid_is_alive(self.pid)
    }

    pub(crate) fn configure(&self, command: &mut Command) -> anyhow::Result<()> {
        command.env(OWNER_ENV, serde_json::to_string(self)?);
        Ok(())
    }

    pub(crate) fn lock(&self) -> anyhow::Result<std::fs::File> {
        use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.data_dir.join("gateway-runtime-ownership.lock"))?;
        anyhow::ensure!(
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0,
            "lock gateway runtime ownership: {}",
            std::io::Error::last_os_error()
        );
        Ok(file)
    }

    pub(crate) fn lock_start(&self) -> anyhow::Result<std::fs::File> {
        let lock = self.lock()?;
        anyhow::ensure!(self.active(), "gateway owner stopped during runtime start");
        Ok(lock)
    }

    fn directory(&self) -> PathBuf {
        self.data_dir
            .join("gateway-owned-runtimes")
            .join(&self.generation)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Record {
    pid: u32,
    process_start: String,
    coords_path: PathBuf,
}

fn process_start(pid: u32) -> Option<String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .output()
        .ok()?;
    let value = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn signal_group(pid: u32, signal: i32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
}

fn group_exists(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(-(pid as i32), 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        pid_is_alive(pid)
    }
}

/// Keeps startup cancellation from abandoning a newly spawned group.
pub(crate) struct StartingChild {
    pub child: Child,
    record: Option<PathBuf>,
    ready: bool,
}

impl StartingChild {
    pub(crate) fn new(
        child: Child,
        owner: Option<&Owner>,
        coords_path: &Path,
    ) -> anyhow::Result<Self> {
        let mut guard = Self {
            child,
            record: None,
            ready: false,
        };
        if let Some(owner) = owner {
            anyhow::ensure!(owner.active(), "gateway owner stopped during runtime start");
            let directory = owner.directory();
            std::fs::create_dir_all(&directory)?;
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
            let record = Record {
                pid: guard.child.id(),
                process_start: process_start(guard.child.id())
                    .ok_or_else(|| anyhow::anyhow!("managed runtime exited during start"))?,
                coords_path: coords_path.to_path_buf(),
            };
            let path = directory.join(format!("{}.json", record.pid));
            let temporary = path.with_extension("tmp");
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            serde_json::to_writer(file, &record)?;
            std::fs::rename(&temporary, &path)?;
            guard.record = Some(path);
        }
        Ok(guard)
    }

    pub(crate) fn ready(&mut self) {
        self.ready = true;
    }
}

impl Drop for StartingChild {
    fn drop(&mut self) {
        if !self.ready {
            signal_group(self.child.id(), libc::SIGKILL);
            let _ = self.child.kill();
            let _ = self.child.wait();
            if let Some(path) = &self.record {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Covers abrupt gateway loss and children started by a gateway-owned Terminal.
pub fn watch_gateway_owner() -> anyhow::Result<()> {
    let Ok(value) = std::env::var(OWNER_ENV) else {
        return Ok(());
    };
    let owner: Owner = serde_json::from_str(&value)?;
    std::thread::spawn(move || loop {
        if !owner.active() {
            // Reuse main's bounded process-group shutdown, including providers.
            unsafe {
                libc::kill(std::process::id() as i32, libc::SIGTERM);
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    });
    Ok(())
}

pub(crate) async fn shutdown(owner: Option<Owner>) -> anyhow::Result<()> {
    let Some(owner) = owner else {
        return Ok(());
    };
    let directory = owner.directory();
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let record: Record = serde_json::from_slice(&std::fs::read(&path)?)?;
        let current_start = process_start(record.pid);
        anyhow::ensure!(
            current_start.as_ref() == Some(&record.process_start) || !pid_is_alive(record.pid),
            "managed runtime {} identity changed or is unavailable; preserving ownership record",
            record.pid
        );
        if current_start.as_ref() == Some(&record.process_start)
            || (!pid_is_alive(record.pid) && group_exists(record.pid))
        {
            signal_group(record.pid, libc::SIGTERM);
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while group_exists(record.pid) && std::time::Instant::now() < deadline {
                // Reap direct children; PTY-created children are reaped by their parent.
                unsafe {
                    libc::waitpid(record.pid as i32, std::ptr::null_mut(), libc::WNOHANG);
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            if group_exists(record.pid) {
                signal_group(record.pid, libc::SIGKILL);
                unsafe {
                    libc::waitpid(record.pid as i32, std::ptr::null_mut(), 0);
                }
                let deadline = std::time::Instant::now() + Duration::from_secs(2);
                while group_exists(record.pid) && std::time::Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                anyhow::ensure!(
                    !group_exists(record.pid),
                    "managed runtime group {} survived shutdown",
                    record.pid
                );
            }
            // A group can disappear just before its direct child is reaped.
            unsafe {
                libc::waitpid(record.pid as i32, std::ptr::null_mut(), libc::WNOHANG);
            }
        }
        if read_private_runtime_coords(&record.coords_path)
            .is_some_and(|coords| coords.pid == record.pid)
        {
            let _ = std::fs::remove_file(&record.coords_path);
        }
        std::fs::remove_file(path)?;
    }
    let _ = std::fs::remove_dir(directory);
    Ok(())
}

#[cfg(test)]
pub(crate) async fn test_child(owner: &Owner, coords_path: &Path) -> (StartingChild, u32) {
    use std::os::unix::process::CommandExt;
    let helper_file = coords_path.with_extension("helper");
    let _ = std::fs::remove_file(&helper_file);
    let child = Command::new("sh")
        .args(["-c", "sleep 60 & helper=$!; echo $helper > \"$1\"; trap 'kill $helper 2>/dev/null; wait $helper 2>/dev/null; exit 0' TERM; wait $helper", "test"])
        .arg(&helper_file).process_group(0).spawn().unwrap();
    let guard = StartingChild::new(child, Some(owner), coords_path).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(pid) = std::fs::read_to_string(&helper_file)
            .ok()
            .and_then(|s| s.trim().parse().ok())
        {
            return (guard, pid);
        }
        assert!(std::time::Instant::now() < deadline, "helper did not start");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_start_reaps_group_and_other_gateway_generation_is_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let coords = RuntimeCoords {
            api_url: "http://127.0.0.1:1".into(),
            attach_secret: "test".into(),
            pid: std::process::id(),
            runtime_kind: RUNTIME_KIND_GATEWAY.into(),
            binary_sha256: String::new(),
            policy_sha256: String::new(),
            dependency_sha256: String::new(),
        };
        let _published = publish_gateway_runtime_coords(temp.path(), coords).unwrap();
        let owner = Owner::read(temp.path()).unwrap();
        let path = temp.path().join("child.json");
        let (guard, helper) = test_child(&owner, &path).await;
        let pid = guard.child.id();
        let mut other = owner.clone();
        other.generation = "another-generation".into();
        shutdown(Some(other)).await.unwrap();
        assert!(pid_is_alive(pid));
        assert!(pid_is_alive(helper));
        drop(guard);
        assert!(!pid_is_alive(pid));
        assert_eq!(std::fs::read_dir(owner.directory()).unwrap().count(), 0);

        // A leader can exit before its providers. Its group remains owned.
        let (mut guard, helper) = test_child(&owner, &path).await;
        guard.child.kill().unwrap();
        guard.child.wait().unwrap();
        guard.ready();
        drop(guard);
        assert!(pid_is_alive(helper));
        shutdown(Some(owner.clone())).await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while pid_is_alive(helper) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!pid_is_alive(helper));

        // A launcher waiting on registration cannot spawn after owner retirement.
        let lock = owner.lock().unwrap();
        let (started, waiting) = std::sync::mpsc::channel();
        let launcher = std::thread::spawn(move || {
            started.send(()).unwrap();
            owner.lock_start()
        });
        waiting.recv().unwrap();
        drop(_published);
        drop(lock);
        assert!(launcher.join().unwrap().is_err());
    }
}
