//! Prepared per-launch Browser rootfs copies (Linux).
//!
//! Each launch boots a private writable copy of the admitted seed. Without
//! reflink (ext4 on an SD card) a full copy takes minutes, longer than any
//! launch budget (#90, Jetson). A launch therefore renames a prepared copy
//! into its session, and a background refill copies the next one. A copy's
//! name carries the admitted rootfs hash from the image manifest, as
//! browser-vm-prepare-rootfs-pool.mjs named it, so a copy of another image is
//! never booted.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{ErrorKind, Write as _};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

pub(crate) const POOL_DIR_ENV: &str = "ELASTOS_BROWSER_VM_ROOTFS_POOL_DIR";
pub(crate) const POOL_COUNT_ENV: &str = "ELASTOS_BROWSER_VM_ROOTFS_POOL_REFILL_COUNT";
pub(crate) const MANIFEST_ENV: &str = "ELASTOS_BROWSER_VM_ROOTFS_MANIFEST";
pub(crate) const ROOTFS_ENV: &str = "ELASTOS_BROWSER_VM_ROOTFS";
/// The process whose life bounds a refill; the refill's parent when unset.
pub(crate) const ANCHOR_ENV: &str = "ELASTOS_BROWSER_VM_ROOTFS_POOL_ANCHOR_PID";
/// The supervisor binary re-entered as a refill. Its pool comes from the
/// environment above; it prints one JSON summary line on stdout.
pub(crate) const REFILL_ARG: &str = "--refill-rootfs-pool";
const REFILL_SCHEMA: &str = "elastos.browser.vm-rootfs-pool-refill/v1";
const MAX_POOL_COUNT: u32 = 8;
/// Free space a refill leaves for update staging, as the earlier pool kept.
const REFILL_RESERVE_BYTES: u64 = 4096 * 1024 * 1024;
const REFILL_LOCK: &str = ".refill.lock";
const REFILL_LOG: &str = "refill.log";
const ANCHOR_POLL: Duration = Duration::from_millis(500);
const COPY_STOP_GRACE: Duration = Duration::from_secs(2);
/// A launch's busy-probe holds the lock shared for an instant; a refill
/// retries before it reports another refill.
const LOCK_ATTEMPTS: u32 = 4;
const LOCK_RETRY: Duration = Duration::from_millis(50);
const WAIT_POLL: Duration = Duration::from_secs(1);
/// A running copy's rate is estimated only after this much of it.
const MIN_ESTIMATE: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub(crate) struct Pool {
    dir: PathBuf,
    seed: PathBuf,
    manifest: PathBuf,
    image_sha256: String,
    seed_size: u64,
    count: u32,
    reserve_bytes: u64,
}

/// One launch's attempt: the copy it moved into its session, if any, and the
/// pool entries it would not boot.
#[derive(Debug, Default)]
pub(crate) struct Take {
    pub(crate) taken: Option<PathBuf>,
    pub(crate) refused: Vec<(String, &'static str)>,
    /// How long the launch waited for a running refill's copy.
    pub(crate) waited: Duration,
}

/// A launch that found no ready copy while a refill was copying one.
#[derive(Debug)]
pub(crate) enum Waited {
    /// A copy was moved in, or no refill is running and the caller copies.
    Done(Take),
    /// The running refill cannot finish before the launch deadline: its
    /// estimated time left, or `None` while its rate is not known yet.
    GettingReady(Option<Duration>),
    Cancelled,
}

/// What one refill did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refilled {
    /// Another refill owns the pool.
    Busy,
    /// The pool holds its target; `created` copies were made now.
    Full { created: usize },
    /// Too little free space for another copy.
    LowSpace {
        created: usize,
        available: u64,
        required: u64,
    },
}

impl Refilled {
    fn summary(&self) -> Value {
        match self {
            Self::Busy => json!({"state": "busy"}),
            Self::Full { created } => json!({"state": "ready", "created": created}),
            Self::LowSpace {
                created,
                available,
                required,
            } => json!({
                "state": "low_space",
                "created": created,
                "available_bytes": available,
                "required_bytes": required,
            }),
        }
    }
}

