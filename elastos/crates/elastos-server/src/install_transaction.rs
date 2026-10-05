//! Installed release-file transaction. Journal schemas bind their fixed destinations.
//! Updates stage all five release files; the installer leaves components to setup.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use elastos_common::localhost::{
    installation_release_head_path, installation_release_manifest_path,
    publisher_release_head_path, publisher_release_manifest_path,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod support;
pub(crate) use support::validate_support_path;

const INSTALL_LOCK: &str = ".elastos.install.lock";
const JOURNAL: &str = ".elastos.update-journal.json";
const JOURNAL_TMP: &str = ".elastos.update-journal.tmp";
const STAGE: &str = ".elastos.update-stage";
const ROLLBACK: &str = ".elastos.update-rollback";
const MAX_JOURNAL: u64 = 16 * 1024;
const RESERVE_PERCENT: u128 = 15;

#[derive(Debug)]
pub(crate) struct DiskReserveError;

impl std::fmt::Display for DiskReserveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("update would cross the 15% disk reserve; previous release preserved")
    }
}

impl std::error::Error for DiskReserveError {}

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

    /// Installer transactions omit components, which Runtime setup installs.
    fn complete_set(ids: impl Iterator<Item = Self>) -> bool {
        let mut count = 0;
        let ids = ids.inspect(|_| count += 1).collect::<BTreeSet<_>>();
        ids.len() == count
            && (ids.len() == Self::ALL.len()
                || ids.len() == Self::ALL.len() - 1 && !ids.contains(&Self::Components))
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restart: Option<RestartRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    support: Vec<support::Entry>,
}

/// A restart controller keeps this record and the verified rollback until Home is ready.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RestartPlan {
    pub request_id: String,
    pub controller_sha256: String,
    pub launch_plan_sha256: String,
    pub support_sha256: String,
    pub previous_version: String,
    pub candidate_version: String,
    pub previous_binary_sha256: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RestartPhase {
    CandidatePending,
    CandidateStartClaimed,
    CandidateRunning,
    CandidateReady,
    Restoring,
    Restored,
    PreviousStartClaimed,
    PreviousRunning,
    PreviousReady,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RestartRecord {
    pub plan: RestartPlan,
    pub phase: RestartPhase,
    pub generation: String,
    pub pid: Option<u32>,
    pub process_start: Option<String>,
}

/// Serializes installation writers at an absolute, resolved binary parent.
/// Writers resolve the parent once and use the same path for their destinations.
/// The lock path stays in place after the guard releases the flock and closes its file.
pub(crate) struct InstallationGuard {
    _lock: File,
    binary_parent: PathBuf,
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
        Ok(Self {
            _lock: lock,
            binary_parent: binary_parent.to_path_buf(),
        })
    }

    pub(crate) fn require_binary(&self, binary: &Path) -> anyhow::Result<()> {
        if !binary.is_absolute()
            || binary.parent().context("installed binary parent missing")?
                != self.binary_parent.as_path()
        {
            bail!("installed release writer owns a different binary parent");
        }
        check_directory(&self.binary_parent)?;
        let path = self.binary_parent.join(INSTALL_LOCK);
        check_file(&self._lock, &path, true)?;
        let held = self._lock.metadata()?;
        let current = fs::symlink_metadata(path)?;
        if !current.is_file() || held.dev() != current.dev() || held.ino() != current.ino() {
            bail!("installed release writer lock identity changed");
        }
        Ok(())
    }
}

impl Drop for InstallationGuard {
    fn drop(&mut self) {
        // A forked command can retain this description until its CLOEXEC fd closes.
        // The guard's scope owns the lock, so release it before closing our fd.
        let _ = unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&self._lock), libc::LOCK_UN) };
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReleaseLayout {
    LegacyPublisher,
    Consumed,
}

fn journal_layout(journal: &Journal) -> anyhow::Result<ReleaseLayout> {
    match (journal.schema.as_str(), journal.restart.is_some()) {
        ("elastos.install-transaction/v1", false) | ("elastos.install-transaction/v2", true) => {
            Ok(ReleaseLayout::LegacyPublisher)
        }
        ("elastos.install-transaction/v3", false) | ("elastos.install-transaction/v4", true) => {
            Ok(ReleaseLayout::Consumed)
        }
        _ => bail!("installation journal schema is incompatible with this writer"),
    }
}

