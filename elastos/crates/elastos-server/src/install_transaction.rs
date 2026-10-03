//! Offline release-file transaction. Destinations retain their installed paths.
//! Installer adoption of this lock is a separate release-owner integration.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use elastos_common::localhost::{publisher_release_head_path, publisher_release_manifest_path};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const INSTALL_LOCK: &str = ".elastos.install.lock";
const JOURNAL: &str = ".elastos.update-journal.json";
const JOURNAL_TMP: &str = ".elastos.update-journal.tmp";
const STAGE: &str = ".elastos.update-stage";
const ROLLBACK: &str = ".elastos.update-rollback";
const MAX_JOURNAL: u64 = 16 * 1024;
const RESERVE_PERCENT: u128 = 15;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReleaseFile {
    RuntimeBinary,
    Components,
    Sources,
    ReleaseHead,
    ReleaseManifest,
}

impl ReleaseFile {
    const ALL: [Self; 5] = [
        Self::RuntimeBinary,
        Self::Components,
        Self::Sources,
        Self::ReleaseHead,
        Self::ReleaseManifest,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::RuntimeBinary => "runtime_binary",
            Self::Components => "components",
            Self::Sources => "sources",
            Self::ReleaseHead => "release_head",
            Self::ReleaseManifest => "release_manifest",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Staging,
    Prepared,
    Committing,
    Committed,
    Recovering,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    id: ReleaseFile,
    original_sha256: Option<String>,
    original_mode: Option<u32>,
    staged_sha256: String,
    staged_mode: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema: String,
    transaction_id: String,
    data_dir: PathBuf,
    binary_basename: String,
    phase: Phase,
    entries: Vec<Entry>,
}

/// Serializes installation writers at an absolute, resolved binary parent.
/// Writers resolve the parent once and use the same path for their destinations.
/// The lock path stays in place after the guard closes its file and releases the flock.
pub(crate) struct InstallationGuard {
    _lock: File,
}

impl InstallationGuard {
    pub(crate) fn acquire(binary_parent: &Path) -> anyhow::Result<Self> {
        if !binary_parent.is_absolute() {
            bail!("installation binary parent must be an absolute existing path");
        }
        check_directory(binary_parent)?;
        let lock_path = binary_parent.join(INSTALL_LOCK);
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&lock_path)
            .context("open installation lock")?;
        check_file(&lock, &lock_path, true)?;
        if unsafe {
            libc::flock(
                std::os::fd::AsRawFd::as_raw_fd(&lock),
                libc::LOCK_EX | libc::LOCK_NB,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error())
                .context("another writer owns the installation lock");
        }
        Ok(Self { _lock: lock })
    }
}

pub(crate) struct InstallTransaction {
    _guard: InstallationGuard,
    data_dir: PathBuf,
    binary: PathBuf,
    destinations: BTreeMap<ReleaseFile, PathBuf>,
}

impl InstallTransaction {
    pub(crate) fn has_pending_recovery(binary: &Path) -> bool {
        binary
            .parent()
            .is_some_and(|parent| fs::symlink_metadata(parent.join(JOURNAL)).is_ok())
    }
    pub(crate) fn acquire(data_dir: &Path, binary: &Path) -> anyhow::Result<Self> {
        if !data_dir.is_absolute() || !binary.is_absolute() {
            bail!("update paths must be absolute installed paths");
        }
        let data_dir = fs::canonicalize(data_dir).context("resolve update data directory")?;
        check_directory(&data_dir)?;
        let bin_parent = fs::canonicalize(binary.parent().context("binary parent missing")?)?;
        check_directory(&bin_parent)?;
        let basename = binary
            .file_name()
            .and_then(|name| name.to_str())
            .context("invalid binary basename")?;
        if basename.starts_with(".elastos.") {
            bail!("binary path conflicts with installation transaction state");
        }
        let binary = bin_parent.join(basename);
        file_state(&binary)?;
        let guard = InstallationGuard::acquire(&bin_parent)?;
        let destinations = BTreeMap::from([
            (ReleaseFile::RuntimeBinary, binary.clone()),
            (ReleaseFile::Components, data_dir.join("components.json")),
            (ReleaseFile::Sources, data_dir.join("sources.json")),
            (
                ReleaseFile::ReleaseHead,
                publisher_release_head_path(&data_dir),
            ),
            (
                ReleaseFile::ReleaseManifest,
                publisher_release_manifest_path(&data_dir),
            ),
        ]);
        if destinations.values().collect::<BTreeSet<_>>().len() != ReleaseFile::ALL.len() {
            bail!("release destinations overlap");
        }
        let tx = Self {
            _guard: guard,
            data_dir,
            binary,
            destinations,
        };
        // Check every destination and parent before the journal can authorize writes.
        for destination in tx.destinations.values() {
            tx.check_parent(destination.parent().unwrap(), false)?;
            file_state(destination)?;
        }
        Ok(tx)
    }

    pub(crate) fn binary_path(&self) -> &Path {
        &self.binary
    }

    pub(crate) fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub(crate) fn staged_binary(&self) -> PathBuf {
        self.scratch(ReleaseFile::RuntimeBinary, STAGE)
    }

    fn journal_path(&self) -> PathBuf {
        self.binary.parent().unwrap().join(JOURNAL)
    }

    fn scratch(&self, id: ReleaseFile, directory: &str) -> PathBuf {
        self.destinations[&id]
            .parent()
            .unwrap()
            .join(directory)
            .join(id.name())
    }

    pub(crate) fn excluded_paths(&self) -> BTreeSet<PathBuf> {
        let mut paths = BTreeSet::from([
            self.binary.clone(),
            self.journal_path(),
            self.binary.parent().unwrap().join(JOURNAL_TMP),
            self.binary.parent().unwrap().join(INSTALL_LOCK),
        ]);
        for parent in self.parents() {
            paths.insert(parent.join(STAGE));
            paths.insert(parent.join(ROLLBACK));
        }
        paths
    }

    fn partial(&self, id: ReleaseFile, directory: &str) -> PathBuf {
        self.scratch(id, directory)
            .with_file_name(format!(".{}.partial", id.name()))
    }

    fn parents(&self) -> BTreeSet<PathBuf> {
        self.destinations
            .values()
            .map(|path| path.parent().unwrap().to_path_buf())
            .collect()
    }