impl Pool {
    /// The pool Runtime configured for this seed; `None` when there is none.
    pub(crate) fn from_env(seed: &Path, data_dir: &Path) -> Result<Option<Self>, String> {
        let Some(dir) = std::env::var_os(POOL_DIR_ENV).filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        let count = match std::env::var(POOL_COUNT_ENV) {
            Ok(value) => parse_count(&value)?,
            Err(_) => 1,
        };
        let manifest = std::env::var_os(MANIFEST_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("browser-vm/browser-vm-rootfs-manifest.json"));
        Self::new(PathBuf::from(dir), seed.to_path_buf(), manifest, count).map(Some)
    }

    pub(crate) fn new(
        dir: PathBuf,
        seed: PathBuf,
        manifest: PathBuf,
        count: u32,
    ) -> Result<Self, String> {
        for (label, path) in [
            ("pool directory", &dir),
            ("seed", &seed),
            ("manifest", &manifest),
        ] {
            if !path.is_absolute() {
                return Err(format!(
                    "Browser rootfs {label} must be an absolute path: {}",
                    path.display()
                ));
            }
        }
        if !(1..=MAX_POOL_COUNT).contains(&count) {
            return Err(format!(
                "{POOL_COUNT_ENV} must be an integer from 1 to {MAX_POOL_COUNT}"
            ));
        }
        let (image_sha256, seed_size) = admitted_image(&manifest, &seed)?;
        Ok(Self {
            dir,
            seed,
            manifest,
            image_sha256,
            seed_size,
            count,
            reserve_bytes: REFILL_RESERVE_BYTES,
        })
    }