fn read_journal_file(path: &Path) -> anyhow::Result<Option<Journal>> {
    let file = match open_read(path) {
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
    check_file(&file, path, true)?;
    if file.metadata()?.len() > MAX_JOURNAL {
        bail!("installation journal exceeds size limit");
    }
    let mut bytes = Vec::new();
    file.take(MAX_JOURNAL + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JOURNAL {
        bail!("installation journal exceeds size limit");
    }
    let journal: Journal = serde_json::from_slice(&bytes)?;
    journal_layout(&journal)?;
    Ok(Some(journal))
}

/// Ordinary installed writers use the persisted binary location, so replacing source
/// settings cannot redirect them away from a retained transaction or its lock.
/// A fresh source has no installation yet; installer/bootstrap ownership covers that path.
pub(crate) fn acquire_installed_writer(
    data_dir: &Path,
) -> anyhow::Result<Option<InstallationGuard>> {
    let Some(binary) = installed_binary(
        data_dir,
        crate::update_controller::installed_writer_binary(data_dir)?,
    )?
    else {
        return Ok(None);
    };
    let parent = binary.parent().context("installed writer parent missing")?;
    let parent = match fs::canonicalize(parent) {
        Ok(parent) => parent,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let guard = InstallationGuard::acquire(&parent)?;
    match fs::symlink_metadata(parent.join(JOURNAL)) {
        Ok(_) => {
            bail!("A signed update requires recovery; preserve its release files and support.")
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(Some(guard))
}

fn installed_binary(data_dir: &Path, retained: Option<PathBuf>) -> anyhow::Result<Option<PathBuf>> {
    let binary = match retained {
        Some(binary) => binary,
        None => {
            let sources = crate::sources::load_trusted_sources(data_dir)?;
            let Some(source) = sources
                .default_source()
                .filter(|source| !source.install_path.is_empty())
            else {
                return Ok(None);
            };
            PathBuf::from(&source.install_path)
        }
    };
    if !binary.is_absolute() {
        bail!("installed writer binary path must be absolute");
    }
    Ok(Some(binary))
}

pub(crate) struct InstallTransaction {
    _guard: InstallationGuard,
    data_dir: PathBuf,
    binary: PathBuf,
    destinations: BTreeMap<ReleaseFile, PathBuf>,
    layout: ReleaseLayout,
}

/// Borrows the writer that activated this CLI release while migration is pending.
/// Its private identity admits only the principal-root migration host lock.
pub(crate) struct SupportActivation<'a> {
    transaction: &'a InstallTransaction,
    journal_sha256: String,
}

impl SupportActivation<'_> {
    pub(crate) fn authorize_principal_root_migration(&self, data_dir: &Path) -> anyhow::Result<()> {
        let transaction = self.transaction;
        transaction._guard.require_binary(&transaction.binary)?;
        if fs::canonicalize(data_dir)? != transaction.data_dir {
            bail!("principal-root migration belongs to a different update Home");
        }
        let journal = transaction
            .read_journal()?
            .context("principal-root migration activation journal missing")?;
        if journal.schema != "elastos.install-transaction/v3"
            || journal.phase != Phase::Committing
            || journal.restart.is_some()
            || sha256(&serde_json::to_vec(&journal)?) != self.journal_sha256
        {
            bail!("principal-root migration requires its own pending CLI activation");
        }
        transaction.validate_scratch(&journal)?;
        for entry in &journal.entries {
            transaction.check_metadata_custody(entry.id)?;
            let activated = matches!(
                entry.id,
                ReleaseFile::RuntimeBinary | ReleaseFile::Components
            );
            transaction.require_state(
                entry.id,
                if activated {
                    Some(&entry.staged_sha256)
                } else {
                    entry.original_sha256.as_deref()
                },
                if activated {
                    Some(entry.staged_mode)
                } else {
                    entry.original_mode
                },
            )?;
            let stage = transaction.scratch(entry.id, STAGE);
            let rollback = transaction.scratch(entry.id, ROLLBACK);
            check_private_directory(stage.parent().unwrap())?;
            check_private_directory(rollback.parent().unwrap())?;
            if !state_matches(
                &file_state(&stage)?,
                if activated {
                    None
                } else {
                    Some(&entry.staged_sha256)
                },
                if activated {
                    None
                } else {
                    Some(entry.staged_mode)
                },
            ) || !state_matches(
                &file_state(&rollback)?,
                entry.original_sha256.as_deref(),
                entry.original_mode,
            ) {
                bail!("principal-root migration activation custody changed; retain recovery files");
            }
        }
        Ok(())
    }
}

impl InstallTransaction {
    pub(crate) fn has_pending_recovery(binary: &Path) -> bool {
        binary
            .parent()
            .is_some_and(|parent| match fs::symlink_metadata(parent.join(JOURNAL)) {
                Ok(_) => true,
                Err(error) => error.kind() != std::io::ErrorKind::NotFound,
            })
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
        let layout = read_journal_file(&bin_parent.join(JOURNAL))?
            .as_ref()
            .map(journal_layout)
            .transpose()?
            .unwrap_or(ReleaseLayout::Consumed);
        let destinations = BTreeMap::from([
            (ReleaseFile::RuntimeBinary, binary.clone()),
            (ReleaseFile::Components, data_dir.join("components.json")),
            (ReleaseFile::Sources, data_dir.join("sources.json")),
            (
                ReleaseFile::ReleaseHead,
                match layout {
                    ReleaseLayout::LegacyPublisher => publisher_release_head_path(&data_dir),
                    ReleaseLayout::Consumed => installation_release_head_path(&data_dir),
                },
            ),
            (
                ReleaseFile::ReleaseManifest,
                match layout {
                    ReleaseLayout::LegacyPublisher => publisher_release_manifest_path(&data_dir),
                    ReleaseLayout::Consumed => installation_release_manifest_path(&data_dir),
                },
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
            layout,
        };
        // Check every destination and parent before the journal can authorize writes.
        for (&id, destination) in &tx.destinations {
            tx.check_parent(destination.parent().unwrap(), false)?;
            tx.check_metadata_custody(id)?;
            file_state(destination)?;
        }
        Ok(tx)
    }

    fn check_metadata_custody(&self, id: ReleaseFile) -> anyhow::Result<()> {
        if self.uses_consumed_layout()
            && matches!(id, ReleaseFile::ReleaseHead | ReleaseFile::ReleaseManifest)
        {
            let destination = &self.destinations[&id];
            match fs::symlink_metadata(destination.parent().unwrap()) {
                Ok(metadata) if metadata.mode() & 0o7777 != 0o700 => {
                    bail!("Consumed release inputs require an owner-only directory.")
                }
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(error.into())
                }
                _ => {}
            }
            match open_read(destination) {
                Ok(file) if file.metadata()?.mode() & 0o7777 != 0o600 => {
                    bail!("Consumed release inputs require owner-only files.")
                }
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_none_or(|error| error.kind() != std::io::ErrorKind::NotFound) =>
                {
                    return Err(error)
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(crate) fn binary_path(&self) -> &Path {
        &self.binary
    }

    pub(crate) fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Recovery reads the same metadata namespace selected by the journal schema.
    pub(crate) fn release_manifest_path(&self) -> &Path {
        &self.destinations[&ReleaseFile::ReleaseManifest]
    }

    pub(crate) fn release_head_path(&self) -> &Path {
        &self.destinations[&ReleaseFile::ReleaseHead]
    }

    pub(crate) fn writer_guard(&self) -> &InstallationGuard {
        &self._guard
    }

    pub(crate) fn uses_consumed_layout(&self) -> bool {
        self.layout == ReleaseLayout::Consumed
    }

    #[cfg(test)]
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
        paths.insert(self.data_dir.join(support::SCRATCH));
        if let Ok(Some(journal)) = self.read_journal() {
            paths.extend(
                journal
                    .support
                    .iter()
                    .map(|entry| self.data_dir.join(&entry.path)),
            );
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
        if journal.restart.is_some() && !rollback_committed {
            bail!("Home restart is pending; use the installed restart controller to recover");
        }
        // Every finalized scratch file is hash/mode bound before any release
        // restoration. Partial writes have distinct names and only staging owns them.
        self.validate_scratch(&journal)?;
        if journal.phase == Phase::Staging {
            support::require_original(self, &journal.support)?;
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
                self.check_metadata_custody(entry.id)?;
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
            support::validate_restore(self, &journal.support)?;
            journal.phase = Phase::Recovering;
            if let Some(restart) = &mut journal.restart {
                restart.phase = RestartPhase::Restoring;
                restart.generation.clear();
                restart.pid = None;
                restart.process_start = None;
            }
            self.write_journal(&journal)?;
            support::restore(self, &journal.support)?;
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
        if let Some(restart) = &mut journal.restart {
            restart.phase = RestartPhase::Restored;
            restart.generation.clear();
            restart.pid = None;
            restart.process_start = None;
            return self.write_journal(&journal);
        }
        self.cleanup(&journal)
    }

    /// Check all refusal-only preparation gates before publishing migrated custody.
    pub(crate) fn preflight_prepare(
        &self,
        files: &[(ReleaseFile, &[u8])],
        previous_head: &[u8],
        previous_release: &[u8],
    ) -> anyhow::Result<()> {
        if !self.uses_consumed_layout() {
            bail!("Legacy recovery must finish before preparing a new installed release.");
        }
        self.require_empty_scratch()?;
        if files.len() != ReleaseFile::ALL.len()
            || files.iter().map(|item| item.0).collect::<BTreeSet<_>>()
                != ReleaseFile::ALL.into_iter().collect()
        {
            bail!("release transaction requires exactly five release files");
        }
        let mut allocations = BTreeMap::new();
        for &(id, bytes) in files {
            self.check_parent(self.destinations[&id].parent().unwrap(), false)?;
            self.check_metadata_custody(id)?;
            let state = file_state(&self.destinations[&id])?;
            let old_metadata = match id {
                ReleaseFile::ReleaseHead => previous_head.len() as u64,
                ReleaseFile::ReleaseManifest => previous_release.len() as u64,
                _ => 0,
            };
            let original = state.as_ref().map(|state| state.2).unwrap_or(old_metadata);
            // An absent pair first needs its migration copy as well as rollback.
            let migration = if state.is_none() { old_metadata } else { 0 };
            allocations.insert(id, bytes.len() as u64 + original + migration);
        }
        self.check_disk(&allocations)
    }

    pub(crate) fn prepare(&self, files: &[(ReleaseFile, &[u8])]) -> anyhow::Result<()> {
        if self.layout == ReleaseLayout::LegacyPublisher {
            bail!("Legacy recovery must finish before preparing a new installed release.");
        }
        self.require_empty_scratch()?;
        if !ReleaseFile::complete_set(files.iter().map(|item| item.0)) {
            bail!("release transaction requires the complete release file set");
        }
        let mut entries = Vec::new();
        let mut allocations = BTreeMap::new();
        for &(id, bytes) in files {
            self.check_metadata_custody(id)?;
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
            schema: "elastos.install-transaction/v3".to_string(),
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
            restart: None,
            support: Vec::new(),
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

    pub(crate) fn prepare_support(&self, paths: &[(PathBuf, PathBuf)]) -> anyhow::Result<()> {
        support::prepare(self, paths)
    }

    pub(crate) fn original_components(&self) -> anyhow::Result<Vec<u8>> {
        let journal = self.read_journal()?.context("release journal missing")?;
        let entry = journal
            .entries
            .iter()
            .find(|entry| entry.id == ReleaseFile::Components)
            .unwrap();
        let path = self.scratch(ReleaseFile::Components, ROLLBACK);
        let path = if path.exists() {
            path
        } else {
            self.destinations[&ReleaseFile::Components].clone()
        };
        require_hash(
            &path,
            entry
                .original_sha256
                .as_deref()
                .context("previous components missing")?,
        )?;
        Ok(fs::read(path)?)
    }

    pub(crate) fn verify_support(&self, previous: bool) -> anyhow::Result<()> {
        let journal = self.read_journal()?.context("release journal missing")?;
        support::require_installed(self, &journal.support, previous)
    }

    pub(crate) fn abort_pre_activation_restart(&self) -> anyhow::Result<()> {
        let mut journal = self.read_journal()?.context("release journal missing")?;
        anyhow::ensure!(
            journal.phase == Phase::Prepared
                && journal
                    .restart
                    .as_ref()
                    .is_some_and(|restart| restart.phase == RestartPhase::CandidatePending),
            "activation already has recovery ownership"
        );
        journal.restart = None;
        journal.schema = "elastos.install-transaction/v3".into();
        self.write_journal(&journal)?;
        self.abort()
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

    pub(crate) fn prepare_restart(&self, plan: RestartPlan) -> anyhow::Result<()> {
        if self.layout == ReleaseLayout::LegacyPublisher {
            bail!("Legacy recovery must finish before preparing a new Home restart.");
        }
        validate_restart_plan(&plan)?;
        let mut journal = self
            .read_journal()?
            .context("prepared release journal missing")?;
        if journal.phase != Phase::Prepared || journal.restart.is_some() {
            bail!("release is not prepared for a new restart");
        }
        if journal
            .entries
            .iter()
            .any(|entry| entry.original_sha256.is_none())
            || journal
                .entries
                .iter()
                .find(|entry| entry.id == ReleaseFile::RuntimeBinary)
                .and_then(|entry| entry.original_sha256.as_deref())
                != Some(&plan.previous_binary_sha256)
        {
            bail!("automatic recovery requires a verified previous complete release");
        }
        journal.schema = "elastos.install-transaction/v4".into();
        journal.restart = Some(RestartRecord {
            plan,
            phase: RestartPhase::CandidatePending,
            generation: String::new(),
            pid: None,
            process_start: None,
        });
        self.write_journal(&journal)
    }

    pub(crate) fn restart_record(&self) -> anyhow::Result<RestartRecord> {
        self.restart_record_if_any()?
            .context("restart journal missing")
    }

    pub(crate) fn restart_record_if_any(&self) -> anyhow::Result<Option<RestartRecord>> {
        Ok(self.read_journal()?.and_then(|journal| journal.restart))
    }

    /// Controller loss before restart planning can leave an unchanged CLI staging journal.
    /// Recover only pre-activation state in its schema-bound namespace.
    pub(crate) fn recover_before_restart(&self) -> anyhow::Result<bool> {
        let Some(journal) = self.read_journal()? else {
            return Ok(false);
        };
        if journal.restart.is_some() {
            return Ok(false);
        }
        if !matches!(journal.phase, Phase::Staging | Phase::Prepared) {
            bail!("Pending release has no controller restart receipt; use its original recovery owner.");
        }
        self.recover()?;
        Ok(true)
    }

    pub(crate) fn claim_start(&self, previous: bool) -> anyhow::Result<RestartRecord> {
        let mut journal = self.read_journal()?.context("restart journal missing")?;
        let restart = journal.restart.as_mut().context("restart record missing")?;
        let expected = if previous {
            RestartPhase::Restored
        } else {
            RestartPhase::CandidatePending
        };
        if restart.phase != expected || (!previous && journal.phase != Phase::Committed) {
            bail!("restart start claim already consumed or release is not activated");
        }
        restart.phase = if previous {
            RestartPhase::PreviousStartClaimed
        } else {
            RestartPhase::CandidateStartClaimed
        };
        restart.generation = hex::encode(rand::random::<[u8; 16]>());
        restart.pid = None;
        restart.process_start = None;
        let record = restart.clone();
        self.write_journal(&journal)?;
        Ok(record)
    }

    pub(crate) fn record_started(
        &self,
        generation: &str,
        pid: u32,
        process_start: String,
    ) -> anyhow::Result<()> {
        let mut journal = self.read_journal()?.context("restart journal missing")?;
        let restart = journal.restart.as_mut().context("restart record missing")?;
        if restart.generation != generation
            || pid == 0
            || process_start.is_empty()
            || process_start.len() > 128
        {
            bail!("restart process identity is invalid");
        }
        restart.phase = match restart.phase {
            RestartPhase::CandidateStartClaimed => RestartPhase::CandidateRunning,
            RestartPhase::PreviousStartClaimed => RestartPhase::PreviousRunning,
            _ => bail!("restart start claim is not available"),
        };
        restart.pid = Some(pid);
        restart.process_start = Some(process_start);
        self.write_journal(&journal)
    }

    pub(crate) fn record_ready(&self, generation: &str, pid: u32) -> anyhow::Result<()> {
        let mut journal = self.read_journal()?.context("restart journal missing")?;
        let restart = journal.restart.as_mut().context("restart record missing")?;
        if restart.generation != generation || restart.pid != Some(pid) {
            bail!("ready Home differs from the claimed generation");
        }
        restart.phase = match restart.phase {
            RestartPhase::CandidateRunning => RestartPhase::CandidateReady,
            RestartPhase::PreviousRunning => RestartPhase::PreviousReady,
            _ => bail!("restart process is not awaiting readiness"),
        };
        self.write_journal(&journal)
    }

    /// Caller first stops and reaps its exact candidate generation and acquires the host lock.
    pub(crate) fn restore_for_restart(&self) -> anyhow::Result<()> {
        let record = self.restart_record()?;
        if matches!(
            record.phase,
            RestartPhase::CandidateReady
                | RestartPhase::PreviousStartClaimed
                | RestartPhase::PreviousRunning
                | RestartPhase::PreviousReady
        ) {
            bail!(
                "restart restoration is unavailable after readiness or a previous-host start claim"
            );
        }
        self.recover_inner(true)
    }

    pub(crate) fn finish_restart(&self) -> anyhow::Result<()> {
        let journal = self.read_journal()?.context("restart journal missing")?;
        let restart = journal.restart.as_ref().context("restart record missing")?;
        let previous = match restart.phase {
            RestartPhase::CandidateReady => false,
            RestartPhase::PreviousReady => true,
            _ => bail!("retain rollback until the claimed Home is ready"),
        };
        for entry in &journal.entries {
            self.require_state(
                entry.id,
                if previous {
                    entry.original_sha256.as_deref()
                } else {
                    Some(&entry.staged_sha256)
                },
                if previous {
                    entry.original_mode
                } else {
                    Some(entry.staged_mode)
                },
            )?;
        }
        support::require_installed(self, &journal.support, previous)?;
        self.cleanup(&journal)
    }

    /// Normal setup needs the admitted candidate components while it refreshes support.
    /// Keep original backups and the committing journal until the final metadata save.
    pub(crate) fn activate_artifacts_for_support(&self) -> anyhow::Result<SupportActivation<'_>> {
        let mut journal = self
            .read_journal()?
            .context("prepared release journal missing")?;
        if !self.uses_consumed_layout()
            || journal.phase != Phase::Prepared
            || journal.restart.is_some()
        {
            bail!("support activation requires a prepared consumed CLI release");
        }
        self.validate_scratch(&journal)?;
        for entry in &journal.entries {
            self.require_state(
                entry.id,
                entry.original_sha256.as_deref(),
                entry.original_mode,
            )?;
        }
        journal.phase = Phase::Committing;
        self.write_journal(&journal)?;
        let result = (|| {
            for id in [ReleaseFile::RuntimeBinary, ReleaseFile::Components] {
                let entry = journal
                    .entries
                    .iter()
                    .find(|entry| entry.id == id)
                    .context("support activation requires all five release files")?;
                fs::rename(self.scratch(id, STAGE), &self.destinations[&id])?;
                sync_directory(self.destinations[&id].parent().unwrap())?;
                self.require_state(id, Some(&entry.staged_sha256), Some(entry.staged_mode))?;
            }
            Ok(())
        })();
        self.restore_on_error(result)?;
        Ok(SupportActivation {
            transaction: self,
            journal_sha256: sha256(&serde_json::to_vec(&journal)?),
        })
    }

    fn commit_with(
        &self,
        mut after_rename: impl FnMut(ReleaseFile) -> anyhow::Result<()>,
        after_activation: impl FnOnce() -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let mut journal = self
            .read_journal()?
            .context("prepared release journal missing")?;
        if !matches!(journal.phase, Phase::Prepared | Phase::Committing) {
            bail!("release transaction is not prepared");
        }
        self.validate_scratch(&journal)?;
        // A resumed writer can finish only a contiguous, admitted activation prefix.
        let mut original_suffix = false;
        for entry in &journal.entries {
            check_private_directory(self.scratch(entry.id, STAGE).parent().unwrap())?;
            check_private_directory(self.scratch(entry.id, ROLLBACK).parent().unwrap())?;
            let state = file_state(&self.destinations[&entry.id])?;
            let original = state_matches(
                &state,
                entry.original_sha256.as_deref(),
                entry.original_mode,
            );
            let candidate =
                state_matches(&state, Some(&entry.staged_sha256), Some(entry.staged_mode));
            if !(original || journal.phase == Phase::Committing && candidate && !original_suffix) {
                bail!("release activation prefix changed; retain journal for recovery");
            }
            if original && !candidate {
                original_suffix = true;
            }
            if !candidate {
                require_hash(&self.scratch(entry.id, STAGE), &entry.staged_sha256)?;
            }
            if let Some(original) = &entry.original_sha256 {
                require_hash(&self.scratch(entry.id, ROLLBACK), original)?;
            }
        }
        let result = (|| {
            journal.phase = Phase::Committing;
            self.write_journal(&journal)?;
            for entry in &journal.entries {
                self.check_parent(self.destinations[&entry.id].parent().unwrap(), false)?;
                if state_matches(
                    &file_state(&self.destinations[&entry.id])?,
                    Some(&entry.staged_sha256),
                    Some(entry.staged_mode),
                ) {
                    continue;
                }
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
            support::activate(self, &journal.support)?;
            after_activation()?;
            journal.phase = Phase::Committed;
            self.write_journal(&journal)
        })();
        self.restore_on_error(result)?;
        if journal.restart.is_some() {
            Ok(())
        } else {
            self.cleanup(&journal)
        }
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
        let Some(journal) = read_journal_file(&self.journal_path())? else {
            return Ok(None);
        };
        if journal_layout(&journal)? != self.layout {
            bail!("installation journal layout changed during recovery");
        }
        if journal.data_dir != self.data_dir
            || journal.binary_basename != self.binary.file_name().unwrap().to_str().unwrap()
            || journal.transaction_id.len() != 32
            || !journal
                .transaction_id
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
            || !ReleaseFile::complete_set(journal.entries.iter().map(|entry| entry.id))
        {
            bail!("installation journal identity is incompatible with this writer");
        }
        support::validate_entries(self, &journal.support)?;
        validate_restart_record(&journal)?;
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
                || (self.uses_consumed_layout()
                    && matches!(
                        entry.id,
                        ReleaseFile::ReleaseHead | ReleaseFile::ReleaseManifest
                    )
                    && (entry.staged_mode != 0o600
                        || entry.original_mode.is_some_and(|mode| mode != 0o600)))
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
        support::validate_scratch(self, journal)?;
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
        support::cleanup(self, journal)?;
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

pub(crate) fn validate_restart_plan(plan: &RestartPlan) -> anyhow::Result<()> {
    if plan.request_id.len() != 32
        || !plan.request_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        || [
            &plan.controller_sha256,
            &plan.launch_plan_sha256,
            &plan.support_sha256,
            &plan.previous_binary_sha256,
        ]
        .iter()
        .any(|hash| !valid_hash(hash))
        || semver::Version::parse(&plan.previous_version).is_err()
        || semver::Version::parse(&plan.candidate_version).is_err()
    {
        bail!("restart plan is invalid");
    }
    Ok(())
}

fn validate_restart_record(journal: &Journal) -> anyhow::Result<()> {
    let Some(restart) = &journal.restart else {
        return Ok(());
    };
    if !matches!(
        journal.schema.as_str(),
        "elastos.install-transaction/v2" | "elastos.install-transaction/v4"
    ) || journal.transaction_id.len() != 32
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
        || journal.entries.iter().any(|entry| {
            !valid_hash(&entry.staged_sha256)
                || entry
                    .original_sha256
                    .as_ref()
                    .is_none_or(|hash| !valid_hash(hash))
                || entry
                    .original_mode
                    .is_none_or(|mode| mode & !0o777 != 0 || mode & 0o022 != 0)
                || entry.staged_mode & !0o777 != 0
                || entry.staged_mode & 0o022 != 0
        })
    {
        bail!("restart journal release entries are invalid");
    }
    validate_restart_plan(&restart.plan)?;
    let (claimed, running, outer_valid) = match restart.phase {
        RestartPhase::CandidatePending => (
            false,
            false,
            matches!(
                journal.phase,
                Phase::Prepared | Phase::Committing | Phase::Committed
            ),
        ),
        RestartPhase::CandidateStartClaimed => (true, false, journal.phase == Phase::Committed),
        RestartPhase::CandidateRunning | RestartPhase::CandidateReady => {
            (true, true, journal.phase == Phase::Committed)
        }
        RestartPhase::Restoring | RestartPhase::Restored => {
            (false, false, journal.phase == Phase::Recovering)
        }
        RestartPhase::PreviousStartClaimed => (true, false, journal.phase == Phase::Recovering),
        RestartPhase::PreviousRunning | RestartPhase::PreviousReady => {
            (true, true, journal.phase == Phase::Recovering)
        }
    };
    let generation_valid = if claimed {
        restart.generation.len() == 32 && restart.generation.bytes().all(|b| b.is_ascii_hexdigit())
    } else {
        restart.generation.is_empty()
    };
    if !outer_valid
        || !generation_valid
        || running != restart.pid.is_some()
        || running != restart.process_start.is_some()
        || restart.pid == Some(0)
        || restart
            .process_start
            .as_ref()
            .is_some_and(|start| start.is_empty() || start.len() > 128)
        || journal
            .entries
            .iter()
            .any(|entry| entry.original_sha256.is_none())
        || journal
            .entries
            .iter()
            .find(|entry| entry.id == ReleaseFile::RuntimeBinary)
            .and_then(|entry| entry.original_sha256.as_deref())
            != Some(&restart.plan.previous_binary_sha256)
    {
        bail!("restart journal identity or phase is invalid");
    }
    Ok(())
}

/// A pending transaction admits only its controller's claimed host generation.
/// This read precedes host startup side effects; the controller retains the writer lock.
pub(crate) fn authorize_host_start(data_dir: &Path, binary: &Path) -> anyhow::Result<()> {
    authorize_host_start_with_generation(
        data_dir,
        binary,
        std::env::var("ELASTOS_UPDATE_GENERATION").ok().as_deref(),
        std::process::id(),
    )
}

pub(crate) fn authorize_host_start_with_generation(
    data_dir: &Path,
    binary: &Path,
    generation: Option<&str>,
    process_id: u32,
) -> anyhow::Result<()> {
    let Some(journal) = read_host_start_journal(data_dir, binary)? else {
        return Ok(());
    };
    if journal.restart.is_none() {
        bail!("{}", pending_home_recovery_hint(&journal));
    }
    let restart = journal.restart.context(
        "Home restart recovery is pending. Start the retained update controller with its receipt.",
    )?;
    validate_restart_plan(&restart.plan)?;
    let generation = generation.unwrap_or_default();
    if generation.len() != 32
        || generation != restart.generation
        || restart.pid.is_some_and(|pid| pid != process_id)
    {
        bail!("Home start differs from the controller's claimed generation. Start the retained update controller with its receipt.");
    }
    let previous = match restart.phase {
        RestartPhase::CandidateStartClaimed | RestartPhase::CandidateRunning => false,
        RestartPhase::PreviousStartClaimed | RestartPhase::PreviousRunning => true,
        _ => bail!("installation is not ready for a claimed Home start"),
    };
    let entry = journal
        .entries
        .iter()
        .find(|entry| entry.id == ReleaseFile::RuntimeBinary)
        .context("restart Runtime entry missing")?;
    let expected = if previous {
        entry.original_sha256.as_deref()
    } else {
        Some(entry.staged_sha256.as_str())
    };
    if !state_matches(
        &file_state(binary)?,
        expected,
        if previous {
            entry.original_mode
        } else {
            Some(entry.staged_mode)
        },
    ) {
        bail!("claimed Runtime binary changed before startup");
    }
    Ok(())
}

pub(crate) fn refuse_pending_home_start(data_dir: &Path, binary: &Path) -> anyhow::Result<()> {
    // Controller preflight already selects its installed binary and can repair a prior receipt.
    if let Some(journal) = read_host_start_journal_at(data_dir, binary)? {
        bail!("{}", pending_home_recovery_hint(&journal));
    }
    Ok(())
}

fn pending_home_recovery_hint(journal: &Journal) -> &'static str {
    if journal.restart.is_some() {
        "Home restart recovery is pending. Start the retained update controller with its receipt."
    } else if journal
        .entries
        .iter()
        .all(|entry| entry.id != ReleaseFile::Components)
    {
        "An interrupted installation requires recovery. Run install.sh again before starting Home."
    } else {
        "An interrupted command-line update requires recovery. Run `elastos update` again before starting Home."
    }
}

fn read_host_start_journal(data_dir: &Path, binary: &Path) -> anyhow::Result<Option<Journal>> {
    let installed = installed_binary(
        data_dir,
        crate::update_controller::installed_host_binary(data_dir)?,
    )?;
    // Installation ownership precedes the invoking binary's own recovery fence.
    for journal_binary in installed
        .as_deref()
        .into_iter()
        .chain(std::iter::once(binary))
    {
        let Some(journal) = read_host_start_journal_at(data_dir, journal_binary)? else {
            continue;
        };
        let owner = installed.as_deref().unwrap_or(binary);
        if fs::canonicalize(binary.parent().context("host binary parent missing")?)?
            != fs::canonicalize(owner.parent().context("installed binary parent missing")?)?
            || binary.file_name() != owner.file_name()
        {
            bail!(
                "Home start differs from the installation retained for recovery. {}",
                pending_home_recovery_hint(&journal)
            );
        }
        return Ok(Some(journal));
    }
    Ok(None)
}

fn read_host_start_journal_at(data_dir: &Path, binary: &Path) -> anyhow::Result<Option<Journal>> {
    let parent = binary.parent().context("host binary parent missing")?;
    let path = parent.join(JOURNAL);
    let Some(journal) = read_journal_file(&path)? else {
        return Ok(None);
    };
    validate_restart_record(&journal)?;
    if journal.data_dir != fs::canonicalize(data_dir)?
        || journal.binary_basename
            != binary
                .file_name()
                .context("host binary basename missing")?
                .to_string_lossy()
    {
        bail!("Installation recovery belongs to a different Home. Retain its files for operator repair.");
    }
    anyhow::ensure!(
        matches!(
            journal.schema.as_str(),
            "elastos.install-transaction/v1"
                | "elastos.install-transaction/v2"
                | "elastos.install-transaction/v3"
                | "elastos.install-transaction/v4"
        ),
        "Installation recovery format is unknown. Retain its files for operator repair."
    );
    Ok(Some(journal))
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
        return Err(DiskReserveError.into());
    }
    Ok(())
}

pub(crate) fn require_controller_disk_reserve(path: &Path, needed: u64) -> anyhow::Result<()> {
    let (total, available) = disk_space(path)?;
    require_disk_reserve(total, available, u128::from(needed))
}

#[cfg(test)]
#[path = "install_transaction/restart_tests.rs"]
mod restart_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn previous(id: ReleaseFile) -> Vec<u8> {
        if id == ReleaseFile::RuntimeBinary {
            b"#!/bin/sh\nprintf 'elastos 0.7.0\\n'\n".to_vec()
        } else if id == ReleaseFile::Sources {
            br#"{"schema":"elastos.trusted-sources/v1","sources":[{"name":"fixture","installed_version":"0.7.0"}]}"#.to_vec()
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
            let root_path = fs::canonicalize(root.path()).unwrap();
            let data = root_path.join("data");
            let binary = root_path.join("bin/elastos");
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

        fn snapshot(&self) -> BTreeMap<PathBuf, (u32, Option<Vec<u8>>)> {
            fn collect(path: &Path, out: &mut BTreeMap<PathBuf, (u32, Option<Vec<u8>>)>) {
                let metadata = fs::symlink_metadata(path).unwrap();
                let bytes = metadata.is_file().then(|| fs::read(path).unwrap());
                out.insert(path.to_path_buf(), (metadata.mode(), bytes));
                if metadata.is_dir() {
                    for entry in fs::read_dir(path).unwrap() {
                        collect(&entry.unwrap().path(), out);
                    }
                }
            }
            let mut snapshot = BTreeMap::new();
            collect(self._root.path(), &mut snapshot);
            snapshot
        }
    }

    fn candidate() -> [(ReleaseFile, &'static [u8]); 5] {
        ReleaseFile::ALL.map(|id| (id, candidate_bytes(id)))
    }

    fn candidate_bytes(id: ReleaseFile) -> &'static [u8] {
        if id == ReleaseFile::Sources {
            br#"{"schema":"elastos.trusted-sources/v1","sources":[{"name":"fixture","installed_version":"0.7.1"}]}"#
        } else {
            b"candidate bytes"
        }
    }

    #[test]
    fn cli_activation_admits_only_its_migration_before_release_commit() {
        let fixture = Fixture::new();
        let writer = fixture.writer();
        fixture.old_files(&writer, false);
        writer.prepare(&candidate()).unwrap();
        let activation = writer.activate_artifacts_for_support().unwrap();

        // Use the actual installed path, unlike a test runner outside this installation.
        let error = authorize_host_start_with_generation(
            writer.data_dir(),
            writer.binary_path(),
            None,
            std::process::id(),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("interrupted command-line update"));
        assert!(InstallationGuard::acquire(writer.binary.parent().unwrap()).is_err());

        let host =
            crate::host_lock::acquire_principal_root_update_lock(writer.data_dir(), &activation)
                .unwrap();
        assert_eq!(
            crate::host_lock::active_host_process(writer.data_dir())
                .unwrap()
                .unwrap()
                .role,
            "principal-root-upgrade"
        );
        assert!(crate::host_lock::acquire_principal_root_update_lock(
            writer.data_dir(),
            &activation,
        )
        .is_err());
        drop(host);

        let receipt = crate::api::auth_gateway::migrate_configured_principal_roots_for_update(
            writer.data_dir(),
            &writer.data_dir.join("backups/update-migration"),
            &activation,
        )
        .unwrap();
        assert_eq!(receipt.status, "already_ready");
        activation
            .authorize_principal_root_migration(writer.data_dir())
            .unwrap();
        writer
            .commit_checked(|| {
                assert!(activation
                    .authorize_principal_root_migration(writer.data_dir())
                    .is_err());
                Ok(())
            })
            .unwrap();
        assert!(activation
            .authorize_principal_root_migration(writer.data_dir())
            .is_err());
        authorize_host_start_with_generation(
            writer.data_dir(),
            writer.binary_path(),
            None,
            std::process::id(),
        )
        .unwrap();
    }

    #[test]
    fn cli_migration_refuses_changed_or_missing_activation_files_without_writes() {
        for id in ReleaseFile::ALL {
            for directory in [None, Some(STAGE), Some(ROLLBACK)] {
                if directory == Some(STAGE)
                    && matches!(id, ReleaseFile::RuntimeBinary | ReleaseFile::Components)
                {
                    continue;
                }
                for change in ["bytes", "mode", "missing"] {
                    let fixture = Fixture::new();
                    let writer = fixture.writer();
                    fixture.old_files(&writer, false);
                    writer.prepare(&candidate()).unwrap();
                    let activation = writer.activate_artifacts_for_support().unwrap();
                    let path = match directory {
                        Some(directory) => writer.scratch(id, directory),
                        None => writer.destinations[&id].clone(),
                    };
                    match change {
                        "bytes" => fs::write(&path, b"foreign bytes").unwrap(),
                        "mode" => {
                            fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap()
                        }
                        "missing" => fs::remove_file(&path).unwrap(),
                        _ => unreachable!(),
                    }
                    let before = fixture.snapshot();
                    assert!(
                        crate::host_lock::acquire_principal_root_update_lock(
                            writer.data_dir(),
                            &activation,
                        )
                        .is_err(),
                        "{id:?} {directory:?} {change}"
                    );
                    assert_eq!(fixture.snapshot(), before);
                }
            }
        }
    }

    #[test]
    fn cli_migration_refuses_foreign_identity_phase_and_custody_without_writes() {
        for change in [
            "staging",
            "prepared",
            "committed",
            "recovering",
            "schema",
            "transaction",
            "journal-home",
            "binary-name",
            "migration-home",
            "lock",
            "early-source",
            "retained-binary-stage",
            "foreign-scratch",
            "missing-journal",
            "rebound-stage",
            "rebound-prefix",
        ] {
            let fixture = Fixture::new();
            let writer = fixture.writer();
            fixture.old_files(&writer, false);
            writer.prepare(&candidate()).unwrap();
            let activation = writer.activate_artifacts_for_support().unwrap();
            let mut journal = writer.read_journal().unwrap().unwrap();
            let mut data_dir = writer.data_dir.clone();
            match change {
                "staging" => journal.phase = Phase::Staging,
                "prepared" => journal.phase = Phase::Prepared,
                "committed" => journal.phase = Phase::Committed,
                "recovering" => journal.phase = Phase::Recovering,
                "schema" => journal.schema = "elastos.install-transaction/v1".into(),
                "transaction" => journal.transaction_id = "f".repeat(32),
                "journal-home" => journal.data_dir = writer.data_dir.join("foreign"),
                "binary-name" => journal.binary_basename = "foreign-runtime".into(),
                "migration-home" => {
                    data_dir = writer.data_dir.join("foreign");
                    fs::create_dir(&data_dir).unwrap();
                }
                "lock" => {
                    let lock = writer.binary.parent().unwrap().join(INSTALL_LOCK);
                    fs::remove_file(&lock).unwrap();
                    write_new(&lock, b"replaced lock", 0o600).unwrap();
                }
                "early-source" => fs::rename(
                    writer.scratch(ReleaseFile::Sources, STAGE),
                    &writer.destinations[&ReleaseFile::Sources],
                )
                .unwrap(),
                "retained-binary-stage" => write_new(
                    &writer.scratch(ReleaseFile::RuntimeBinary, STAGE),
                    b"candidate bytes",
                    0o755,
                )
                .unwrap(),
                "foreign-scratch" => write_new(
                    &writer.binary.parent().unwrap().join(STAGE).join("foreign"),
                    b"owner file",
                    0o600,
                )
                .unwrap(),
                "missing-journal" => fs::remove_file(writer.journal_path()).unwrap(),
                "rebound-stage" | "rebound-prefix" => {
                    let id = if change == "rebound-stage" {
                        ReleaseFile::Sources
                    } else {
                        ReleaseFile::RuntimeBinary
                    };
                    let entry = journal
                        .entries
                        .iter_mut()
                        .find(|entry| entry.id == id)
                        .unwrap();
                    entry.staged_sha256 = sha256(b"foreign admitted bytes");
                    let path = if change == "rebound-stage" {
                        writer.scratch(id, STAGE)
                    } else {
                        writer.destinations[&id].clone()
                    };
                    fs::write(path, b"foreign admitted bytes").unwrap();
                }
                _ => unreachable!(),
            }
            if change != "missing-journal" {
                writer.write_journal(&journal).unwrap();
            }
            let before = fixture.snapshot();
            assert!(
                crate::host_lock::acquire_principal_root_update_lock(&data_dir, &activation)
                    .is_err(),
                "{change}"
            );
            assert_eq!(fixture.snapshot(), before);
        }
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
    fn installation_guard_releases_ownership_with_a_retained_descriptor() {
        use std::os::fd::AsRawFd;

        let fixture = Fixture::new();
        let parent = fs::canonicalize(fixture.binary.parent().unwrap()).unwrap();
        let guard = InstallationGuard::acquire(&parent).unwrap();
        let retained = guard._lock.try_clone().unwrap();
        let inode = retained.metadata().unwrap().ino();
        let flags = unsafe { libc::fcntl(retained.as_raw_fd(), libc::F_GETFD) };
        assert!(flags >= 0);
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
        assert!(InstallationGuard::acquire(&parent).is_err());
        let contender = OpenOptions::new()
            .read(true)
            .write(true)
            .open(parent.join(INSTALL_LOCK))
            .unwrap();
        assert_eq!(
            unsafe { libc::flock(contender.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            -1,
        );
        assert!(matches!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN
        ));

        drop(guard);
        let next_guard = InstallationGuard::acquire(&parent).unwrap();
        assert_eq!(retained.metadata().unwrap().ino(), inode);
        assert_eq!(
            fs::metadata(parent.join(INSTALL_LOCK)).unwrap().ino(),
            inode
        );
        assert!(InstallationGuard::acquire(&parent).is_err());
        drop(retained);
        assert!(InstallationGuard::acquire(&parent).is_err());
        drop(next_guard);
        let _guard = InstallationGuard::acquire(&parent).unwrap();
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
    fn installation_guard_requires_its_resolved_parent_for_binary_admission() {
        let fixture = Fixture::new();
        let parent = fs::canonicalize(fixture.binary.parent().unwrap()).unwrap();
        let alias = parent.with_file_name("bin-alias");
        let other = parent.with_file_name("other-bin");
        fs::create_dir(&other).unwrap();
        symlink(&parent, &alias).unwrap();
        let guard = InstallationGuard::acquire(&parent).unwrap();
        guard.require_binary(&parent.join("elastos")).unwrap();
        assert!(guard.require_binary(&alias.join("elastos")).is_err());
        assert!(guard.require_binary(&other.join("elastos")).is_err());
        fs::remove_file(&alias).unwrap();
        symlink(&other, &alias).unwrap();
        guard.require_binary(&parent.join("elastos")).unwrap();
        assert!(guard.require_binary(&alias.join("elastos")).is_err());

        let moved = parent.with_file_name("moved-bin");
        fs::rename(&parent, &moved).unwrap();
        symlink(&other, &parent).unwrap();
        assert!(guard.require_binary(&parent.join("elastos")).is_err());
        assert!(moved.join(INSTALL_LOCK).is_file());
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
                for (id, path) in &writer.destinations {
                    assert_eq!(fs::read(path).unwrap(), candidate_bytes(*id));
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
        for (id, destination) in &resumed.destinations {
            assert_eq!(fs::read(destination).unwrap(), candidate_bytes(*id));
        }
        resumed.require_empty_scratch().unwrap();
    }

    #[test]
    fn committed_cleanup_preserves_later_owner_bytes_and_modes() {
        assert_committed_cleanup_preserves_owner_edits(ReleaseLayout::Consumed);
    }

    #[test]
    fn legacy_committed_cleanup_preserves_publisher_owner_bytes_and_modes() {
        assert_committed_cleanup_preserves_owner_edits(ReleaseLayout::LegacyPublisher);
    }

    fn assert_committed_cleanup_preserves_owner_edits(layout: ReleaseLayout) {
        let fixture = Fixture::new();
        let writer = fixture.writer();
        fixture.old_files(&writer, false);
        fixture.commit_before_cleanup(&writer);
        if layout == ReleaseLayout::LegacyPublisher {
            let mut journal = writer.read_journal().unwrap().unwrap();
            let publisher = publisher_release_head_path(&fixture.data)
                .parent()
                .unwrap()
                .to_path_buf();
            writer
                .check_parent(publisher.parent().unwrap(), true)
                .unwrap();
            fs::rename(
                writer.destinations[&ReleaseFile::ReleaseHead]
                    .parent()
                    .unwrap(),
                publisher,
            )
            .unwrap();
            journal.schema = "elastos.install-transaction/v1".into();
            writer.write_journal(&journal).unwrap();
        }
        drop(writer);
        let writer = fixture.writer();
        assert_eq!(writer.layout, layout);
        let independent_metadata = if layout == ReleaseLayout::Consumed {
            [
                publisher_release_head_path(&fixture.data),
                publisher_release_manifest_path(&fixture.data),
            ]
        } else {
            [
                installation_release_head_path(&fixture.data),
                installation_release_manifest_path(&fixture.data),
            ]
        };
        for path in &independent_metadata {
            writer.check_parent(path.parent().unwrap(), true).unwrap();
            write_new(path, b"independent signed input", 0o600).unwrap();
        }
        fs::remove_file(writer.scratch(ReleaseFile::RuntimeBinary, ROLLBACK)).unwrap();
        let modes: BTreeMap<_, _> = ReleaseFile::ALL
            .map(|id| {
                let mode = match (id, layout) {
                    (ReleaseFile::RuntimeBinary, _) => 0o750,
                    (
                        ReleaseFile::ReleaseHead | ReleaseFile::ReleaseManifest,
                        ReleaseLayout::Consumed,
                    ) => 0o600,
                    _ => 0o640,
                };
                (id, mode)
            })
            .into_iter()
            .collect();
        for id in ReleaseFile::ALL {
            let destination = &writer.destinations[&id];
            fs::write(destination, format!("later owner {}", id.name())).unwrap();
            fs::set_permissions(destination, fs::Permissions::from_mode(modes[&id])).unwrap();
        }
        drop(writer);
        let resumed = fixture.writer();
        resumed.recover().unwrap();
        resumed.require_empty_scratch().unwrap();
        resumed.recover().unwrap();
        for metadata in [
            publisher_release_head_path(&fixture.data),
            installation_release_head_path(&fixture.data),
        ] {
            for directory in [STAGE, ROLLBACK] {
                assert_eq!(
                    fs::symlink_metadata(metadata.parent().unwrap().join(directory))
                        .unwrap_err()
                        .kind(),
                    std::io::ErrorKind::NotFound,
                );
            }
        }
        assert!(!resumed.journal_path().exists());
        for id in ReleaseFile::ALL {
            let destination = &resumed.destinations[&id];
            assert_eq!(
                fs::read(destination).unwrap(),
                format!("later owner {}", id.name()).as_bytes()
            );
            assert_eq!(
                fs::metadata(destination).unwrap().mode() & 0o777,
                modes[&id]
            );
        }
        for path in independent_metadata {
            assert_eq!(fs::read(&path).unwrap(), b"independent signed input");
            assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
        }
        assert_eq!(
            fs::read(fixture.data.join("owner-data")).unwrap(),
            b"data written by owner"
        );
    }

    #[test]
    fn committed_cleanup_refuses_nonprivate_consumed_metadata_and_preserves_custody() {
        for (id, mode) in [ReleaseFile::ReleaseHead, ReleaseFile::ReleaseManifest]
            .into_iter()
            .flat_map(|id| [0o640, 0o644].into_iter().map(move |mode| (id, mode)))
        {
            let fixture = Fixture::new();
            let writer = fixture.writer();
            fixture.old_files(&writer, false);
            fixture.commit_before_cleanup(&writer);
            let destination = &writer.destinations[&id];
            fs::write(destination, b"preserve later owner input").unwrap();
            fs::set_permissions(destination, fs::Permissions::from_mode(mode)).unwrap();
            let paths: Vec<_> = writer
                .destinations
                .values()
                .cloned()
                .chain(ReleaseFile::ALL.map(|id| writer.scratch(id, ROLLBACK)))
                .chain(std::iter::once(writer.journal_path()))
                .collect();
            let before: Vec<_> = paths
                .iter()
                .map(|path| (fs::read(path).unwrap(), fs::metadata(path).unwrap().mode()))
                .collect();
            let error = writer.recover().unwrap_err();
            assert!(error.to_string().contains("owner-only files"));
            drop(writer);
            assert!(InstallTransaction::acquire(&fixture.data, &fixture.binary).is_err());
            for (path, (bytes, mode)) in paths.iter().zip(before) {
                assert_eq!(fs::read(path).unwrap(), bytes);
                assert_eq!(fs::metadata(path).unwrap().mode(), mode);
            }
            assert_eq!(
                fs::read(fixture.data.join("owner-data")).unwrap(),
                b"data written by owner"
            );
        }
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