    fn check_parent(&self, parent: &Path, create: bool) -> anyhow::Result<()> {
        if parent == self.binary.parent().unwrap() {
            return check_directory(parent);
        }
        let relative = parent
            .strip_prefix(&self.data_dir)
            .context("release parent escapes data root")?;
        let mut path = self.data_dir.clone();
        for part in relative.components() {
            if !matches!(part, std::path::Component::Normal(_)) {
                bail!("unsafe release parent");
            }
            path.push(part);
            match fs::symlink_metadata(&path) {
                Ok(_) => check_directory(&path)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
                    fs::DirBuilder::new().mode(0o700).create(&path)?;
                    sync_directory(path.parent().unwrap())?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    pub(crate) fn recover(&self) -> anyhow::Result<()> {
        self.recover_inner(false)
    }

    fn recover_inner(&self, rollback_committed: bool) -> anyhow::Result<()> {
        let Some(mut journal) = self.read_journal()? else {
            // Unjournalled scratch belongs to reconciliation, not a new update.
            self.require_empty_scratch()?;
            return Ok(());
        };
        // Every finalized scratch file is hash/mode bound before any release
        // restoration. Partial writes have distinct names and only staging owns them.
        self.validate_scratch(&journal)?;
        if journal.phase == Phase::Staging {
            for entry in &journal.entries {
                self.require_state(
                    entry.id,
                    entry.original_sha256.as_deref(),
                    entry.original_mode,
                )?;
            }
            return self.cleanup(&journal);
        }
        if journal.phase == Phase::Committed && !rollback_committed {
            // Commit already verified the installed set. Cleanup owns only the
            // journal and scratch; later safe owner changes belong to the live set.
            for entry in &journal.entries {
                if file_state(&self.destinations[&entry.id])?.is_none() {
                    bail!(
                        "release file {} is missing; retain journal for recovery",
                        entry.id.name()
                    );
                }
            }
        } else {
            // Validate the complete restore plan before changing a release file.
            for entry in &journal.entries {
                let current = file_state(&self.destinations[&entry.id])?;
                if !state_matches(
                    &current,
                    entry.original_sha256.as_deref(),
                    entry.original_mode,
                ) && !state_matches(
                    &current,
                    Some(&entry.staged_sha256),
                    Some(entry.staged_mode),
                ) {
                    bail!(
                        "release file {} has foreign changes; retain the journal for recovery",
                        entry.id.name()
                    );
                }
                if !state_matches(
                    &current,
                    entry.original_sha256.as_deref(),
                    entry.original_mode,
                ) && entry.original_sha256.is_some()
                {
                    require_hash(
                        &self.scratch(entry.id, ROLLBACK),
                        entry.original_sha256.as_deref().unwrap(),
                    )?;
                }
            }
            journal.phase = Phase::Recovering;
            self.write_journal(&journal)?;
            for entry in &journal.entries {
                let current = file_state(&self.destinations[&entry.id])?;
                if state_matches(
                    &current,
                    entry.original_sha256.as_deref(),
                    entry.original_mode,
                ) {
                    continue;
                }
                if !state_matches(
                    &current,
                    Some(&entry.staged_sha256),
                    Some(entry.staged_mode),
                ) {
                    bail!("release file changed during recovery");
                }
                if let Some(original) = &entry.original_sha256 {
                    let backup = self.scratch(entry.id, ROLLBACK);
                    require_hash(&backup, original)?;
                    fs::rename(&backup, &self.destinations[&entry.id])?;
                } else {
                    fs::remove_file(&self.destinations[&entry.id])?;
                }
                sync_directory(self.destinations[&entry.id].parent().unwrap())?;
                self.require_state(
                    entry.id,
                    entry.original_sha256.as_deref(),
                    entry.original_mode,
                )?;
            }
        }
        self.cleanup(&journal)
    }

    pub(crate) fn prepare(&self, files: &[(ReleaseFile, &[u8])]) -> anyhow::Result<()> {
        self.require_empty_scratch()?;
        if files.len() != ReleaseFile::ALL.len()
            || files.iter().map(|item| item.0).collect::<BTreeSet<_>>()
                != ReleaseFile::ALL.into_iter().collect()
        {
            bail!("release transaction requires exactly five release files");
        }
        let mut entries = Vec::new();
        let mut allocations = BTreeMap::new();
        for &(id, bytes) in files {
            let state = file_state(&self.destinations[&id])?;
            let original_mode = state.as_ref().map(|item| item.1);
            let staged_mode = if id == ReleaseFile::RuntimeBinary {
                0o755
            } else {
                original_mode.unwrap_or(0o600)
            };
            let original_size = state.as_ref().map(|item| item.2).unwrap_or(0);
            allocations.insert(id, bytes.len() as u64 + original_size);
            entries.push(Entry {
                id,
                original_sha256: state.map(|item| item.0),
                original_mode,
                staged_sha256: sha256(bytes),
                staged_mode,
            });
        }
        self.check_disk(&allocations)?;
        for parent in self.parents() {
            self.check_parent(&parent, true)?;
        }
        let mut journal = Journal {
            schema: "elastos.install-transaction/v1".to_string(),
            transaction_id: hex::encode(rand::random::<[u8; 16]>()),
            data_dir: self.data_dir.clone(),
            binary_basename: self
                .binary
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string(),
            phase: Phase::Staging,
            entries,
        };
        self.write_journal(&journal)?;
        let result = (|| {
            for parent in self.parents() {
                for directory in [STAGE, ROLLBACK] {
                    fs::DirBuilder::new()
                        .mode(0o700)
                        .create(parent.join(directory))?;
                }
                sync_directory(&parent)?;
            }
            for entry in &journal.entries {
                let bytes = files.iter().find(|item| item.0 == entry.id).unwrap().1;
                let stage_partial = self.partial(entry.id, STAGE);
                write_new(&stage_partial, bytes, entry.staged_mode)?;
                self.publish_scratch(entry.id, STAGE, &entry.staged_sha256, entry.staged_mode)?;
                if let Some(original) = &entry.original_sha256 {
                    let mut input = open_read(&self.destinations[&entry.id])?;
                    let backup = self.partial(entry.id, ROLLBACK);
                    let mut output = open_new(&backup, entry.original_mode.unwrap())?;
                    let mut buffer = [0; 64 * 1024];
                    loop {
                        let count = input.read(&mut buffer)?;
                        if count == 0 {
                            break;
                        }
                        let (total, available) = disk_space(backup.parent().unwrap())?;
                        require_disk_reserve(total, available, count as u128)?;
                        output.write_all(&buffer[..count])?;
                    }
                    output.sync_all()?;
                    self.publish_scratch(
                        entry.id,
                        ROLLBACK,
                        original,
                        entry.original_mode.unwrap(),
                    )?;
                }
                sync_directory(self.scratch(entry.id, STAGE).parent().unwrap())?;
                sync_directory(self.scratch(entry.id, ROLLBACK).parent().unwrap())?;
                self.require_state(
                    entry.id,
                    entry.original_sha256.as_deref(),
                    entry.original_mode,
                )?;
            }
            journal.phase = Phase::Prepared;
            self.write_journal(&journal)
        })();
        self.restore_on_error(result)
    }

    pub(crate) fn abort(&self) -> anyhow::Result<()> {
        self.recover()
    }

    #[cfg(test)]
    fn commit(&self) -> anyhow::Result<()> {
        self.commit_with(|_| Ok(()), || Ok(()))
    }

    pub(crate) fn commit_checked(
        &self,
        check: impl FnOnce() -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        self.commit_with(|_| Ok(()), check)
    }

    fn commit_with(
        &self,
        mut after_rename: impl FnMut(ReleaseFile) -> anyhow::Result<()>,
        after_activation: impl FnOnce() -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let mut journal = self
            .read_journal()?
            .context("prepared release journal missing")?;
        if journal.phase != Phase::Prepared {
            bail!("release transaction is not prepared");
        }
        self.validate_scratch(&journal)?;
        for entry in &journal.entries {
            check_private_directory(self.scratch(entry.id, STAGE).parent().unwrap())?;
            check_private_directory(self.scratch(entry.id, ROLLBACK).parent().unwrap())?;
            self.require_state(
                entry.id,
                entry.original_sha256.as_deref(),
                entry.original_mode,
            )?;
            require_hash(&self.scratch(entry.id, STAGE), &entry.staged_sha256)?;
            if let Some(original) = &entry.original_sha256 {
                require_hash(&self.scratch(entry.id, ROLLBACK), original)?;
            }
        }
        let result = (|| {
            journal.phase = Phase::Committing;
            self.write_journal(&journal)?;
            for entry in &journal.entries {
                self.check_parent(self.destinations[&entry.id].parent().unwrap(), false)?;
                self.require_state(
                    entry.id,
                    entry.original_sha256.as_deref(),
                    entry.original_mode,
                )?;
                fs::rename(self.scratch(entry.id, STAGE), &self.destinations[&entry.id])?;
                sync_directory(self.destinations[&entry.id].parent().unwrap())?;
                after_rename(entry.id)?;
                self.require_state(
                    entry.id,
                    Some(&entry.staged_sha256),
                    Some(entry.staged_mode),
                )?;
            }
            after_activation()?;
            journal.phase = Phase::Committed;
            self.write_journal(&journal)
        })();
        self.restore_on_error(result)?;
        self.cleanup(&journal)
    }

    fn restore_on_error(&self, result: anyhow::Result<()>) -> anyhow::Result<()> {
        match result {
            Ok(()) => Ok(()),
            Err(error) => match self.recover_inner(true) {
                Ok(()) => {
                    Err(error.context("release transaction refused; previous release restored"))
                }
                Err(recovery) => Err(error.context(format!(
                    "release recovery required; retained journal: {recovery:#}"
                ))),
            },
        }
    }

    fn require_state(
        &self,
        id: ReleaseFile,
        expected: Option<&str>,
        mode: Option<u32>,
    ) -> anyhow::Result<()> {
        let actual = file_state(&self.destinations[&id])?;
        if !state_matches(&actual, expected, mode) {
            bail!("release file {} changed; recovery required", id.name());
        }
        Ok(())
    }

    fn require_empty_scratch(&self) -> anyhow::Result<()> {
        for parent in self.parents() {
            for name in [STAGE, ROLLBACK] {
                if fs::symlink_metadata(parent.join(name)).is_ok() {
                    bail!("unresolved installation scratch requires recovery");
                }
            }
        }
        if fs::symlink_metadata(self.journal_path()).is_ok() {
            bail!("unresolved installation journal requires recovery");
        }
        Ok(())
    }

    fn read_journal(&self) -> anyhow::Result<Option<Journal>> {
        let mut file = match open_read(&self.journal_path()) {
            Ok(file) => file,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(None)
            }
            Err(error) => return Err(error),
        };
        check_file(&file, &self.journal_path(), true)?;
        if file.metadata()?.len() > MAX_JOURNAL {
            bail!("installation journal exceeds size limit");
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let journal: Journal = serde_json::from_slice(&bytes)?;
        if journal.schema != "elastos.install-transaction/v1"
            || journal.data_dir != self.data_dir
            || journal.binary_basename != self.binary.file_name().unwrap().to_str().unwrap()
            || journal.transaction_id.len() != 32
            || !journal
                .transaction_id
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
            || journal.entries.len() != ReleaseFile::ALL.len()
            || journal
                .entries
                .iter()
                .map(|entry| entry.id)
                .collect::<BTreeSet<_>>()
                != ReleaseFile::ALL.into_iter().collect()
        {
            bail!("installation journal identity is incompatible with this writer");
        }
        for entry in &journal.entries {
            if !valid_hash(&entry.staged_sha256)
                || entry
                    .original_sha256
                    .as_ref()
                    .is_some_and(|hash| !valid_hash(hash))
                || entry.original_sha256.is_some() != entry.original_mode.is_some()
                || entry
                    .original_mode
                    .is_some_and(|mode| mode & !0o777 != 0 || mode & 0o022 != 0)
                || entry.staged_mode & !0o777 != 0
                || entry.staged_mode & 0o022 != 0
            {
                bail!("installation journal entry is invalid");
            }
        }
        Ok(Some(journal))
    }

    fn write_journal(&self, journal: &Journal) -> anyhow::Result<()> {
        let temporary = self.binary.parent().unwrap().join(JOURNAL_TMP);
        if let Ok(file) = open_read(&temporary) {
            check_file(&file, &temporary, true)?;
            fs::remove_file(&temporary)?;
        }
        let bytes = serde_json::to_vec_pretty(journal)?;
        if bytes.len() as u64 > MAX_JOURNAL {
            bail!("installation journal exceeds size limit");
        }
        let mut file = open_new(&temporary, 0o600)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, self.journal_path())?;
        sync_directory(self.binary.parent().unwrap())
    }

    fn publish_scratch(
        &self,
        id: ReleaseFile,
        directory: &str,
        hash: &str,
        mode: u32,
    ) -> anyhow::Result<()> {
        let partial = self.partial(id, directory);
        if !state_matches(&file_state(&partial)?, Some(hash), Some(mode)) {
            bail!("installation partial file failed verification");
        }
        let finalized = self.scratch(id, directory);
        if fs::symlink_metadata(&finalized).is_ok() {
            bail!("installation scratch destination changed");
        }
        fs::rename(partial, &finalized)?;
        sync_directory(finalized.parent().unwrap())
    }

    fn validate_scratch(&self, journal: &Journal) -> anyhow::Result<()> {
        for parent in self.parents() {
            self.check_parent(&parent, false)?;
            for directory in [STAGE, ROLLBACK] {
                let path = parent.join(directory);
                match fs::symlink_metadata(&path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.into()),
                    Ok(_) => check_private_directory(&path)?,
                }
                for item in fs::read_dir(&path)? {
                    let item = item?;
                    let finalized = journal
                        .entries
                        .iter()
                        .find(|entry| self.scratch(entry.id, directory) == item.path());
                    if let Some(entry) = finalized {
                        let (hash, mode) = if directory == STAGE {
                            (Some(entry.staged_sha256.as_str()), Some(entry.staged_mode))
                        } else {
                            (entry.original_sha256.as_deref(), entry.original_mode)
                        };
                        if hash.is_none() || !state_matches(&file_state(&item.path())?, hash, mode)
                        {
                            bail!("finalized installation scratch changed; retain journal for recovery");
                        }
                    } else if journal.phase == Phase::Staging
                        && journal
                            .entries
                            .iter()
                            .any(|entry| self.partial(entry.id, directory) == item.path())
                    {
                        let file = open_read(&item.path())?;
                        check_file(&file, &item.path(), false)?;
                    } else {
                        bail!("foreign file in installation scratch; recovery required");
                    }
                }
            }
        }
        Ok(())
    }

    fn cleanup(&self, journal: &Journal) -> anyhow::Result<()> {
        // Validate the whole scratch set before deleting even its first file.
        self.validate_scratch(journal)?;
        for parent in self.parents() {
            for directory in [STAGE, ROLLBACK] {
                let path = parent.join(directory);
                if !path.exists() {
                    continue;
                }
                for item in fs::read_dir(&path)? {
                    fs::remove_file(item?.path())?;
                }
                fs::remove_dir(&path)?;
                sync_directory(&parent)?;
            }
        }
        fs::remove_file(self.journal_path())?;
        sync_directory(self.binary.parent().unwrap())
    }

    fn check_disk(&self, allocations: &BTreeMap<ReleaseFile, u64>) -> anyhow::Result<()> {
        let mut volumes = BTreeMap::<u64, (u128, u128, u128)>::new();
        for (&id, &bytes) in allocations {
            let mut parent = self.destinations[&id].parent().unwrap();
            while !parent.exists() {
                parent = parent.parent().context("disk parent missing")?;
            }
            let device = fs::metadata(parent)?.dev();
            let (total, available) = disk_space(parent)?;
            let volume = volumes
                .entry(device)
                .or_insert((total, available, 64 * 1024));
            volume.2 += u128::from(bytes);
        }
        for (total, available, needed) in volumes.values() {
            require_disk_reserve(*total, *available, *needed)?;
        }
        Ok(())
    }
}

fn check_directory(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o022 != 0
    {
        bail!("installation directory is unsafe: {}", path.display());
    }
    Ok(())
}

fn check_private_directory(path: &Path) -> anyhow::Result<()> {
    check_directory(path)?;
    if fs::symlink_metadata(path)?.mode() & 0o077 != 0 {
        bail!("installation scratch directory must be owner-only");
    }
    Ok(())
}

fn check_file(file: &File, path: &Path, owner_only: bool) -> anyhow::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.mode() & 0o6000 != 0
        || metadata.mode() & if owner_only { 0o077 } else { 0o022 } != 0
    {
        bail!("installation file is unsafe: {}", path.display());
    }
    Ok(())
}