    /// Moves one prepared copy of the current image into `launch_rootfs`.
    /// Copies of another image, links, shared or foreign files, copies of
    /// another size and unfinished copies are never used.
    pub(crate) fn take(&self, launch_rootfs: &Path) -> Take {
        let mut result = Take::default();
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return result;
        };
        let mut names = Vec::new();
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            match copy_image(&name).map(|image| image == self.image_sha256) {
                Some(true) => names.push(name),
                Some(false) => result.refused.push((name, "copy of another image")),
                None => {}
            }
        }
        names.sort();
        // SAFETY: geteuid has no preconditions.
        let euid = unsafe { libc::geteuid() };
        for name in names {
            let candidate = self.dir.join(&name);
            let Ok(metadata) = fs::symlink_metadata(&candidate) else {
                continue;
            };
            if let Err(reason) = self.usable(&metadata, euid) {
                result.refused.push((name, reason));
                continue;
            }
            match fs::rename(&candidate, launch_rootfs) {
                Ok(()) => {}
                // Another launch took it first.
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(_) => {
                    result
                        .refused
                        .push((name, "could not move into the launch"));
                    continue;
                }
            }
            if same_file(launch_rootfs, &metadata) {
                result.taken = Some(candidate);
                return result;
            }
            // Replaced between the check and the move: never boot it.
            let _ = fs::remove_file(launch_rootfs);
            result.refused.push((name, "changed while taken"));
        }
        result
    }

    fn usable(&self, metadata: &Metadata, euid: u32) -> Result<(), &'static str> {
        if !metadata.file_type().is_file() {
            return Err("not a regular file");
        }
        if metadata.uid() != euid {
            return Err("not owned by this user");
        }
        if metadata.nlink() != 1 {
            return Err("linked elsewhere");
        }
        if metadata.mode() & 0o077 != 0 {
            return Err("open to other users");
        }
        if metadata.len() != self.seed_size {
            return Err("size differs from the image");
        }
        Ok(())
    }

    fn ready_count(&self) -> usize {
        fs::read_dir(&self.dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| {
                        entry
                            .file_name()
                            .to_str()
                            .and_then(copy_image)
                            .is_some_and(|image| image == self.image_sha256)
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    /// Takes a ready copy. When none is ready but a refill is copying one,
    /// waits for that copy while its estimate finishes before `deadline`.
    pub(crate) fn take_or_wait(
        &self,
        launch_rootfs: &Path,
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
    ) -> Waited {
        let first = self.take(launch_rootfs);
        if first.taken.is_some() || !self.refill_running() {
            return Waited::Done(first);
        }
        let started = Instant::now();
        let refused = first.refused;
        loop {
            let now = Instant::now();
            match self.refill_remaining() {
                Some(left) if now + left > deadline => return Waited::GettingReady(Some(left)),
                None if now >= deadline => return Waited::GettingReady(None),
                _ => {}
            }
            if cancelled() {
                return Waited::Cancelled;
            }
            thread::sleep(WAIT_POLL.min(deadline.saturating_duration_since(now)));
            let mut take = self.take(launch_rootfs);
            if take.taken.is_none() && !self.refill_running() {
                // The copy may have been renamed just before its refill ended.
                take = self.take(launch_rootfs);
            }
            if take.taken.is_some() || !self.refill_running() {
                return Waited::Done(Take {
                    taken: take.taken,
                    refused,
                    waited: started.elapsed(),
                });
            }
        }
    }

    /// Whether a refill holds the pool lock. The probe holds the lock shared
    /// only while it asks; a starting refill retries past it.
    fn refill_running(&self) -> bool {
        let Ok(lock) = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.dir.join(REFILL_LOCK))
        else {
            return false;
        };
        // SAFETY: a valid descriptor; closing it releases a granted lock.
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) != 0 }
    }

    /// Estimated time until the running refill's copy is ready, from how far
    /// its copier has read the seed; `None` until a rate is known.
    fn refill_remaining(&self) -> Option<Duration> {
        let log = fs::read_to_string(self.dir.join(REFILL_LOG)).ok()?;
        let mut copy = None;
        for line in log.lines() {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            match value["event"].as_str() {
                Some("copy_start") => copy = Some(value),
                Some("discarded" | "kept") | None => {}
                Some(_) => copy = None,
            }
        }
        let copy = copy?;
        let elapsed = u64::try_from(unix_ms())
            .ok()?
            .checked_sub(copy["at_unix_ms"].as_u64()?)?;
        if u128::from(elapsed) < MIN_ESTIMATE.as_millis() {
            return None;
        }
        let copied = copy["copier_pid"]
            .as_i64()
            .and_then(|pid| i32::try_from(pid).ok())
            .and_then(|pid| self.copier_position(pid))
            .or_else(|| {
                copy["partial"]
                    .as_str()
                    .and_then(|partial| fs::metadata(partial).ok())
                    .map(|metadata| metadata.len())
            })?
            .min(self.seed_size);
        if copied == 0 {
            return None;
        }
        let left = u128::from(elapsed) * u128::from(self.seed_size - copied) / u128::from(copied);
        Some(Duration::from_millis(
            u64::try_from(left).unwrap_or(u64::MAX),
        ))
    }

    /// The copier's read offset in the seed, from its open descriptor.
    fn copier_position(&self, pid: i32) -> Option<u64> {
        let seed = fs::canonicalize(&self.seed).ok()?;
        for entry in fs::read_dir(format!("/proc/{pid}/fd")).ok()?.flatten() {
            if fs::read_link(entry.path()).ok().as_ref() != Some(&seed) {
                continue;
            }
            let fd = entry.file_name();
            let info =
                fs::read_to_string(format!("/proc/{pid}/fdinfo/{}", fd.to_string_lossy())).ok()?;
            return info
                .lines()
                .find_map(|line| line.strip_prefix("pos:"))
                .and_then(|pos| pos.trim().parse().ok());
        }
        None
    }

    /// Starts the background refill when fewer than `count` copies are ready.
    /// It outlives this launch and stops with this process's parent, the
    /// Browser control service, so Home stop ends it.
    pub(crate) fn start_refill(&self) -> Result<Option<u32>, String> {
        if self.ready_count() >= self.count as usize {
            return Ok(None);
        }
        let program = std::env::current_exe().map_err(|error| error.to_string())?;
        // SAFETY: getppid has no preconditions.
        let anchor = unsafe { libc::getppid() };
        let child = Command::new(program)
            .arg(REFILL_ARG)
            // The launch secrets stay with the launch: only the pool's
            // inputs, and no private stdin.
            .env_clear()
            .env(POOL_DIR_ENV, &self.dir)
            .env(ROOTFS_ENV, &self.seed)
            .env(MANIFEST_ENV, &self.manifest)
            .env(POOL_COUNT_ENV, self.count.to_string())
            .env(ANCHOR_ENV, anchor.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("Browser rootfs pool refill did not start: {error}"))?;
        let pid = child.id();
        // Reaped here if it ends during this launch; afterwards by init.
        thread::spawn(move || {
            let mut child = child;
            let _ = child.wait();
        });
        Ok(Some(pid))
    }

    /// Copies the seed until `count` copies are ready. One refill runs at a
    /// time; its events go to refill.log in the pool directory.
    pub(crate) fn refill(&self, anchor: &Anchor) -> Result<Refilled, String> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)
            .map_err(|error| format!("Browser rootfs pool directory: {error}"))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.dir.join(REFILL_LOCK))
            .map_err(|error| format!("Browser rootfs pool lock: {error}"))?;
        let mut attempts = 0;
        // SAFETY: the descriptor stays open for the whole refill.
        while unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            attempts += 1;
            if attempts == LOCK_ATTEMPTS {
                return Ok(Refilled::Busy);
            }
            thread::sleep(LOCK_RETRY);
        }
        let mut log = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.dir.join(REFILL_LOG))
            .map_err(|error| format!("Browser rootfs pool log: {error}"))?;
        let outcome = self.refill_locked(anchor, &mut log);
        match &outcome {
            Ok(refilled) => event(
                &mut log,
                json!({"event": "refill_done", "summary": refilled.summary()}),
            ),
            Err(error) => event(&mut log, json!({"event": "refill_stopped", "error": error})),
        }
        outcome
    }

    fn refill_locked(&self, anchor: &Anchor, log: &mut File) -> Result<Refilled, String> {
        self.discard_stale(log);
        let mut created = 0;
        while self.ready_count() < self.count as usize {
            let available = available_bytes(&self.dir)?;
            let required = self.seed_size.saturating_add(self.reserve_bytes);
            if available < required {
                event(
                    log,
                    json!({"event": "refill_skipped", "available_bytes": available, "required_bytes": required}),
                );
                return Ok(Refilled::LowSpace {
                    created,
                    available,
                    required,
                });
            }
            let before = seed_identity(&self.seed)?;
            let id = format!(
                "{:x}-{}",
                unix_ms(),
                &uuid::Uuid::new_v4().simple().to_string()[..8]
            );
            let ready = self
                .dir
                .join(format!("rootfs-{}-{id}.ext4", self.image_sha256));
            let partial = self
                .dir
                .join(format!("rootfs-{}-{id}.ext4.partial", self.image_sha256));
            let started = Instant::now();
            let copied = copy_while_anchored(&self.seed, &partial, anchor, log).and_then(|()| {
                // An update or Undo during the copy discards it.
                let (image, _) = admitted_image(&self.manifest, &self.seed)?;
                if image != self.image_sha256 || seed_identity(&self.seed)? != before {
                    return Err("Browser image changed during the rootfs pool copy".to_string());
                }
                fs::set_permissions(&partial, fs::Permissions::from_mode(0o600))
                    .and_then(|()| fs::rename(&partial, &ready))
                    .map_err(|error| error.to_string())
            });
            if let Err(error) = copied {
                let _ = fs::remove_file(&partial);
                return Err(error);
            }
            created += 1;
            event(
                log,
                json!({
                    "event": "copy_ready",
                    "ready": ready.display().to_string(),
                    "elapsed_ms": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                }),
            );
        }
        Ok(Refilled::Full { created })
    }

    /// Copies of another image and unfinished copies free their space. Only
    /// this user's regular files are removed; anything else stays.
    fn discard_stale(&self, log: &mut File) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        // SAFETY: geteuid has no preconditions.
        let euid = unsafe { libc::geteuid() };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let stale = match copy_image(&name) {
                Some(image) => image != self.image_sha256,
                None => name.starts_with("rootfs-") && name.ends_with(".ext4.partial"),
            };
            if !stale {
                continue;
            }
            let path = self.dir.join(&name);
            let owned = fs::symlink_metadata(&path)
                .is_ok_and(|metadata| metadata.file_type().is_file() && metadata.uid() == euid);
            if owned && fs::remove_file(&path).is_ok() {
                event(log, json!({"event": "discarded", "name": name}));
            } else {
                event(log, json!({"event": "kept", "name": name}));
            }
        }
    }
}

