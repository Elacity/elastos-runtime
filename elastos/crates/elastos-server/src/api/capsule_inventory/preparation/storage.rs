use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{ensure, Context as _};

use super::PreparationInventory;

const DIRECTORY: &CStr = c"model-preparation";
const LOCK: &CStr = c"lock";
const STATE: &CStr = c"state.json";
const NEXT: &CStr = c"state.next";
const MAX_STATE_BYTES: u64 = 256 * 1024;

#[derive(PartialEq)]
pub(super) struct Stamp(u64, u64, u64, i64, i64, i64, i64);

fn stamp(meta: &Metadata) -> Stamp {
    Stamp(
        meta.dev(),
        meta.ino(),
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec(),
    )
}

fn same_inode(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev() && a.ino() == b.ino()
}

fn check_directory(meta: &Metadata, private: bool) -> anyhow::Result<()> {
    let mask = if private { 0o7077 } else { 0o7022 };
    ensure!(
        meta.is_dir() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & mask == 0,
        "unsafe preparation directory"
    );
    Ok(())
}

fn check_file(meta: &Metadata, limit: u64) -> anyhow::Result<()> {
    ensure!(
        meta.is_file()
            && meta.uid() == unsafe { libc::geteuid() }
            && meta.mode() & 0o7777 == 0o600
            && meta.nlink() == 1
            && meta.len() <= limit,
        "unsafe preparation file"
    );
    Ok(())
}

