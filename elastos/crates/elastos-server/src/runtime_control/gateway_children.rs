//! The gateway owns subordinate process groups, including launches from its PTY.
use super::*;
use crate::update_controller::child as process_ownership;
use anyhow::Context;
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

    pub(crate) fn read_for_generation(
        data_dir: &Path,
        pid: u32,
        update_generation: &str,
    ) -> anyhow::Result<Option<Self>> {
        let path = gateway_runtime_coord_path(data_dir);
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        let coords = read_private_runtime_coords(&path)
            .ok_or_else(|| anyhow::anyhow!("gateway owner coordinates are invalid or changed"))?;
        anyhow::ensure!(
            coords.is_gateway_runtime()
                && coords.pid == pid
                && !update_generation.is_empty()
                && coords.generation == update_generation,
            "gateway owner generation mismatch"
        );
        Ok(Some(Self {
            data_dir: data_dir.to_path_buf(),
            pid,
            generation: sha256_bytes(&serde_json::to_vec(&coords)?),
        }))
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
#[serde(deny_unknown_fields)]
struct Record {
    pid: u32,
    process_start: String,
    coords_path: PathBuf,
}

fn process_start(pid: u32) -> Option<String> {
    process_ownership::process_start(pid)
}

fn signal_group(pid: u32, signal: i32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), signal);
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
            let pid = self.child.id();
            if process_ownership::observe_exit(pid).is_ok() {
                signal_group(pid, libc::SIGKILL);
            }
            let deadline = std::time::Instant::now() + Duration::from_millis(200);
            while std::time::Instant::now() < deadline {
                match process_ownership::group_descendants(pid) {
                    Ok(members) if members.is_empty() => {
                        if matches!(self.child.try_wait(), Ok(Some(_))) {
                            if let Some(path) = &self.record {
                                let _ = std::fs::remove_file(path);
                            }
                            break;
                        }
                    }
                    Err(_) => break,
                    _ => {}
                }
                std::thread::sleep(Duration::from_millis(5));
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
    shutdown_with_limits(owner, Duration::from_secs(3), Duration::from_secs(10)).await
}

async fn shutdown_with_limits(
    owner: Option<Owner>,
    grace: Duration,
    total: Duration,
) -> anyhow::Result<()> {
    let Some(owner) = owner else {
        return Ok(());
    };
    let started = tokio::time::Instant::now();
    let deadline = started + total;
    let kill_deadline = (started + grace).min(deadline);
    let directory = owner.directory();
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let mut failures = Vec::new();
    let mut pending = Vec::new();
    for (index, entry) in entries.enumerate() {
        if index >= 4096 || tokio::time::Instant::now() >= deadline {
            failures.push("ownership record scan exceeded its bound".to_owned());
            break;
        }
        let result: anyhow::Result<_> = (|| {
            let path = entry?.path();
            let record = read_record(&path)?;
            anyhow::ensure!(
                record.coords_path.starts_with(&owner.data_dir),
                "managed runtime coordinates are outside their owner data root"
            );
            validate_record_birth(&record)
                .context("validate managed runtime birth before capture")?;
            let anchored = direct_child_anchor(&record)
                .context("capture managed runtime direct child and group")?;
            if anchored {
                checked_signal(&record, libc::SIGTERM)
                    .context("send managed runtime group TERM")?;
            }
            Ok(Pending {
                path,
                record,
                anchored,
                killed: false,
            })
        })();
        match result {
            Ok(record) => pending.push(record),
            Err(error) => failures.push(format!("{error:#}")),
        }
    }
    while !pending.is_empty() {
        let mut retained = Vec::new();
        for mut entry in pending {
            if tokio::time::Instant::now() >= deadline {
                retained.push(entry);
                continue;
            }
            match settle_record(&mut entry, tokio::time::Instant::now() >= kill_deadline) {
                Ok(true) => {
                    if let Err(error) = retire_record(entry) {
                        failures.push(format!("{error:#}"));
                    }
                }
                Ok(false) => retained.push(entry),
                Err(error) => failures.push(format!("{error:#}")),
            }
        }
        pending = retained;
        if pending.is_empty() {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            failures.push(format!(
                "{} managed runtime groups remain at shutdown deadline",
                pending.len()
            ));
            break;
        }
        tokio::time::sleep_until(
            (tokio::time::Instant::now() + Duration::from_millis(20)).min(deadline),
        )
        .await;
    }
    if failures.is_empty() {
        std::fs::remove_dir(directory)?;
        Ok(())
    } else {
        anyhow::bail!(
            "gateway owned group cleanup incomplete: {}",
            failures.join("; ")
        )
    }
}