fn open_read(path: &Path) -> anyhow::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    check_file(&file, path, false)?;
    Ok(file)
}

fn open_new(path: &Path, mode: u32) -> anyhow::Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(mode))?;
    Ok(file)
}

fn write_new(path: &Path, bytes: &[u8], mode: u32) -> anyhow::Result<()> {
    let mut file = open_new(path, mode)?;
    for chunk in bytes.chunks(64 * 1024) {
        let (total, available) = disk_space(path.parent().unwrap())?;
        require_disk_reserve(total, available, chunk.len() as u128)?;
        file.write_all(chunk)?;
    }
    file.sync_all()?;
    Ok(())
}

fn file_state(path: &Path) -> anyhow::Result<Option<(String, u32, u64)>> {
    let mut file = match open_read(path) {
        Ok(file) => file,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(None)
        }
        Err(error) => return Err(error),
    };
    let before = file.metadata()?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let length = file.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        digest.update(&buffer[..length]);
    }
    let after = file.metadata()?;
    if before.len() != after.len() || before.modified()? != after.modified()? {
        bail!("release file changed while reading");
    }
    Ok(Some((
        hex::encode(digest.finalize()),
        before.mode() & 0o777,
        before.len(),
    )))
}

fn state_matches(
    state: &Option<(String, u32, u64)>,
    hash: Option<&str>,
    mode: Option<u32>,
) -> bool {
    match state {
        Some((actual, actual_mode, _)) => {
            Some(actual.as_str()) == hash && Some(*actual_mode) == mode
        }
        None => hash.is_none() && mode.is_none(),
    }
}