/// The refill process, `REFILL_ARG`, for the pool its environment names.
/// `seed` and `data_dir` resolve as for a launch. Prints one JSON summary.
pub(crate) fn refill_main(seed: &Path, data_dir: &Path) -> i32 {
    let started = Instant::now();
    let outcome = Pool::from_env(seed, data_dir).and_then(|pool| {
        let Some(pool) = pool else {
            return Ok(json!({"state": "off"}));
        };
        let anchor = match std::env::var(ANCHOR_ENV) {
            Ok(pid) => pid
                .parse::<i32>()
                .map_err(|_| "Browser rootfs pool anchor is invalid".to_string())?,
            // SAFETY: getppid has no preconditions.
            Err(_) => unsafe { libc::getppid() },
        };
        let mut summary = pool.refill(&Anchor::of(anchor)?)?.summary();
        summary["ready"] = json!(pool.ready_count());
        Ok(summary)
    });
    let failed = outcome.is_err();
    let mut summary = outcome.unwrap_or_else(|error| json!({"state": "failed", "error": error}));
    summary["schema"] = json!(REFILL_SCHEMA);
    summary["elapsed_ms"] = json!(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
    // The caller may be gone; the pool state is on disk either way.
    let _ = writeln!(std::io::stdout().lock(), "{summary}");
    i32::from(failed)
}

/// The process whose life bounds a refill, identified by pid and start time
/// so a reused pid never keeps a refill alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Anchor {
    pid: i32,
    start: u64,
}