struct Pending {
    path: PathBuf,
    record: Record,
    anchored: bool,
    killed: bool,
}

fn read_record(path: &Path) -> anyhow::Result<Record> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file()
            && metadata.mode() & 0o777 == 0o600
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.len() <= 64 * 1024,
        "invalid managed runtime ownership record"
    );
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= 64 * 1024,
        "managed runtime record exceeded its bound"
    );
    let record: Record = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(
        path.file_name().and_then(|name| name.to_str())
            == Some(format!("{}.json", record.pid).as_str())
            && !record.process_start.is_empty()
            && record.process_start.len() <= 256
            && record.coords_path.is_absolute(),
        "managed runtime ownership record is incomplete"
    );
    Ok(record)
}

fn validate_record_birth(record: &Record) -> anyhow::Result<()> {
    if let Some(current) = process_ownership::process_identity(record.pid)? {
        // Legacy lstart records remain readable. Their whole-second birth has
        // insufficient precision for a live signal claim, so recovery keeps them.
        anyhow::ensure!(current == record.process_start,
            "managed runtime {} birth differs or is a live legacy record; preserving ownership record", record.pid);
    }
    Ok(())
}

fn direct_child_anchor(record: &Record) -> anyhow::Result<bool> {
    match process_ownership::observe_exit(record.pid) {
        Ok(_) => {
            anyhow::ensure!(
                process_ownership::process_group(record.pid)? == Some(record.pid),
                "managed runtime group differs from its ownership record"
            );
            Ok(true)
        }
        Err(error) if error.raw_os_error() == Some(libc::ECHILD) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn checked_signal(record: &Record, signal: i32) -> anyhow::Result<()> {
    // The caller already proved this direct child's birth and process group.
    // Its unreaped PID keeps that anchor when macOS hides its exited birth.
    observe_anchored_exit(
        || {
            process_ownership::observe_exit(record.pid)
                .context("observe managed runtime exit before group signal")
        },
        || {
            validate_record_birth(record)
                .context("validate live managed runtime birth before signal")?;
            let group = process_ownership::process_group(record.pid)
                .context("read live managed runtime group before signal")?;
            let Some(group) = group else {
                return Err(std::io::Error::from_raw_os_error(libc::ESRCH))
                    .context("live managed runtime group became unavailable before signal");
            };
            anyhow::ensure!(
                group == record.pid,
                "managed runtime group differs from its ownership record"
            );
            Ok(())
        },
    )?
    .require_observed()?;
    process_ownership::signal_group(record.pid, signal).context("signal managed runtime group")?;
    Ok(())
}

#[derive(Debug)]
enum AnchoredExit {
    Observed(Option<ExitStatus>),
    AwaitingExit(anyhow::Error),
}

impl AnchoredExit {
    fn require_observed(self) -> anyhow::Result<Option<ExitStatus>> {
        match self {
            Self::Observed(exited) => Ok(exited),
            Self::AwaitingExit(error) => Err(error),
        }
    }
}

/// The caller retains a direct child with a previously proved birth and group.
fn observe_anchored_exit(
    mut observe: impl FnMut() -> anyhow::Result<Option<ExitStatus>>,
    inspect_live: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<AnchoredExit> {
    let exited = observe()?;
    if exited.is_some() {
        return Ok(AnchoredExit::Observed(exited));
    }
    if let Err(error) = inspect_live() {
        if cfg!(any(target_os = "macos", test))
            && error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.raw_os_error() == Some(libc::ESRCH))
        {
            // Darwin can hide the birth/group after the preceding live event.
            return match observe()
                .context("reobserve retained managed runtime child after ESRCH")?
            {
                Some(exited) => Ok(AnchoredExit::Observed(Some(exited))),
                None => Ok(AnchoredExit::AwaitingExit(error)),
            };
        }
        return Err(error);
    }
    Ok(AnchoredExit::Observed(None))
}

fn settle_record(entry: &mut Pending, force: bool) -> anyhow::Result<bool> {
    settle_record_with_observer(entry, force, |record| {
        observe_anchored_exit(
            || {
                process_ownership::observe_exit(record.pid)
                    .context("observe managed runtime exit during settlement")
            },
            || {
                validate_record_birth(record)
                    .context("validate live managed runtime birth during settlement")
            },
        )
    })
}

fn settle_record_with_observer(
    entry: &mut Pending,
    force: bool,
    observe: impl FnOnce(&Record) -> anyhow::Result<AnchoredExit>,
) -> anyhow::Result<bool> {
    if entry.anchored {
        let exited = match observe(&entry.record)? {
            AnchoredExit::Observed(exited) => exited,
            AnchoredExit::AwaitingExit(_) => return Ok(false),
        };
        if force && !entry.killed {
            checked_signal(&entry.record, libc::SIGKILL)
                .context("send managed runtime group KILL during settlement")?;
            entry.killed = true;
        }
        if exited.is_some()
            && process_ownership::group_descendants(entry.record.pid)
                .context("inspect managed runtime descendants during settlement")?
                .is_empty()
        {
            let result = unsafe {
                libc::waitpid(
                    entry.record.pid as libc::pid_t,
                    std::ptr::null_mut(),
                    libc::WNOHANG,
                )
            };
            if result < 0 {
                return Err(std::io::Error::last_os_error())
                    .context("reap managed runtime during settlement");
            }
            if result == 0 {
                return Ok(false);
            }
            entry.anchored = false;
        } else {
            return Ok(false);
        }
    }
    process_ownership::generation_gone(entry.record.pid, &entry.record.process_start)
        .context("confirm managed runtime generation disappearance after settlement")
}

fn retire_record(entry: Pending) -> anyhow::Result<()> {
    // Preserve a record which a concurrent owner replaced during inspection.
    let current = read_record(&entry.path)?;
    anyhow::ensure!(
        current.pid == entry.record.pid
            && current.process_start == entry.record.process_start
            && current.coords_path == entry.record.coords_path,
        "managed runtime ownership record changed during cleanup"
    );
    anyhow::ensure!(
        process_ownership::generation_gone(entry.record.pid, &entry.record.process_start)
            .context("confirm managed runtime generation disappearance during retirement")?,
        "managed runtime generation remains during record retirement"
    );
    if read_private_runtime_coords(&entry.record.coords_path)
        .is_some_and(|coords| coords.pid == entry.record.pid)
    {
        std::fs::remove_file(&entry.record.coords_path)?;
    }
    std::fs::remove_file(entry.path)?;
    Ok(())
}

pub(crate) fn refuse_unreconciled_groups(data_dir: &Path) -> anyhow::Result<()> {
    let generations = match std::fs::read_dir(data_dir.join("gateway-owned-runtimes")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for (index, generation) in generations.enumerate() {
        anyhow::ensure!(index < 4096, "gateway ownership scan exceeded its bound");
        let generation = generation?;
        anyhow::ensure!(
            generation.file_type()?.is_dir(),
            "gateway ownership entry needs reconciliation"
        );
        anyhow::ensure!(
            std::fs::read_dir(generation.path())?.next().is_none(),
            "gateway owned groups still need reconciliation"
        );
    }
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
    use std::os::unix::process::{CommandExt, ExitStatusExt};

    fn fixture_owner(data: &Path, generation: &str) -> (GatewayRuntimeCoordsGuard, Owner) {
        let coords = RuntimeCoords {
            api_url: "http://127.0.0.1:1".into(),
            attach_secret: "test".into(),
            pid: std::process::id(),
            runtime_kind: RUNTIME_KIND_GATEWAY.into(),
            binary_sha256: String::new(),
            policy_sha256: String::new(),
            dependency_sha256: String::new(),
            generation: generation.into(),
            home_url: String::new(),
        };
        let published = publish_gateway_runtime_coords(data, coords).unwrap();
        let owner = Owner::read(data).unwrap();
        (published, owner)
    }

    #[test]
    fn exact_gateway_generation_is_required_before_shutdown_capture() {
        let temp = tempfile::tempdir().unwrap();
        assert!(
            Owner::read_for_generation(temp.path(), std::process::id(), "gateway-fixture")
                .unwrap()
                .is_none()
        );
        let (_published, owner) = fixture_owner(temp.path(), "gateway-fixture");
        assert!(
            Owner::read_for_generation(temp.path(), owner.pid, "gateway-fixture")
                .unwrap()
                .is_some()
        );
        assert!(Owner::read_for_generation(temp.path(), owner.pid + 1, "gateway-fixture").is_err());
        assert!(Owner::read_for_generation(temp.path(), owner.pid, "another-generation").is_err());
        assert!(Owner::read_for_generation(temp.path(), owner.pid, "").is_err());
        std::fs::write(
            gateway_runtime_coord_path(temp.path()),
            b"invalid coordinates",
        )
        .unwrap();
        assert!(Owner::read_for_generation(temp.path(), owner.pid, "gateway-fixture").is_err());
    }

    #[test]
    fn retained_anchor_retries_only_lost_darwin_visibility_with_an_exited_child() {
        let io_error = |errno| anyhow::Error::from(std::io::Error::from_raw_os_error(errno));
        for case in [
            "already exited",
            "valid live child",
            "birth vanished during exit",
            "group vanished during exit",
            "birth mismatch",
            "group mismatch",
            "permission denied",
            "other lookup failure",
            "lookup lost child ownership",
            "still live after ESRCH",
            "reaped after ESRCH",
            "uncertain after ESRCH",
            "initial child ownership lost",
            "initial exit uncertain",
            "initial ESRCH",
        ] {
            let status = ExitStatus::from_raw(15);
            let before = match case {
                "already exited" => Ok(Some(status)),
                "initial child ownership lost" => Err(io_error(libc::ECHILD)),
                "initial exit uncertain" => Err(io_error(libc::EPERM)),
                "initial ESRCH" => Err(io_error(libc::ESRCH)),
                _ => Ok(None),
            };
            let after = match case {
                "still live after ESRCH" => Ok(None),
                "reaped after ESRCH" => Err(io_error(libc::ECHILD)),
                "uncertain after ESRCH" => Err(io_error(libc::EPERM)),
                _ => Ok(Some(status)),
            };
            let inspection = match case {
                "valid live child" => Ok(()),
                "birth mismatch" => Err(anyhow::anyhow!("managed runtime birth differs")),
                "group mismatch" => Err(anyhow::anyhow!("managed runtime group differs")),
                "permission denied" => Err(io_error(libc::EPERM)),
                "other lookup failure" => Err(io_error(libc::EACCES)),
                "lookup lost child ownership" => Err(io_error(libc::ECHILD)),
                _ => Err(io_error(libc::ESRCH)),
            };
            let mut observations = vec![after, before];
            let mut observed = 0;
            let mut inspected = false;
            let result = observe_anchored_exit(
                || {
                    observed += 1;
                    observations.pop().unwrap()
                },
                || {
                    inspected = true;
                    inspection.context("inspect retained live child")
                },
            )
            .and_then(AnchoredExit::require_observed);
            match case {
                "already exited" | "birth vanished during exit" | "group vanished during exit" => {
                    assert_eq!(result.unwrap(), Some(status), "{case}");
                }
                "valid live child" => assert!(result.unwrap().is_none(), "{case}"),
                _ => assert!(result.is_err(), "{case}"),
            }
            let retry = matches!(
                case,
                "birth vanished during exit"
                    | "group vanished during exit"
                    | "still live after ESRCH"
                    | "reaped after ESRCH"
                    | "uncertain after ESRCH"
            );
            assert_eq!(observed, if retry { 2 } else { 1 }, "{case}");
            assert_eq!(
                inspected,
                !matches!(
                    case,
                    "already exited"
                        | "initial child ownership lost"
                        | "initial exit uncertain"
                        | "initial ESRCH"
                ),
                "{case}"
            );
        }
    }

    #[test]
    fn delayed_exit_preserves_the_signal_error_until_an_exit_is_observed() {
        let status = ExitStatus::from_raw(15);
        let mut events = [None, None, None, Some(status)].into_iter();
        let mut sample = || {
            observe_anchored_exit(
                || Ok(events.next().unwrap()),
                || {
                    Err(std::io::Error::from_raw_os_error(libc::ESRCH))
                        .context("inspect retained live child birth")
                },
            )
            .unwrap()
        };
        let awaiting = sample();
        assert!(matches!(&awaiting, AnchoredExit::AwaitingExit(_)));
        let error = awaiting.require_observed().unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(libc::ESRCH)
        );
        assert!(format!("{error:#}").starts_with("inspect retained live child birth:"));
        assert_eq!(sample().require_observed().unwrap(), Some(status));
    }

    #[test]
    fn awaiting_exit_retains_the_record_and_anchor_across_forced_settlement_retries() {
        use std::os::unix::fs::OpenOptionsExt;
        let temp = tempfile::tempdir().unwrap();
        // A synthetic absent PID makes any accidental signal path refuse.
        let pid = libc::pid_t::MAX as u32;
        let path = temp.path().join(format!("{pid}.json"));
        let record = Record {
            pid,
            process_start: "macos:fixture:retained-anchor".into(),
            coords_path: temp.path().join("child-coords.json"),
        };
        let bytes = serde_json::to_vec(&record).unwrap();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        serde_json::to_writer(file, &record).unwrap();
        let mut entry = Pending {
            path: path.clone(),
            record,
            anchored: true,
            killed: false,
        };
        for force in [false, true, true] {
            assert!(!settle_record_with_observer(&mut entry, force, |_| {
                observe_anchored_exit(
                    || Ok(None),
                    || {
                        Err(std::io::Error::from_raw_os_error(libc::ESRCH))
                            .context("inspect retained live child birth")
                    },
                )
            })
            .unwrap());
            assert!(entry.anchored);
            assert!(!entry.killed);
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert_eq!(
                read_record(&path).unwrap().process_start,
                entry.record.process_start
            );
        }
    }

    #[tokio::test]
    async fn anchored_exited_group_retires_after_kernel_hides_birth() {
        let temp = tempfile::tempdir().unwrap();
        let (_published, owner) = fixture_owner(temp.path(), "gateway-fixture");
        let child = Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        let mut child =
            StartingChild::new(child, Some(&owner), &temp.path().join("child-coords.json"))
                .unwrap();
        let path = child.record.clone().unwrap();
        let record = read_record(&path).unwrap();
        validate_record_birth(&record).unwrap();
        assert!(direct_child_anchor(&record).unwrap());
        let mut entry = Pending {
            path: path.clone(),
            record,
            anchored: true,
            killed: false,
        };
        let unrelated = Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        let mut unrelated =
            StartingChild::new(unrelated, None, &temp.path().join("unrelated.json")).unwrap();
        checked_signal(&entry.record, libc::SIGTERM).unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while process_ownership::observe_exit(entry.record.pid)
            .unwrap()
            .is_none()
        {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        #[cfg(target_os = "macos")]
        {
            // A new cleanup scan still refuses the now hidden birth identity.
            assert!(shutdown(Some(owner)).await.is_err());
            assert!(path.exists());
        }
        assert!(settle_record(&mut entry, true).unwrap());
        assert!(!entry.anchored);
        retire_record(entry).unwrap();
        child.ready();
        assert!(!path.exists());
        assert!(unrelated.child.try_wait().unwrap().is_none());
    }

    #[tokio::test]
    async fn failed_identity_preserves_its_record_while_other_groups_are_drained() {
        let temp = tempfile::tempdir().unwrap();
        let (_published, owner) = fixture_owner(temp.path(), "gateway-fixture");
        let foreign = Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        let mut foreign = StartingChild::new(
            foreign,
            Some(&owner),
            &temp.path().join("foreign-coords.json"),
        )
        .unwrap();
        let foreign_path = foreign.record.clone().unwrap();
        let mut record = read_record(&foreign_path).unwrap();
        record.process_start = "another-generation".into();
        std::fs::write(&foreign_path, serde_json::to_vec(&record).unwrap()).unwrap();
        let valid = Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        let mut valid =
            StartingChild::new(valid, Some(&owner), &temp.path().join("valid-coords.json"))
                .unwrap();
        let valid_pid = valid.child.id();
        let valid_birth = process_start(valid_pid).unwrap();
        let valid_path = valid.record.clone().unwrap();
        let shutdown_error = shutdown_with_limits(
            Some(owner.clone()),
            Duration::from_millis(20),
            Duration::from_millis(500),
        )
        .await
        .unwrap_err();
        assert!(foreign.child.try_wait().unwrap().is_none());
        assert!(foreign_path.exists());
        assert!(
            !valid_path.exists(),
            "valid ownership record remains after cleanup: {shutdown_error:#}"
        );
        assert!(process_ownership::generation_gone(valid_pid, &valid_birth).unwrap());
        valid.ready();
        assert!(refuse_unreconciled_groups(temp.path()).is_err());
        foreign.child.kill().unwrap();
        foreign.child.wait().unwrap();
        foreign.ready();
        shutdown(Some(owner)).await.unwrap();
        refuse_unreconciled_groups(temp.path()).unwrap();
    }

    #[tokio::test]
    async fn legacy_absent_records_retire_and_live_legacy_records_remain_blocked() {
        use std::os::unix::fs::OpenOptionsExt;
        let temp = tempfile::tempdir().unwrap();
        let (_published, owner) = fixture_owner(temp.path(), "gateway-fixture");
        std::fs::create_dir_all(owner.directory()).unwrap();
        let write = |pid| {
            let path = owner.directory().join(format!("{pid}.json"));
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .unwrap();
            serde_json::to_writer(
                file,
                &Record {
                    pid,
                    process_start: "Fri Oct  2 00:00:00 2026".into(),
                    coords_path: temp.path().join("legacy-coords.json"),
                },
            )
            .unwrap();
            path
        };
        let absent = write(i32::MAX as u32);
        let live = write(std::process::id());
        assert!(shutdown(Some(owner)).await.is_err());
        assert!(!absent.exists());
        assert!(live.exists());
        assert!(refuse_unreconciled_groups(temp.path()).is_err());
    }

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
            generation: String::new(),
            home_url: String::new(),
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

        // A reaped leader leaves a group whose live members need reconciliation.
        let child = Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        let mut guard = StartingChild::new(child, Some(&owner), &path).unwrap();
        let mut helper = Command::new("sleep")
            .arg("60")
            .process_group(guard.child.id() as i32)
            .spawn()
            .unwrap();
        guard.child.kill().unwrap();
        guard.child.wait().unwrap();
        guard.ready();
        drop(guard);
        assert!(pid_is_alive(helper.id()));
        assert!(shutdown_with_limits(
            Some(owner.clone()),
            Duration::from_millis(20),
            Duration::from_millis(100)
        )
        .await
        .is_err());
        assert!(refuse_unreconciled_groups(temp.path()).is_err());
        helper.kill().unwrap();
        helper.wait().unwrap();
        shutdown(Some(owner.clone())).await.unwrap();
        refuse_unreconciled_groups(temp.path()).unwrap();

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