fn require_hash(path: &Path, expected: &str) -> anyhow::Result<()> {
    if file_state(path)?.as_ref().map(|state| state.0.as_str()) != Some(expected) {
        bail!(
            "release recovery artifact hash mismatch: {}",
            path.display()
        );
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sync_directory(path: &Path) -> anyhow::Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn disk_space(path: &Path) -> anyhow::Result<(u128, u128)> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let mut status = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), status.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let status = unsafe { status.assume_init() };
    Ok((
        u128::from(status.f_blocks) * u128::from(status.f_frsize),
        u128::from(status.f_bavail) * u128::from(status.f_frsize),
    ))
}

fn require_disk_reserve(total: u128, available: u128, needed: u128) -> anyhow::Result<()> {
    let reserve = (total * RESERVE_PERCENT).div_ceil(100);
    if total == 0 || available < reserve || needed > available.saturating_sub(reserve) {
        bail!("update would cross the 15% disk reserve; previous release preserved");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn previous(id: ReleaseFile) -> Vec<u8> {
        if id == ReleaseFile::RuntimeBinary {
            b"#!/bin/sh\nprintf 'elastos 0.7.0\\n'\n".to_vec()
        } else {
            format!("previous {}", id.name()).into_bytes()
        }
    }

    struct Fixture {
        _root: tempfile::TempDir,
        data: PathBuf,
        binary: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let data = root.path().join("data");
            let binary = root.path().join("bin/elastos");
            fs::create_dir(&data).unwrap();
            fs::create_dir(binary.parent().unwrap()).unwrap();
            fs::write(data.join("owner-data"), b"data written by owner").unwrap();
            Self {
                _root: root,
                data,
                binary,
            }
        }

        fn writer(&self) -> InstallTransaction {
            InstallTransaction::acquire(&self.data, &self.binary).unwrap()
        }

        fn old_files(&self, writer: &InstallTransaction, absent_metadata: bool) {
            for id in ReleaseFile::ALL {
                if absent_metadata
                    && matches!(id, ReleaseFile::ReleaseHead | ReleaseFile::ReleaseManifest)
                {
                    continue;
                }
                let path = &writer.destinations[&id];
                writer.check_parent(path.parent().unwrap(), true).unwrap();
                write_new(
                    path,
                    &previous(id),
                    if id == ReleaseFile::RuntimeBinary {
                        0o755
                    } else {
                        0o600
                    },
                )
                .unwrap();
            }
        }

        fn assert_previous(&self, writer: &InstallTransaction, absent_metadata: bool) {
            for id in ReleaseFile::ALL {
                let path = &writer.destinations[&id];
                if absent_metadata
                    && matches!(id, ReleaseFile::ReleaseHead | ReleaseFile::ReleaseManifest)
                {
                    assert!(!path.exists());
                } else {
                    assert_eq!(fs::read(path).unwrap(), previous(id));
                }
            }
            let output = std::process::Command::new(&writer.binary)
                .arg("--version")
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, b"elastos 0.7.0\n");
            assert_eq!(
                fs::read(self.data.join("owner-data")).unwrap(),
                b"data written by owner"
            );
        }

        fn commit_before_cleanup(&self, writer: &InstallTransaction) {
            writer.prepare(&candidate()).unwrap();
            let mut journal = writer.read_journal().unwrap().unwrap();
            journal.phase = Phase::Committing;
            writer.write_journal(&journal).unwrap();
            for entry in &journal.entries {
                fs::rename(
                    writer.scratch(entry.id, STAGE),
                    &writer.destinations[&entry.id],
                )
                .unwrap();
            }
            journal.phase = Phase::Committed;
            writer.write_journal(&journal).unwrap();
        }
    }

    fn candidate() -> [(ReleaseFile, &'static [u8]); 5] {
        ReleaseFile::ALL.map(|id| (id, b"candidate bytes".as_slice()))
    }

    #[test]
    fn installation_guard_and_transaction_share_ownership_until_drop() {
        let fixture = Fixture::new();
        let parent = fixture.binary.parent().unwrap();
        let guard = InstallationGuard::acquire(parent).unwrap();
        assert!(InstallTransaction::acquire(&fixture.data, &fixture.binary).is_err());
        drop(guard);
        let writer = fixture.writer();
        assert!(InstallationGuard::acquire(parent).is_err());
        drop(writer);
        let guard = InstallationGuard::acquire(parent).unwrap();
        let inode = fs::metadata(parent.join(INSTALL_LOCK)).unwrap().ino();
        drop(guard);
        assert_eq!(
            fs::metadata(parent.join(INSTALL_LOCK)).unwrap().ino(),
            inode
        );
        let _writer = fixture.writer();
    }

    #[test]
    fn installation_guard_uses_resolved_aliases_and_keeps_distinct_parents_independent() {
        let fixture = Fixture::new();
        let parent = fs::canonicalize(fixture.binary.parent().unwrap()).unwrap();
        let alias = fixture._root.path().join("bin-alias");
        symlink(&parent, &alias).unwrap();
        let other = parent.with_file_name("other-bin");
        fs::create_dir(&other).unwrap();
        let resolved = fs::canonicalize(&alias).unwrap();
        assert!(InstallationGuard::acquire(&alias).is_err());
        let guard = InstallationGuard::acquire(&resolved).unwrap();
        assert!(InstallationGuard::acquire(&parent.join(".")).is_err());
        assert!(InstallTransaction::acquire(&fixture.data, &alias.join("elastos")).is_err());
        let _other_guard = InstallationGuard::acquire(&other).unwrap();
        drop(guard);
        let _writer = fixture.writer();
    }

    #[test]
    fn installation_guard_refuses_a_replaced_resolved_parent() {
        fn snapshot(parent: &Path) -> BTreeMap<PathBuf, (u32, Option<Vec<u8>>)> {
            fs::read_dir(parent)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .chain(std::iter::once(parent.to_path_buf()))
                .map(|path| {
                    let metadata = fs::symlink_metadata(&path).unwrap();
                    let bytes = metadata.is_file().then(|| fs::read(&path).unwrap());
                    (path, (metadata.mode(), bytes))
                })
                .collect()
        }

        let fixture = Fixture::new();
        let parent = fs::canonicalize(fixture.binary.parent().unwrap()).unwrap();
        let moved = parent.with_file_name("moved-bin");
        let other = parent.with_file_name("other-bin");
        fs::create_dir(&other).unwrap();
        write_new(&parent.join("elastos"), b"original binary", 0o755).unwrap();
        write_new(&parent.join("owner-data"), b"original owner data", 0o600).unwrap();
        write_new(&other.join("elastos"), b"other binary", 0o750).unwrap();
        write_new(&other.join("owner-data"), b"other owner data", 0o640).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o750)).unwrap();
        fs::set_permissions(&other, fs::Permissions::from_mode(0o700)).unwrap();
        fs::rename(&parent, &moved).unwrap();
        symlink(&other, &parent).unwrap();
        let moved_before = snapshot(&moved);
        let other_before = snapshot(&other);

        assert!(InstallationGuard::acquire(&parent).is_err());

        assert_eq!(snapshot(&moved), moved_before);
        assert_eq!(snapshot(&other), other_before);
        assert!(!moved.join(INSTALL_LOCK).exists());
        assert!(!other.join(INSTALL_LOCK).exists());
    }

    #[test]
    fn transaction_keeps_resolved_binary_parent_after_alias_retarget() {
        let fixture = Fixture::new();
        let parent = fs::canonicalize(fixture.binary.parent().unwrap()).unwrap();
        let alias = fixture._root.path().join("bin-alias");
        let other = parent.with_file_name("other-bin");
        fs::create_dir(&other).unwrap();
        symlink(&parent, &alias).unwrap();
        let writer = InstallTransaction::acquire(&fixture.data, &alias.join("elastos")).unwrap();
        fixture.old_files(&writer, false);
        write_new(&other.join("elastos"), b"other binary", 0o755).unwrap();
        fs::remove_file(&alias).unwrap();
        symlink(&other, &alias).unwrap();

        assert_eq!(writer.binary_path(), parent.join("elastos"));
        assert_eq!(
            writer.destinations[&ReleaseFile::RuntimeBinary],
            parent.join("elastos")
        );
        assert_eq!(writer.journal_path(), parent.join(JOURNAL));
        assert_eq!(
            writer.staged_binary(),
            parent.join(STAGE).join("runtime_binary")
        );
        assert!(InstallationGuard::acquire(&parent).is_err());
        let resolved_other = fs::canonicalize(&alias).unwrap();
        let _other_guard = InstallationGuard::acquire(&resolved_other).unwrap();
        writer.prepare(&candidate()).unwrap();
        writer.commit().unwrap();
        assert_eq!(
            fs::read(parent.join("elastos")).unwrap(),
            b"candidate bytes"
        );
        assert_eq!(fs::read(other.join("elastos")).unwrap(), b"other binary");
    }

    #[test]
    fn transaction_checks_binary_before_acquiring_installation_lock() {
        let fixture = Fixture::new();
        let parent = fixture.binary.parent().unwrap();
        assert!(InstallTransaction::acquire(&fixture.data, &parent.join(INSTALL_LOCK)).is_err());
        assert!(!parent.join(INSTALL_LOCK).exists());
        write_new(&fixture.binary, b"unsafe binary", 0o777).unwrap();
        assert!(InstallTransaction::acquire(&fixture.data, &fixture.binary).is_err());
        assert!(!parent.join(INSTALL_LOCK).exists());
        assert_eq!(fs::read(&fixture.binary).unwrap(), b"unsafe binary");
        assert_eq!(fs::metadata(&fixture.binary).unwrap().mode() & 0o777, 0o777);
    }

    #[test]
    fn installation_guard_preserves_journal_scratch_and_release_files() {
        fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u32, Option<Vec<u8>>)> {
            let mut entries = BTreeMap::new();
            for entry in fs::read_dir(root).unwrap() {
                let path = entry.unwrap().path();
                if path.file_name().unwrap() == INSTALL_LOCK {
                    continue;
                }
                let metadata = fs::symlink_metadata(&path).unwrap();
                let bytes = metadata.is_file().then(|| fs::read(&path).unwrap());
                entries.insert(path.clone(), (metadata.mode(), bytes));
                if metadata.is_dir() {
                    entries.extend(snapshot(&path));
                }
            }
            entries
        }

        let fixture = Fixture::new();
        let writer = fixture.writer();
        fixture.old_files(&writer, false);
        writer.prepare(&candidate()).unwrap();
        let parent = fixture.binary.parent().unwrap();
        drop(writer);
        fs::remove_file(parent.join(INSTALL_LOCK)).unwrap();
        let before = snapshot(fixture._root.path());
        let guard = InstallationGuard::acquire(parent).unwrap();
        assert_eq!(snapshot(fixture._root.path()), before);
        let lock = parent.join(INSTALL_LOCK);
        assert!(fs::read(&lock).unwrap().is_empty());
        assert_eq!(fs::metadata(&lock).unwrap().mode() & 0o777, 0o600);
        drop(guard);
        assert_eq!(snapshot(fixture._root.path()), before);
        assert!(lock.exists());
    }

    // A child uses the raw flock interface, independently of InstallationGuard.
    // Only the parent test selects this helper and supplies its isolated inputs.
    #[test]
    #[ignore]
    fn installation_guard_raw_flock_child() {
        use std::os::fd::AsRawFd;

        let parent = std::env::var_os("ELASTOS_INSTALL_LOCK_TEST_PARENT").unwrap();
        let action = std::env::var("ELASTOS_INSTALL_LOCK_TEST_ACTION").unwrap();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(Path::new(&parent).join(INSTALL_LOCK))
            .unwrap();
        let result = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if action == "busy" {
            assert_eq!(result, -1);
            let error = std::io::Error::last_os_error();
            assert!(
                matches!(error.raw_os_error(), Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN)
            );
            println!("\ninstallation-raw-lock:busy");
        } else {
            assert_eq!(action, "free");
            assert_eq!(result, 0);
            println!("\ninstallation-raw-lock:acquired");
        }
    }

    #[test]
    fn installation_guard_interoperates_with_raw_flock_in_another_process() {
        use std::process::Command;

        let fixture = Fixture::new();
        let parent = fixture.binary.parent().unwrap();
        let test = format!(
            "{}::installation_guard_raw_flock_child",
            module_path!().split_once("::").unwrap().1
        );
        let child_command = |action: &str| {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .env_clear()
                .env("ELASTOS_INSTALL_LOCK_TEST_PARENT", parent)
                .env("ELASTOS_INSTALL_LOCK_TEST_ACTION", action)
                .args(["--exact", &test, "--ignored", "--nocapture"]);
            command
        };
        let guard = InstallationGuard::acquire(parent).unwrap();
        let output = child_command("busy").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .any(|line| line == "installation-raw-lock:busy"));
        drop(guard);

        let output = child_command("free").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .any(|line| line == "installation-raw-lock:acquired"));
        let _guard = InstallationGuard::acquire(parent).unwrap();
    }

    #[test]
    fn installation_guard_refuses_unsafe_locks_and_preserves_their_state() {
        for mutation in [
            "hardlink",
            "directory",
            "0640",
            "0601",
            "0660",
            "4600",
            "2600",
        ] {
            let fixture = Fixture::new();
            let lock = fixture.binary.parent().unwrap().join(INSTALL_LOCK);
            let protected = fixture.data.join("owner-data");
            match mutation {
                "hardlink" => {
                    fs::set_permissions(&protected, fs::Permissions::from_mode(0o600)).unwrap();
                    fs::hard_link(&protected, &lock).unwrap();
                }
                "directory" => fs::create_dir(&lock).unwrap(),
                mode => {
                    fs::write(&lock, b"preserve lock bytes").unwrap();
                    fs::set_permissions(
                        &lock,
                        fs::Permissions::from_mode(u32::from_str_radix(mode, 8).unwrap()),
                    )
                    .unwrap();
                }
            }
            let before = fs::symlink_metadata(&lock).unwrap();
            assert!(
                InstallationGuard::acquire(fixture.binary.parent().unwrap()).is_err(),
                "{mutation}"
            );
            assert!(
                InstallTransaction::acquire(&fixture.data, &fixture.binary).is_err(),
                "{mutation}"
            );
            let after = fs::symlink_metadata(&lock).unwrap();
            assert_eq!(
                (after.ino(), after.mode(), after.nlink()),
                (before.ino(), before.mode(), before.nlink())
            );
            if after.is_file() {
                assert_eq!(
                    fs::read(&lock).unwrap(),
                    if mutation == "hardlink" {
                        b"data written by owner".as_slice()
                    } else {
                        b"preserve lock bytes".as_slice()
                    }
                );
            }
            assert_eq!(fs::read(protected).unwrap(), b"data written by owner");
        }
    }

    #[test]
    fn installation_guard_refuses_unsafe_missing_and_relative_parents() {
        let fixture = Fixture::new();
        let parent = fixture.binary.parent().unwrap();
        let missing = parent.join("missing");
        assert!(InstallationGuard::acquire(&missing).is_err());
        assert!(!missing.exists());
        assert!(InstallationGuard::acquire(Path::new(".")).is_err());
        let file = fixture.data.join("owner-data");
        assert!(InstallationGuard::acquire(&file).is_err());
        for mode in [0o770, 0o707] {
            fs::set_permissions(parent, fs::Permissions::from_mode(mode)).unwrap();
            assert!(InstallationGuard::acquire(parent).is_err());
            assert!(!parent.join(INSTALL_LOCK).exists());
            assert_eq!(fs::metadata(parent).unwrap().mode() & 0o777, mode);
        }
        assert_eq!(fs::read(file).unwrap(), b"data written by owner");
    }

    #[test]
    fn every_late_file_failure_restores_complete_previous_release() {
        for absent in [false, true] {
            for fault in ReleaseFile::ALL {
                let fixture = Fixture::new();
                let writer = fixture.writer();
                fixture.old_files(&writer, absent);
                writer.prepare(&candidate()).unwrap();
                let error = writer
                    .commit_with(
                        |id| {
                            if id == fault {
                                bail!("injected late write failure");
                            }
                            Ok(())
                        },
                        || Ok(()),
                    )
                    .unwrap_err();
                assert!(format!("{error:#}").contains("previous release restored"));
                fixture.assert_previous(&writer, absent);
                writer.require_empty_scratch().unwrap();
                // The corrected retry uses the real paths and leaves one complete candidate.
                writer.prepare(&candidate()).unwrap();
                writer.commit().unwrap();
                for path in writer.destinations.values() {
                    assert_eq!(fs::read(path).unwrap(), b"candidate bytes");
                }
            }
        }
    }

    #[test]
    fn next_writer_recovers_each_interrupted_activation_prefix() {
        for count in 0..=ReleaseFile::ALL.len() {
            let fixture = Fixture::new();
            let writer = fixture.writer();
            fixture.old_files(&writer, true);
            writer.prepare(&candidate()).unwrap();
            let mut journal = writer.read_journal().unwrap().unwrap();
            journal.phase = Phase::Committing;
            writer.write_journal(&journal).unwrap();
            for entry in journal.entries.iter().take(count) {
                fs::rename(
                    writer.scratch(entry.id, STAGE),
                    &writer.destinations[&entry.id],
                )
                .unwrap();
                sync_directory(writer.destinations[&entry.id].parent().unwrap()).unwrap();
            }
            drop(writer);
            let resumed = fixture.writer();
            resumed.recover().unwrap();
            fixture.assert_previous(&resumed, true);
            resumed.recover().unwrap();
            resumed.require_empty_scratch().unwrap();
        }
    }

    #[test]
    fn final_verification_failure_restores_release_and_preserves_new_owner_data() {
        let fixture = Fixture::new();
        let writer = fixture.writer();
        fixture.old_files(&writer, false);
        writer.prepare(&candidate()).unwrap();
        writer
            .commit_checked(|| {
                fs::write(fixture.data.join("owner-data"), b"new owner data").unwrap();
                bail!("candidate verification failed")
            })
            .unwrap_err();
        for id in ReleaseFile::ALL {
            assert_eq!(fs::read(&writer.destinations[&id]).unwrap(), previous(id));
        }
        assert_eq!(
            fs::read(fixture.data.join("owner-data")).unwrap(),
            b"new owner data"
        );
    }

    #[test]
    fn foreign_changes_and_corrupt_rollback_require_reconciliation() {
        for foreign in [false, true] {
            let fixture = Fixture::new();
            let writer = fixture.writer();
            fixture.old_files(&writer, false);
            writer.prepare(&candidate()).unwrap();
            let mut journal = writer.read_journal().unwrap().unwrap();
            journal.phase = Phase::Committing;
            writer.write_journal(&journal).unwrap();
            fs::rename(
                writer.scratch(ReleaseFile::RuntimeBinary, STAGE),
                &writer.binary,
            )
            .unwrap();
            if foreign {
                fs::write(&writer.binary, b"foreign operator change").unwrap();
            } else {
                fs::write(
                    writer.scratch(ReleaseFile::RuntimeBinary, ROLLBACK),
                    b"corrupt rollback",
                )
                .unwrap();
            }
            drop(writer);
            let resumed = fixture.writer();
            assert!(resumed.recover().is_err());
            assert!(resumed.journal_path().exists());
            assert_eq!(
                fs::read(&resumed.binary).unwrap(),
                if foreign {
                    b"foreign operator change".as_slice()
                } else {
                    b"candidate bytes".as_slice()
                }
            );
            assert_eq!(
                fs::read(&resumed.destinations[&ReleaseFile::Components]).unwrap(),
                b"previous components"
            );
        }
    }

    #[test]
    fn lock_symlinks_overlap_and_unsafe_parents_refuse_before_release_writes() {
        let fixture = Fixture::new();
        let writer = fixture.writer();
        assert!(InstallTransaction::acquire(&fixture.data, &fixture.binary).is_err());
        drop(writer);
        let lock = fixture.binary.parent().unwrap().join(INSTALL_LOCK);
        fs::remove_file(&lock).unwrap();
        let protected = fixture.data.join("owner-data");
        symlink(&protected, &lock).unwrap();
        assert!(InstallationGuard::acquire(fixture.binary.parent().unwrap()).is_err());
        assert!(InstallTransaction::acquire(&fixture.data, &fixture.binary).is_err());
        assert_eq!(fs::read(&protected).unwrap(), b"data written by owner");
        fs::remove_file(lock).unwrap();
        assert!(
            InstallTransaction::acquire(&fixture.data, &fixture.data.join("components.json"))
                .is_err()
        );
        fs::set_permissions(
            fixture.binary.parent().unwrap(),
            fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        assert!(InstallTransaction::acquire(&fixture.data, &fixture.binary).is_err());
    }

    #[test]
    fn malformed_identity_and_foreign_scratch_are_retained() {
        let fixture = Fixture::new();
        let writer = fixture.writer();
        fixture.old_files(&writer, false);
        writer.prepare(&candidate()).unwrap();
        let mut journal = writer.read_journal().unwrap().unwrap();
        journal.data_dir = fixture.data.join("different-home");
        writer.write_journal(&journal).unwrap();
        assert!(writer.recover().is_err());
        fixture.assert_previous(&writer, false);
        journal.data_dir = writer.data_dir.clone();
        writer.write_journal(&journal).unwrap();
        write_new(
            &writer
                .scratch(ReleaseFile::RuntimeBinary, STAGE)
                .parent()
                .unwrap()
                .join("foreign-file"),
            b"preserve",
            0o600,
        )
        .unwrap();
        assert!(writer.recover().is_err());
        assert!(writer.journal_path().exists());
    }

    #[test]
    fn next_writer_recovers_partial_staging_and_prepared_release() {
        for partial_directory in [None, Some(STAGE), Some(ROLLBACK)] {
            let fixture = Fixture::new();
            let writer = fixture.writer();
            fixture.old_files(&writer, false);
            writer.prepare(&candidate()).unwrap();
            if let Some(directory) = partial_directory {
                let mut journal = writer.read_journal().unwrap().unwrap();
                journal.phase = Phase::Staging;
                writer.write_journal(&journal).unwrap();
                let partial = writer.partial(ReleaseFile::RuntimeBinary, directory);
                fs::rename(
                    writer.scratch(ReleaseFile::RuntimeBinary, directory),
                    &partial,
                )
                .unwrap();
                fs::write(&partial, b"interrupted partial bytes").unwrap();
            }
            drop(writer);
            let resumed = fixture.writer();
            resumed.recover().unwrap();
            fixture.assert_previous(&resumed, false);
            resumed.require_empty_scratch().unwrap();
        }
    }

    #[test]
    fn next_writer_finishes_interrupted_committed_cleanup() {
        let fixture = Fixture::new();
        let writer = fixture.writer();
        fixture.old_files(&writer, false);
        fixture.commit_before_cleanup(&writer);
        fs::remove_file(writer.scratch(ReleaseFile::RuntimeBinary, ROLLBACK)).unwrap();
        fs::remove_dir(
            writer
                .scratch(ReleaseFile::RuntimeBinary, STAGE)
                .parent()
                .unwrap(),
        )
        .unwrap();
        drop(writer);
        let resumed = fixture.writer();
        resumed.recover().unwrap();
        for destination in resumed.destinations.values() {
            assert_eq!(fs::read(destination).unwrap(), b"candidate bytes");
        }
        resumed.require_empty_scratch().unwrap();
    }

    #[test]
    fn committed_cleanup_preserves_later_owner_bytes_and_modes() {
        let fixture = Fixture::new();
        let writer = fixture.writer();
        fixture.old_files(&writer, false);
        fixture.commit_before_cleanup(&writer);
        fs::remove_file(writer.scratch(ReleaseFile::RuntimeBinary, ROLLBACK)).unwrap();
        for id in ReleaseFile::ALL {
            let destination = &writer.destinations[&id];
            fs::write(destination, format!("later owner {}", id.name())).unwrap();
            let mode = if id == ReleaseFile::RuntimeBinary {
                0o750
            } else {
                0o640
            };
            fs::set_permissions(destination, fs::Permissions::from_mode(mode)).unwrap();
        }
        drop(writer);
        let resumed = fixture.writer();
        resumed.recover().unwrap();
        resumed.require_empty_scratch().unwrap();
        resumed.recover().unwrap();
        for id in ReleaseFile::ALL {
            let destination = &resumed.destinations[&id];
            assert_eq!(
                fs::read(destination).unwrap(),
                format!("later owner {}", id.name()).as_bytes()
            );
            let mode = if id == ReleaseFile::RuntimeBinary {
                0o750
            } else {
                0o640
            };
            assert_eq!(fs::metadata(destination).unwrap().mode() & 0o777, mode);
        }
        assert_eq!(
            fs::read(fixture.data.join("owner-data")).unwrap(),
            b"data written by owner"
        );
    }

    #[test]
    fn committed_cleanup_refuses_unsafe_live_changes_after_lock_acquisition() {
        for (id, mutation) in ReleaseFile::ALL.into_iter().flat_map(|id| {
            ["symlink", "hardlink", "writable", "missing"]
                .into_iter()
                .map(move |mutation| (id, mutation))
        }) {
            let fixture = Fixture::new();
            let writer = fixture.writer();
            fixture.old_files(&writer, false);
            fixture.commit_before_cleanup(&writer);
            let destination = &writer.destinations[&id];
            let owner_data = fixture.data.join("owner-data");
            match mutation {
                "symlink" => {
                    fs::remove_file(destination).unwrap();
                    symlink(&owner_data, destination).unwrap();
                }
                "hardlink" => {
                    fs::remove_file(destination).unwrap();
                    fs::hard_link(&owner_data, destination).unwrap();
                }
                "missing" => fs::remove_file(destination).unwrap(),
                _ => fs::set_permissions(destination, fs::Permissions::from_mode(0o666)).unwrap(),
            }
            assert!(writer.recover().is_err(), "{}: {mutation}", id.name());
            assert!(writer.journal_path().exists());
            for id in ReleaseFile::ALL {
                assert!(writer.scratch(id, ROLLBACK).exists());
            }
            assert_eq!(fs::read(owner_data).unwrap(), b"data written by owner");
        }
    }

    #[test]
    fn committed_cleanup_retains_changed_scratch_and_later_owner_data() {
        for mutation in ["corrupt", "foreign"] {
            let fixture = Fixture::new();
            let writer = fixture.writer();
            fixture.old_files(&writer, false);
            fixture.commit_before_cleanup(&writer);
            let destination = &writer.destinations[&ReleaseFile::Sources];
            fs::write(destination, b"later owner sources").unwrap();
            let changed_scratch = if mutation == "corrupt" {
                writer.scratch(ReleaseFile::Sources, ROLLBACK)
            } else {
                writer
                    .scratch(ReleaseFile::Sources, ROLLBACK)
                    .with_file_name("foreign")
            };
            fs::write(&changed_scratch, b"preserve changed scratch").unwrap();
            assert!(writer.recover().is_err(), "{mutation}");
            assert!(writer.journal_path().exists());
            for id in ReleaseFile::ALL {
                assert!(writer.scratch(id, ROLLBACK).exists());
            }
            assert_eq!(fs::read(destination).unwrap(), b"later owner sources");
            assert_eq!(
                fs::read(changed_scratch).unwrap(),
                b"preserve changed scratch"
            );
        }
    }

    #[test]
    fn next_writer_resumes_interrupted_recovery_prefix() {
        let fixture = Fixture::new();
        let writer = fixture.writer();
        fixture.old_files(&writer, false);
        writer.prepare(&candidate()).unwrap();
        let mut journal = writer.read_journal().unwrap().unwrap();
        journal.phase = Phase::Committing;
        writer.write_journal(&journal).unwrap();
        for entry in journal.entries.iter().take(3) {
            fs::rename(
                writer.scratch(entry.id, STAGE),
                &writer.destinations[&entry.id],
            )
            .unwrap();
        }
        journal.phase = Phase::Recovering;
        writer.write_journal(&journal).unwrap();
        fs::rename(
            writer.scratch(ReleaseFile::RuntimeBinary, ROLLBACK),
            &writer.binary,
        )
        .unwrap();
        sync_directory(writer.binary.parent().unwrap()).unwrap();
        drop(writer);
        let resumed = fixture.writer();
        resumed.recover().unwrap();
        fixture.assert_previous(&resumed, false);
        resumed.require_empty_scratch().unwrap();
    }

    #[test]
    fn unchanged_live_originals_do_not_authorize_discarding_foreign_finalized_scratch() {
        for directory in [STAGE, ROLLBACK] {
            let fixture = Fixture::new();
            let writer = fixture.writer();
            fixture.old_files(&writer, false);
            writer.prepare(&candidate()).unwrap();
            let scratch = writer.scratch(ReleaseFile::ReleaseManifest, directory);
            fs::write(&scratch, b"foreign scratch changes").unwrap();
            assert!(writer.recover().is_err());
            fixture.assert_previous(&writer, false);
            assert_eq!(fs::read(&scratch).unwrap(), b"foreign scratch changes");
            assert!(writer
                .scratch(ReleaseFile::RuntimeBinary, ROLLBACK)
                .exists());
            assert!(writer.journal_path().exists());
        }
    }

    #[test]
    fn same_binary_bytes_with_changed_mode_restore_previous_executable() {
        let fixture = Fixture::new();
        let writer = fixture.writer();
        fixture.old_files(&writer, false);
        fs::set_permissions(&writer.binary, fs::Permissions::from_mode(0o700)).unwrap();
        let old_binary = previous(ReleaseFile::RuntimeBinary);
        let mut next: [(ReleaseFile, &[u8]); 5] = candidate();
        next[0].1 = &old_binary;
        writer.prepare(&next).unwrap();
        writer
            .commit_with(
                |id| {
                    if id == ReleaseFile::RuntimeBinary {
                        bail!("late failure after mode change");
                    }
                    Ok(())
                },
                || Ok(()),
            )
            .unwrap_err();
        fixture.assert_previous(&writer, false);
        assert_eq!(fs::metadata(&writer.binary).unwrap().mode() & 0o777, 0o700);
    }

    #[test]
    fn disk_floor_counts_projected_peak_and_accepts_exact_reserve() {
        assert!(require_disk_reserve(1000, 149, 0).is_err());
        assert!(require_disk_reserve(1000, 200, 51).is_err());
        assert!(require_disk_reserve(1000, 200, 50).is_ok());
        assert!(require_disk_reserve(0, 200, 0).is_err());
    }
}