fn open_at(dir: &File, name: &CStr, flags: i32) -> std::io::Result<File> {
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn optional_file(dir: &File, name: &CStr) -> anyhow::Result<Option<File>> {
    match open_at(dir, name, libc::O_RDONLY) {
        Ok(file) => {
            check_file(&file.metadata()?, MAX_STATE_BYTES)?;
            Ok(Some(file))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn unlink_at(dir: &File, name: &CStr) -> anyhow::Result<()> {
    if unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

// This lock protects exactly one bounded inventory snapshot. Separate opens
// contend across both processes and threads; dropping the fd releases flock.
pub(super) struct Inventory {
    data_path: PathBuf,
    data: File,
    dir: File,
    lock: File,
    loaded: RefCell<Option<Option<Stamp>>>,
}

impl Inventory {
    pub(super) fn open(data_path: &Path, create: bool) -> anyhow::Result<Self> {
        let data = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(data_path)?;
        check_directory(&data.metadata()?, false)?;
        ensure!(
            same_inode(&data.metadata()?, &std::fs::symlink_metadata(data_path)?),
            "preparation root changed"
        );
        if create {
            require_available_space(&data, 0)?;
            if unsafe { libc::mkdirat(data.as_raw_fd(), DIRECTORY.as_ptr(), 0o700) } == 0 {
                data.sync_all()?;
            } else {
                let err = std::io::Error::last_os_error();
                if err.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(err.into());
                }
            }
        }
        let dir = open_at(&data, DIRECTORY, libc::O_RDONLY | libc::O_DIRECTORY)?;
        check_directory(&dir.metadata()?, true)?;
        let lock = open_at(
            &dir,
            LOCK,
            libc::O_RDWR | if create { libc::O_CREAT } else { 0 },
        )?;
        check_file(&lock.metadata()?, 0)?;
        // Snapshot readers and progress writers share this short transaction
        // lock. Ordinary contention must not terminate the preparation worker.
        // Keep the wait bounded; the separate worker lock still rejects a
        // competing preparation immediately.
        let deadline = Instant::now() + Duration::from_secs(1);
        while unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            let remaining = deadline.saturating_duration_since(Instant::now());
            if error.kind() != std::io::ErrorKind::WouldBlock || remaining.is_zero() {
                return Err(error).context("preparation inventory busy");
            }
            std::thread::sleep(remaining.min(Duration::from_millis(5)));
        }
        let result = Self {
            data_path: data_path.into(),
            data,
            dir,
            lock,
            loaded: RefCell::new(None),
        };
        result.revalidate()?;
        // Staging and admitted bytes share this inventory's ownership.
        for (count, entry) in std::fs::read_dir(data_path.join("model-preparation"))?.enumerate() {
            ensure!(
                count < super::MAX_RECORDS + 5,
                "preparation directory exceeds its entry bound"
            );
            let name = entry?.file_name();
            ensure!(
                matches!(
                    name.to_str(),
                    Some("lock" | "worker" | "state.json" | "state.next" | "stage")
                ) || name.to_str().is_some_and(admitted_name),
                "unexpected preparation storage"
            );
        }
        result.revalidate()?;
        Ok(result)
    }

    fn revalidate(&self) -> anyhow::Result<()> {
        let current = std::fs::symlink_metadata(&self.data_path)?;
        check_directory(&current, false)?;
        ensure!(
            same_inode(&current, &self.data.metadata()?),
            "preparation root replaced"
        );
        let dir = open_at(&self.data, DIRECTORY, libc::O_RDONLY | libc::O_DIRECTORY)?;
        check_directory(&dir.metadata()?, true)?;
        ensure!(
            same_inode(&dir.metadata()?, &self.dir.metadata()?),
            "preparation directory replaced"
        );
        let lock = open_at(&self.dir, LOCK, libc::O_RDONLY)?;
        check_file(&lock.metadata()?, 0)?;
        ensure!(
            same_inode(&lock.metadata()?, &self.lock.metadata()?),
            "preparation lock replaced"
        );
        Ok(())
    }

    pub(super) fn snapshot(&self) -> anyhow::Result<PreparationInventory> {
        self.revalidate()?;
        let file = optional_file(&self.dir, STATE)?;
        let (state, original) = if let Some(mut file) = file {
            let before = stamp(&file.metadata()?);
            let mut bytes = Vec::new();
            (&mut file)
                .take(MAX_STATE_BYTES + 1)
                .read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 <= MAX_STATE_BYTES && stamp(&file.metadata()?) == before,
                "preparation state changed during read"
            );
            (
                serde_json::from_slice::<PreparationInventory>(&bytes)?,
                Some(before),
            )
        } else {
            (PreparationInventory::default(), None)
        };
        state.validate()?;
        for entry in std::fs::read_dir(self.data_path.join("model-preparation"))? {
            let name = entry?.file_name();
            if name == "stage" {
                ensure!(
                    state.records.iter().any(|r| r.active()),
                    "unowned preparation stage"
                );
            } else if let Some(id) = name.to_str().and_then(|n| n.strip_prefix("admitted-")) {
                ensure!(
                    state.records.iter().any(|r| r.operation_id == id
                        && matches!(
                            r.state,
                            super::PreparationState::Admitted
                                | super::PreparationState::AdmissionPending
                                | super::PreparationState::Uncertain
                        )),
                    "unaccounted admitted directory"
                );
            }
        }
        *self.loaded.borrow_mut() = Some(original);
        self.check_loaded()?;
        let _ = optional_file(&self.dir, NEXT)?;
        Ok(state)
    }

    pub(super) fn load(&self) -> anyhow::Result<PreparationInventory> {
        let state = self.snapshot()?;
        // A complete or partial uncommitted snapshot never supersedes state.json.
        // There can be only this one bounded temporary file; invalid shape is refused.
        if let Some(file) = optional_file(&self.dir, NEXT)? {
            self.revalidate()?;
            let current =
                optional_file(&self.dir, NEXT)?.context("preparation temporary file changed")?;
            ensure!(
                same_inode(&current.metadata()?, &file.metadata()?),
                "preparation temporary file replaced"
            );
            unlink_at(&self.dir, NEXT)?;
            self.dir.sync_all()?;
        }
        Ok(state)
    }

    fn check_loaded(&self) -> anyhow::Result<()> {
        self.revalidate()?;
        let current = optional_file(&self.dir, STATE)?
            .map(|file| file.metadata().map(|meta| stamp(&meta)))
            .transpose()?;
        ensure!(
            self.loaded.borrow().as_ref() == Some(&current),
            "preparation state replaced"
        );
        Ok(())
    }

    pub(super) fn save(&self, state: &PreparationInventory) -> anyhow::Result<()> {
        state.validate()?;
        let bytes = serde_json::to_vec(state)?;
        ensure!(
            bytes.len() as u64 <= MAX_STATE_BYTES,
            "preparation state exceeds its byte bound"
        );
        self.check_loaded()?;
        let previous = self.snapshot()?;
        for owner in previous.records.iter().filter(|r| r.activation.is_some()) {
            ensure!(
                state
                    .records
                    .iter()
                    .any(|r| r.operation_id == owner.operation_id
                        && r.activation == owner.activation),
                "original activation binding changed"
            );
        }
        if let Some(retirement) = &previous.retirement {
            match &state.retirement {
                Some(next) => ensure!(
                    next.operation_id == retirement.operation_id
                        && next.admission_id == retirement.admission_id
                        && (retirement.phase == super::RetirementPhase::WithdrawalPending
                            || next.phase == super::RetirementPhase::Withdrawn),
                    "retirement target changed"
                ),
                None => ensure!(
                    retirement.phase == super::RetirementPhase::Withdrawn
                        && state
                            .records
                            .iter()
                            .filter(|r| r.admission_id == retirement.admission_id)
                            .all(|r| matches!(
                                r.state,
                                super::PreparationState::Reclaimed
                                    | super::PreparationState::Cancelled
                                    | super::PreparationState::Expired
                                    | super::PreparationState::Failed
                            ) && r.reserved_bytes == 0),
                    "unresolved retirement removed"
                ),
            }
        }
        let mut next = open_at(
            &self.dir,
            NEXT,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )?;
        let result = (|| {
            check_file(&next.metadata()?, MAX_STATE_BYTES)?;
            next.write_all(&bytes)?;
            next.sync_all()?;
            self.check_loaded()?;
            let current =
                optional_file(&self.dir, NEXT)?.context("preparation temporary file missing")?;
            ensure!(
                same_inode(&current.metadata()?, &next.metadata()?),
                "preparation temporary file replaced"
            );
            if unsafe {
                libc::renameat(
                    self.dir.as_raw_fd(),
                    NEXT.as_ptr(),
                    self.dir.as_raw_fd(),
                    STATE.as_ptr(),
                )
            } != 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            *self.loaded.borrow_mut() = Some(Some(stamp(&next.metadata()?)));
            // A sync error after rename is an uncertain durability result, not
            // permission to retry effects. Reopen the exact request to reconcile.
            self.dir
                .sync_all()
                .context("preparation publication durability uncertain")?;
            self.revalidate()?;
            Ok(())
        })();
        if result.is_err() && self.revalidate().is_ok() {
            if let Ok(Some(current)) = optional_file(&self.dir, NEXT) {
                if same_inode(&current.metadata()?, &next.metadata()?) {
                    unlink_at(&self.dir, NEXT)?;
                }
            }
        }
        result
    }

    pub(super) fn require_space(&self, reserved_bytes: u64) -> anyhow::Result<()> {
        require_available_space(&self.dir, reserved_bytes)
    }

    pub(super) fn space_fits(&self, reserved_bytes: u64) -> anyhow::Result<bool> {
        let (capacity, available, required) = space_observation(&self.dir, reserved_bytes)?;
        space_floor_fits(capacity, available, required)
    }

    pub(super) fn volume(&self) -> anyhow::Result<u64> {
        Ok(self.dir.metadata()?.dev())
    }

    // This same-inventory lock outlives short snapshot transactions. Status
    // and cancellation remain available while a worker awaits provider drain.
    pub(super) fn worker_lock(&self) -> anyhow::Result<File> {
        let file = open_at(&self.dir, c"worker", libc::O_RDWR | libc::O_CREAT)?;
        check_file(&file.metadata()?, 0)?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(std::io::Error::last_os_error()).context("preparation worker busy");
        }
        Ok(file)
    }

    pub(super) fn stage(&self, create: bool) -> anyhow::Result<Stage> {
        self.revalidate()?;
        if create {
            if unsafe { libc::mkdirat(self.dir.as_raw_fd(), c"stage".as_ptr(), 0o700) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            self.dir.sync_all()?;
        }
        self.directory(c"stage")
    }

    fn directory(&self, name: &CStr) -> anyhow::Result<Stage> {
        let dir = open_at(&self.dir, name, libc::O_RDONLY | libc::O_DIRECTORY)?;
        let path = self
            .data_path
            .canonicalize()?
            .join("model-preparation")
            .join(name.to_str()?);
        let stage = Stage { dir, path };
        stage.check()?;
        self.revalidate()?;
        Ok(stage)
    }

    pub(super) fn admitted(&self, id: &str) -> anyhow::Result<Stage> {
        let name = format!("admitted-{id}");
        ensure!(admitted_name(&name), "invalid admission identity");
        self.directory(&CString::new(name)?)
    }

    pub(super) fn admit(&self, id: &str) -> anyhow::Result<()> {
        let name = CString::new(format!("admitted-{id}"))?;
        ensure!(admitted_name(name.to_str()?), "invalid admission identity");
        self.stage(false)?.check()?;
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::renameat2(
                self.dir.as_raw_fd(),
                c"stage".as_ptr(),
                self.dir.as_raw_fd(),
                name.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(target_os = "macos")]
        let result = unsafe {
            libc::renameatx_np(
                self.dir.as_raw_fd(),
                c"stage".as_ptr(),
                self.dir.as_raw_fd(),
                name.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let result = -1;
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        self.dir.sync_all()?;
        self.revalidate()
    }

    pub(super) fn remove_stage(&self) -> anyhow::Result<()> {
        match self.stage(false) {
            Ok(stage) => {
                stage.validate_tree(&mut 300)?;
                stage.remove_contents()?;
                stage.check()?;
                if unsafe {
                    libc::unlinkat(self.dir.as_raw_fd(), c"stage".as_ptr(), libc::AT_REMOVEDIR)
                } != 0
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                self.dir.sync_all()?;
                self.revalidate()
            }
            Err(err) if missing(&err) => Ok(()),
            Err(err) => Err(err),
        }
    }

    // Only a durable withdrawn retirement may call this idempotent removal.
    pub(super) fn remove_admitted(&self, id: &str) -> anyhow::Result<()> {
        self.revalidate()?;
        let name = CString::new(format!("admitted-{id}"))?;
        ensure!(admitted_name(name.to_str()?), "invalid admission identity");
        match self.admitted(id) {
            Ok(stage) => {
                stage.validate_tree(&mut 300)?;
                stage.remove_contents()?;
                stage.check()?;
                if unsafe {
                    libc::unlinkat(self.dir.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR)
                } != 0
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                self.dir.sync_all()?;
                self.revalidate()
            }
            Err(error) if missing(&error) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

pub(super) fn missing(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}

fn admitted_name(name: &str) -> bool {
    name.strip_prefix("admitted-").is_some_and(|id| {
        id.len() == 64
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

pub(super) struct Stage {
    dir: File,
    pub(super) path: PathBuf,
}

impl Stage {
    pub(super) fn check(&self) -> anyhow::Result<()> {
        let current = std::fs::symlink_metadata(&self.path)?;
        check_directory(&current, true)?;
        ensure!(
            current.mode() & 0o7777 == 0o700 && same_inode(&current, &self.dir.metadata()?),
            "stage replaced"
        );
        Ok(())
    }

    fn parent(&self, path: &str, create: bool) -> anyhow::Result<(File, CString)> {
        elastos_common::validate_model_content_path(path).map_err(anyhow::Error::msg)?;
        self.check()?;
        let mut dir = self.dir.try_clone()?;
        let parts: Vec<_> = path.split('/').collect();
        for part in &parts[..parts.len() - 1] {
            let name = CString::new(*part)?;
            if create {
                if unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) } != 0
                    && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                dir.sync_all()?;
            }
            dir = open_at(&dir, &name, libc::O_RDONLY | libc::O_DIRECTORY)?;
            check_directory(&dir.metadata()?, true)?;
            ensure!(
                dir.metadata()?.mode() & 0o7777 == 0o700,
                "private stage required"
            );
        }
        Ok((dir, CString::new(parts[parts.len() - 1])?))
    }

    pub(super) fn create_file(&self, path: &str) -> anyhow::Result<File> {
        let (dir, name) = self.parent(path, true)?;
        let file = open_at(&dir, &name, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?;
        check_file(&file.metadata()?, super::MAX_PACKAGE_BYTES)?;
        dir.sync_all()?;
        self.check()?;
        Ok(file)
    }

    pub(super) fn read_index(&self) -> anyhow::Result<Vec<u8>> {
        self.check()?;
        let mut file = open_at(&self.dir, c"_elastos_object.json", libc::O_RDONLY)?;
        check_file(&file.metadata()?, 65536)?;
        let before = stamp(&file.metadata()?);
        let mut bytes = Vec::new();
        (&mut file).take(65537).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= 65536 && stamp(&file.metadata()?) == before,
            "stored index changed"
        );
        self.check_file("_elastos_object.json", &file, bytes.len() as u64)?;
        Ok(bytes)
    }

    pub(super) fn verify_model_file(
        &self,
        expected: &crate::content::ContentObjectFile,
        entrypoint: bool,
    ) -> anyhow::Result<()> {
        use sha2::{Digest as _, Sha256};

        let (dir, name) = self.parent(&expected.path, false)?;
        let mut file = open_at(&dir, &name, libc::O_RDONLY)?;
        self.check_file(&expected.path, &file, expected.size)?;
        let before = stamp(&file.metadata()?);
        let mut digest = Sha256::new();
        let mut read = 0u64;
        let mut buffer = [0u8; 65536];
        if entrypoint {
            file.read_exact(&mut buffer[..8])?;
            ensure!(
                &buffer[..4] == b"GGUF"
                    && matches!(u32::from_le_bytes(buffer[4..8].try_into()?), 2 | 3),
                "invalid admitted GGUF header"
            );
            digest.update(&buffer[..8]);
            read = 8;
        }
        while read < expected.size {
            let limit = (expected.size - read).min(buffer.len() as u64) as usize;
            let count = file.read(&mut buffer[..limit])?;
            ensure!(count > 0, "admitted file is truncated");
            digest.update(&buffer[..count]);
            read += count as u64;
        }
        ensure!(
            read == expected.size
                && hex::encode(digest.finalize()) == expected.sha256
                && stamp(&file.metadata()?) == before,
            "admitted file changed or has an invalid digest"
        );
        self.check_file(&expected.path, &file, expected.size)
    }

    pub(super) fn file_stamp(
        &self,
        expected: &crate::content::ContentObjectFile,
    ) -> anyhow::Result<Stamp> {
        let (dir, name) = self.parent(&expected.path, false)?;
        let file = open_at(&dir, &name, libc::O_RDONLY)?;
        self.check_file(&expected.path, &file, expected.size)?;
        Ok(stamp(&file.metadata()?))
    }

    pub(super) fn check_file(&self, path: &str, file: &File, size: u64) -> anyhow::Result<()> {
        let (dir, name) = self.parent(path, false)?;
        let current = open_at(&dir, &name, libc::O_RDONLY)?.metadata()?;
        check_file(&current, size)?;
        ensure!(
            current.len() == size && same_inode(&current, &file.metadata()?),
            "staged file changed"
        );
        Ok(())
    }

    fn children(&self) -> anyhow::Result<Vec<(CString, Metadata)>> {
        self.check()?;
        let mut result = Vec::new();
        for entry in std::fs::read_dir(&self.path)? {
            ensure!(result.len() < 300, "stage entry bound exceeded");
            let name = CString::new(entry?.file_name().as_encoded_bytes())?;
            result.push((
                name.clone(),
                open_at(&self.dir, &name, libc::O_RDONLY)?.metadata()?,
            ));
        }
        self.check()?;
        Ok(result)
    }

    fn child(&self, name: &CStr) -> anyhow::Result<Self> {
        Ok(Self {
            dir: open_at(&self.dir, name, libc::O_RDONLY | libc::O_DIRECTORY)?,
            path: self.path.join(name.to_str()?),
        })
    }

    fn validate_tree(&self, budget: &mut usize) -> anyhow::Result<()> {
        for (name, meta) in self.children()? {
            *budget = budget.checked_sub(1).context("stage tree bound exceeded")?;
            if meta.is_dir() {
                self.child(&name)?.validate_tree(budget)?;
            } else {
                check_file(&meta, super::MAX_PACKAGE_BYTES)?;
            }
        }
        Ok(())
    }

    fn remove_contents(&self) -> anyhow::Result<()> {
        for (name, meta) in self.children()? {
            if meta.is_dir() {
                let child = self.child(&name)?;
                child.remove_contents()?;
                child.check()?;
                if unsafe {
                    libc::unlinkat(self.dir.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR)
                } != 0
                {
                    return Err(std::io::Error::last_os_error().into());
                }
            } else {
                check_file(&meta, super::MAX_PACKAGE_BYTES)?;
                unlink_at(&self.dir, &name)?;
            }
        }
        self.dir.sync_all()?;
        Ok(())
    }
}

fn require_available_space(dir: &File, reserved_bytes: u64) -> anyhow::Result<()> {
    let (capacity, available, required) = space_observation(dir, reserved_bytes)?;
    require_space_floor(capacity, available, required)
}

fn space_observation(dir: &File, reserved_bytes: u64) -> anyhow::Result<(u128, u128, u128)> {
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::fstatvfs(dir.as_raw_fd(), stats.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stats = unsafe { stats.assume_init() };
    let unit = u128::from(stats.f_frsize);
    let capacity = u128::from(stats.f_blocks)
        .checked_mul(unit)
        .context("disk capacity overflow")?;
    let available = u128::from(stats.f_bavail)
        .checked_mul(unit)
        .context("disk availability overflow")?;
    let required = u128::from(reserved_bytes)
        .checked_add(u128::from(MAX_STATE_BYTES) * 2)
        .and_then(|bytes| bytes.checked_add(unit.checked_mul(4)?))
        .context("preparation disk reservation overflow")?;
    Ok((capacity, available, required))
}

pub(super) fn space_floor_fits(
    capacity: u128,
    available: u128,
    reserved: u128,
) -> anyhow::Result<bool> {
    ensure!(
        capacity > 0 && available <= capacity,
        "invalid preparation disk capacity"
    );
    let Some(remaining) = available.checked_sub(reserved) else {
        return Ok(false);
    };
    Ok(remaining.checked_mul(10).context("disk floor overflow")? >= capacity)
}

pub(super) fn require_space_floor(
    capacity: u128,
    available: u128,
    reserved: u128,
) -> anyhow::Result<()> {
    let fits = space_floor_fits(capacity, available, reserved)?;
    ensure!(available >= reserved, "insufficient preparation space");
    ensure!(
        fits,
        "preparation requires ten percent free space after reservation"
    );
    Ok(())
}