impl Anchor {
    pub(crate) fn of(pid: i32) -> Result<Self, String> {
        if pid <= 1 {
            return Err("Browser rootfs pool anchor is invalid".to_string());
        }
        let start = process_start(pid)
            .ok_or_else(|| "Browser rootfs pool anchor is not running".to_string())?;
        Ok(Self { pid, start })
    }

    fn alive(&self) -> bool {
        process_start(self.pid) == Some(self.start)
    }
}

/// Field 22 (start time) of a live process; `None` once it exited.
fn process_start(pid: i32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name in parentheses may hold spaces; fields follow it.
    let mut fields = stat.get(stat.rfind(')')? + 1..)?.split_whitespace();
    if matches!(fields.next()?, "Z" | "X" | "x") {
        return None;
    }
    fields.nth(18)?.parse().ok()
}

fn copy_while_anchored(
    seed: &Path,
    partial: &Path,
    anchor: &Anchor,
    log: &mut File,
) -> Result<(), String> {
    // The control service at Home start or launch, or setup.
    let stopped = || "the process that owns this rootfs pool copy stopped".to_string();
    if !anchor.alive() {
        return Err(stopped());
    }
    let mut child = Command::new("/bin/cp")
        .args(["--reflink=auto", "--sparse=always", "--"])
        .arg(seed)
        .arg(partial)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Browser rootfs pool copy did not start: {error}"))?;
    // A waiting launch estimates the time left from the copier's offset.
    event(
        log,
        json!({"event": "copy_start", "partial": partial.display().to_string(), "copier_pid": child.id()}),
    );
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("Browser rootfs pool copy failed: {status}")),
            Ok(None) => {}
            Err(error) => {
                stop_child(&mut child);
                return Err(error.to_string());
            }
        }
        if !anchor.alive() {
            stop_child(&mut child);
            return Err(stopped());
        }
        thread::sleep(ANCHOR_POLL);
    }
}

fn stop_child(child: &mut Child) {
    if let Ok(pid) = i32::try_from(child.id()) {
        // SAFETY: the pid is this process's unreaped child.
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
    let deadline = Instant::now() + COPY_STOP_GRACE;
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The admitted rootfs hash and size the image manifest records for `seed`.
fn admitted_image(manifest: &Path, seed: &Path) -> Result<(String, u64), String> {
    let bytes = fs::read(manifest).map_err(|error| {
        format!(
            "Browser rootfs manifest is unreadable: {}: {error}",
            manifest.display()
        )
    })?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| format!("Browser rootfs manifest is invalid: {}", manifest.display()))?;
    let image = value
        .get("sha256")
        .and_then(Value::as_str)
        .filter(|image| is_sha256(image))
        .ok_or_else(|| {
            format!(
                "Browser rootfs manifest has no image hash: {}",
                manifest.display()
            )
        })?;
    let size = value.get("size").and_then(Value::as_u64);
    let seed_size = fs::metadata(seed)
        .map_err(|error| format!("Browser rootfs seed: {}: {error}", seed.display()))?
        .len();
    if size != Some(seed_size) {
        return Err(format!(
            "Browser rootfs manifest does not describe {}",
            seed.display()
        ));
    }
    Ok((image.to_string(), seed_size))
}

/// The image hash in a ready copy's name, `rootfs-<sha256>-<id>.ext4`.
fn copy_image(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("rootfs-")?.strip_suffix(".ext4")?;
    let (image, id) = rest.split_at_checked(64)?;
    let id = id.strip_prefix('-')?;
    (is_sha256(image)
        && !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')))
    .then_some(image)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn parse_count(value: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .ok()
        .filter(|count| (1..=MAX_POOL_COUNT).contains(count))
        .ok_or_else(|| format!("{POOL_COUNT_ENV} must be an integer from 1 to {MAX_POOL_COUNT}"))
}

fn same_file(path: &Path, expected: &Metadata) -> bool {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .and_then(|file| file.metadata())
        .is_ok_and(|metadata| {
            metadata.is_file()
                && metadata.dev() == expected.dev()
                && metadata.ino() == expected.ino()
                && metadata.nlink() == 1
        })
}

fn seed_identity(seed: &Path) -> Result<(u64, u64, u64, i64, i64), String> {
    let metadata = fs::metadata(seed).map_err(|error| error.to_string())?;
    Ok((
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
    ))
}

fn available_bytes(path: &Path) -> Result<u64, String> {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "Browser rootfs pool path is invalid".to_string())?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: a valid C path and a writable statvfs buffer.
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: statvfs succeeded and filled the buffer.
    let stat = unsafe { stat.assume_init() };
    Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default()
}

fn event(log: &mut File, mut value: Value) {
    value["at_unix_ms"] = json!(u64::try_from(unix_ms()).unwrap_or(u64::MAX));
    let _ = writeln!(log, "{value}");
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMAGE: &str = "c838a0e0d1fad60ca23ece17b8e69f9bedd09e88d37a68bb7dcacff56087f947";
    const OTHER: &str = "019daa8c62af028dae99ef084562e308ba3e7a8a0091fe3563ea286411fc9d2b";

    struct Fixture {
        _tmp: tempfile::TempDir,
        pool: Pool,
        seed: PathBuf,
        launch: PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let seed = tmp.path().join("rootfs.ext4");
        fs::write(&seed, b"admitted seed bytes").unwrap();
        fs::set_permissions(&seed, fs::Permissions::from_mode(0o444)).unwrap();
        let manifest = tmp.path().join("browser-vm-rootfs-manifest.json");
        fs::write(&manifest, json!({"sha256": IMAGE, "size": 19}).to_string()).unwrap();
        let dir = tmp.path().join("rootfs-pool");
        fs::create_dir(&dir).unwrap();
        fs::create_dir(tmp.path().join("session")).unwrap();
        let mut pool = Pool::new(dir, seed.clone(), manifest, 1).unwrap();
        pool.reserve_bytes = 0;
        Fixture {
            launch: tmp.path().join("session/rootfs.ext4"),
            _tmp: tmp,
            pool,
            seed,
        }
    }

    fn entry(pool: &Pool, name: &str, bytes: &[u8], mode: u32) -> PathBuf {
        let path = pool.dir.join(name);
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    #[test]
    fn take_moves_a_ready_copy_of_the_current_image() {
        let f = fixture();
        let ready = entry(
            &f.pool,
            &format!("rootfs-{IMAGE}-a.ext4"),
            b"pooled copy bytes!!",
            0o600,
        );
        let take = f.pool.take(&f.launch);
        assert_eq!(take.taken, Some(ready.clone()));
        assert!(take.refused.is_empty(), "{:?}", take.refused);
        assert!(!ready.exists());
        assert_eq!(fs::read(&f.launch).unwrap(), b"pooled copy bytes!!");
        assert_eq!(fs::read(&f.seed).unwrap(), b"admitted seed bytes");
    }

    #[test]
    fn take_refuses_stale_and_foreign_entries() {
        let f = fixture();
        let names = [
            (format!("rootfs-{OTHER}-a.ext4"), "copy of another image"),
            (format!("rootfs-{IMAGE}-b.ext4"), "not a regular file"),
            (format!("rootfs-{IMAGE}-c.ext4"), "linked elsewhere"),
            (
                format!("rootfs-{IMAGE}-d.ext4"),
                "size differs from the image",
            ),
            (format!("rootfs-{IMAGE}-e.ext4"), "open to other users"),
        ];
        entry(&f.pool, &names[0].0, b"other image copy!!!", 0o600);
        std::os::unix::fs::symlink(&f.seed, f.pool.dir.join(&names[1].0)).unwrap();
        let linked = entry(&f.pool, &names[2].0, b"hard linked copy!!!", 0o600);
        fs::hard_link(&linked, f.pool.dir.parent().unwrap().join("other-link")).unwrap();
        entry(&f.pool, &names[3].0, b"short", 0o600);
        entry(&f.pool, &names[4].0, b"group readable copy", 0o640);
        entry(
            &f.pool,
            &format!("rootfs-{IMAGE}-f.ext4.partial"),
            b"unfinished copy!!!!",
            0o600,
        );

        let take = f.pool.take(&f.launch);

        assert_eq!(take.taken, None);
        let mut refused = take.refused.clone();
        refused.sort();
        let mut expected: Vec<_> = names
            .iter()
            .map(|(name, reason)| (name.clone(), *reason))
            .collect();
        expected.sort();
        assert_eq!(refused, expected);
        assert!(!f.launch.exists());
        assert_eq!(fs::read_dir(&f.pool.dir).unwrap().count(), 6);
        assert_eq!(fs::read(&f.seed).unwrap(), b"admitted seed bytes");
    }

    #[test]
    fn refill_makes_one_private_copy_and_discards_stale_entries() {
        let f = fixture();
        let other = entry(
            &f.pool,
            &format!("rootfs-{OTHER}-a.ext4"),
            b"other image copy!!!",
            0o600,
        );
        let partial = entry(
            &f.pool,
            &format!("rootfs-{IMAGE}-b.ext4.partial"),
            b"unfinished",
            0o600,
        );
        let anchor = Anchor::of(i32::try_from(std::process::id()).unwrap()).unwrap();

        assert_eq!(
            f.pool.refill(&anchor).unwrap(),
            Refilled::Full { created: 1 }
        );
        let log = fs::read_to_string(f.pool.dir.join(REFILL_LOG)).unwrap();
        assert!(log.contains("\"copy_ready\""), "{log}");
        assert_eq!(
            f.pool.refill(&anchor).unwrap(),
            Refilled::Full { created: 0 },
            "a full pool copies nothing"
        );

        assert!(!other.exists() && !partial.exists());
        let ready: Vec<_> = fs::read_dir(&f.pool.dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_str().and_then(copy_image) == Some(IMAGE))
            .map(|entry| entry.path())
            .collect();
        assert_eq!(ready.len(), 1);
        assert_eq!(fs::read(&ready[0]).unwrap(), b"admitted seed bytes");
        assert_eq!(fs::metadata(&ready[0]).unwrap().mode() & 0o777, 0o600);
        assert_eq!(f.pool.take(&f.launch).taken, Some(ready[0].clone()));
    }

    #[test]
    fn refill_stops_without_its_control_service() {
        let f = fixture();
        let mut anchor = Anchor::of(i32::try_from(std::process::id()).unwrap()).unwrap();
        anchor.start += 1; // The same pid, another process.

        let error = f.pool.refill(&anchor).unwrap_err();

        assert!(
            error.contains("owns this rootfs pool copy stopped"),
            "{error}"
        );
        let left: Vec<_> = fs::read_dir(&f.pool.dir)
            .unwrap()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with("rootfs-"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    /// Holds the pool lock as a running refill does.
    fn hold_refill_lock(pool: &Pool) -> File {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(pool.dir.join(REFILL_LOCK))
            .unwrap();
        // SAFETY: a valid descriptor owned by the test.
        assert_eq!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        lock
    }

    /// A refill's log and unfinished copy, started `ago_ms` before now.
    fn running_copy(pool: &Pool, ago_ms: u64, copied: &[u8]) -> PathBuf {
        let partial = entry(
            pool,
            &format!("rootfs-{IMAGE}-run.ext4.partial"),
            copied,
            0o600,
        );
        let started = u64::try_from(unix_ms()).unwrap() - ago_ms;
        fs::write(
            pool.dir.join(REFILL_LOG),
            format!(
                "{}\n",
                json!({"event": "copy_start", "partial": partial, "at_unix_ms": started})
            ),
        )
        .unwrap();
        partial
    }

    #[test]
    fn a_second_refill_reports_the_running_one() {
        let f = fixture();
        let _running = hold_refill_lock(&f.pool);
        let anchor = Anchor::of(i32::try_from(std::process::id()).unwrap()).unwrap();

        assert_eq!(f.pool.refill(&anchor).unwrap(), Refilled::Busy);
        assert_eq!(f.pool.ready_count(), 0);
    }

    #[test]
    fn a_launch_takes_the_copy_a_running_refill_finishes() {
        let f = fixture();
        let running = hold_refill_lock(&f.pool);
        // About half copied in 4 s: some 4 s left, well inside the deadline.
        let partial = running_copy(&f.pool, 4000, b"admitted s");
        let ready = f.pool.dir.join(format!("rootfs-{IMAGE}-run.ext4"));
        let finish = {
            let ready = ready.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(300));
                fs::write(&partial, b"admitted seed bytes").unwrap();
                fs::rename(&partial, &ready).unwrap();
                drop(running);
            })
        };

        let started = Instant::now();
        let Waited::Done(take) =
            f.pool
                .take_or_wait(&f.launch, started + Duration::from_secs(30), &|| false)
        else {
            panic!("the launch did not wait for the running copy");
        };
        finish.join().unwrap();

        assert_eq!(take.taken, Some(ready.clone()));
        assert!(
            take.waited >= Duration::from_millis(300),
            "{:?}",
            take.waited
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(fs::read(&f.launch).unwrap(), b"admitted seed bytes");
        assert!(!ready.exists());
    }

    #[test]
    fn a_launch_fails_fast_when_the_running_refill_cannot_finish_in_time() {
        let f = fixture();
        let _running = hold_refill_lock(&f.pool);
        // 2 of 19 bytes in 10 s: some 85 s left, past a 5 s deadline.
        running_copy(&f.pool, 10_000, b"ad");

        let started = Instant::now();
        let waited = f
            .pool
            .take_or_wait(&f.launch, started + Duration::from_secs(5), &|| false);

        let Waited::GettingReady(Some(left)) = &waited else {
            panic!("expected getting ready, got {waited:?}");
        };
        assert!(
            (Duration::from_secs(80)..Duration::from_secs(90)).contains(left),
            "{left:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!f.launch.exists());
    }

    #[test]
    fn a_launch_waits_only_for_a_running_refill() {
        let f = fixture();
        // A stale log and unfinished copy without a refill holding the pool.
        running_copy(&f.pool, 4000, b"admitted s");

        let started = Instant::now();
        let waited = f
            .pool
            .take_or_wait(&f.launch, started + Duration::from_secs(30), &|| false);

        assert!(
            matches!(&waited, Waited::Done(take) if take.taken.is_none()),
            "{waited:?}"
        );
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn pool_requires_a_manifest_that_describes_the_seed() {
        let f = fixture();
        fs::write(
            &f.pool.manifest,
            json!({"sha256": IMAGE, "size": 20}).to_string(),
        )
        .unwrap();
        let error = Pool::new(
            f.pool.dir.clone(),
            f.seed.clone(),
            f.pool.manifest.clone(),
            1,
        )
        .unwrap_err();
        assert!(error.contains("does not describe"), "{error}");
        assert!(parse_count("0").is_err() && parse_count("9").is_err());
        assert_eq!(parse_count("1"), Ok(1));
    }
}
