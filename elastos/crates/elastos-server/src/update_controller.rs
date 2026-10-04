//! Signed installed Home controller. The controller survives replacement of Runtime.
//! Its private receipt binds the signed controller and the retained launch settings.

pub(crate) mod child;

pub fn watch_parent() -> anyhow::Result<()> {
    child::watch_parent()
}

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::install_transaction::{InstallTransaction, RestartPhase, RestartPlan, RestartRecord};
use crate::sources::{load_trusted_sources, TrustedSource};

const DIRECTORY: &str = "update-controller";
const RECEIPT: &str = "receipt.json";
const STATUS: &str = "status.json";
const REQUEST: &str = "request.json";
const ACTIVE_REQUEST: &str = "active-request.json";
const LEASE_ENV: &str = "ELASTOS_UPDATE_CONTROLLER_LEASE";
const HOST_ENV: &str = "ELASTOS_UPDATE_CONTROLLER_HOST";
const MAX_PRIVATE_JSON: u64 = 256 * 1024;
const INITIAL_READY_TIMEOUT: Duration = Duration::from_secs(120);
const READY_TIMEOUT: Duration = Duration::from_secs(30);

// Home reads provider and Browser settings from its installed private config files.
// Persist the paths, desktop session, locale, and Runtime bindings needed to restart this Home.
// Wallet's existing API key binding remains in the same owner-only receipt.
const LAUNCH_ENVIRONMENT: &[&str] = &[
    "HOME",
    "XDG_DATA_HOME",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "XDG_RUNTIME_DIR",
    "PATH",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TMPDIR",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "DBUS_SESSION_BUS_ADDRESS",
    "XAUTHORITY",
    "BROWSER",
    "WSL_INTEROP",
    "WSL_DISTRO_NAME",
    "COINGECKO_DEMO_API_KEY",
    "ELASTOS_CAPSULE_BIN_DIR",
    "ELASTOS_IPFS_KUBO_PATH",
    "ELASTOS_IPFS_PROVIDER_BIN",
    "ELASTOS_POLICY_FILE",
    "ELASTOS_CARRIER_NETWORK",
    "ELASTOS_CARRIER_MDNS",
    "ELASTOS_RELAY_URL",
    "ELASTOS_HOME_LAUNCH_TRUSTED_SIGNER_DID",
    "ELASTOS_HOME_LAUNCH_TRUSTED_AUTH_DATA_DIR",
    "ELASTOS_HOSTED_HTTPS_OWNER_DATA_DIR",
    "ELASTOS_HOME_CLI_AUTH_CONTEXT_PRINCIPAL_ID",
    "ELASTOS_HOME_CLI_AUTH_CONTEXT_SESSION_ID",
    "ELASTOS_HOME_CLI_AUTH_CONTEXT_PROOF_BINDING_ID",
    "ELASTOS_HOME_CLI_AUTH_CONTEXT_GRANT_ID",
    "ELASTOS_HOME_CLI_GATEWAY_API_URL",
    "ELASTOS_HOME_CLI_TERMINAL_PROGRAM",
    "ELASTOS_HOME_CLI_TERMINAL_ARGS_JSON",
    "ELASTOS_BROWSER_MAX_ACTIVE_SESSIONS",
    "ELASTOS_BROWSER_MAX_SESSIONS_PER_PRINCIPAL",
    "ELASTOS_QUIET_RUNTIME_NOTICES",
];

fn allowed_launch_key(key: &std::ffi::OsStr) -> bool {
    LAUNCH_ENVIRONMENT
        .iter()
        .any(|allowed| key == std::ffi::OsStr::new(allowed))
}

fn readiness_budget(restarting: bool) -> Duration {
    if restarting {
        READY_TIMEOUT
    } else {
        INITIAL_READY_TIMEOUT
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchPlan {
    args: Vec<String>,
    environment: Vec<(String, String)>,
    cwd: String,
}

impl LaunchPlan {
    fn capture() -> Result<Self> {
        Ok(Self::capture_environment(
            std::env::vars_os(),
            std::env::current_dir()?,
        ))
    }

    fn capture_environment(
        environment: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
        cwd: PathBuf,
    ) -> Self {
        let encode = |value: &std::ffi::OsStr| BASE64.encode(value.as_bytes());
        Self {
            args: vec![
                encode(std::ffi::OsStr::new("home")),
                encode(std::ffi::OsStr::new("--browser")),
            ],
            environment: environment
                .into_iter()
                .filter(|(key, _)| allowed_launch_key(key))
                .map(|(key, value)| (encode(&key), encode(&value)))
                .collect(),
            cwd: encode(cwd.as_os_str()),
        }
    }

    fn command(
        &self,
        binary: &Path,
        generation: &str,
        restart: bool,
    ) -> Result<tokio::process::Command> {
        let decode = |value: &str| -> Result<std::ffi::OsString> {
            Ok(std::ffi::OsString::from_vec(BASE64.decode(value)?))
        };
        let mut command = tokio::process::Command::new(binary);
        command
            .env_clear()
            .current_dir(decode(&self.cwd)?)
            .stdin(Stdio::null());
        for arg in &self.args {
            command.arg(decode(arg)?);
        }
        for (key, value) in &self.environment {
            let key = decode(key)?;
            if allowed_launch_key(&key) {
                command.env(key, decode(value)?);
            }
        }
        command
            .env(HOST_ENV, "1")
            .env("ELASTOS_UPDATE_GENERATION", generation)
            .env("ELASTOS_UPDATE_RESTART", if restart { "1" } else { "0" });
        Ok(command)
    }

    fn sha256(&self) -> Result<String> {
        Ok(digest(&serde_json::to_vec(self)?))
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema: String,
    data_dir: PathBuf,
    binary: PathBuf,
    controller: PathBuf,
    controller_sha256: String,
    signed_controller_release: String,
    trusted_source: TrustedSource,
    launch: LaunchPlan,
    launch_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateRequest {
    pub id: String,
    pub source_name: String,
    pub channel: String,
    pub publisher_did: String,
    pub current_version: String,
    pub new_version: String,
    pub head_cid: String,
    pub release_cid: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateStatus {
    pub id: Option<String>,
    pub phase: String,
    pub current_version: String,
    pub new_version: Option<String>,
    pub message: String,
    pub controller_pid: u32,
    pub controller_start: String,
    pub host_pid: Option<u32>,
    pub generation: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerActionReceipt {
    schema: String,
    effect_id: String,
    request_sha256: String,
    request: UpdateRequest,
    queued: bool,
}

fn owner_action_guard(data_dir: &Path) -> Result<(PathBuf, File)> {
    let data_dir = fs::canonicalize(data_dir)?;
    let directory = controller_directory(&data_dir)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join("owner-action.lock"))?;
    check_private_file(&lock)?;
    anyhow::ensure!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "Another Home owner action is in progress."
    );
    Ok((directory, lock))
}

fn check_owner_effect(
    receipt: &OwnerActionReceipt,
    request: &UpdateRequest,
    effect_id: &str,
    request_sha256: &str,
) -> Result<()> {
    anyhow::ensure!(
        receipt.schema == "elastos.home-update-owner/v1"
            && receipt.effect_id == effect_id
            && receipt.request_sha256 == request_sha256
            && receipt.request == *request,
        "Home approval intent changed."
    );
    Ok(())
}

/// Called only after the passkey helper verifies the complete approval binding,
/// and before it writes the consumed marker. Thus both first and later actions
/// retain a recoverable intent through cancellation or a crash before queueing.
pub(crate) fn reserve_owner_update(
    data_dir: &Path,
    request: &UpdateRequest,
    effect_id: &str,
    request_sha256: &str,
    recovered: bool,
) -> Result<()> {
    anyhow::ensure!(
        !effect_id.is_empty()
            && effect_id.len() <= 256
            && !effect_id.chars().any(char::is_control)
            && valid_hash(request_sha256),
        "Invalid Home approval effect."
    );
    let (directory, _lock) = owner_action_guard(data_dir)?;
    let path = directory.join("owner-action.json");
    if path_present(&path)? {
        let previous: OwnerActionReceipt = read_private_json(&path)?;
        anyhow::ensure!(
            previous.schema == "elastos.home-update-owner/v1",
            "Unknown Home approval receipt."
        );
        if previous.effect_id == effect_id {
            return check_owner_effect(&previous, request, effect_id, request_sha256);
        }
        anyhow::ensure!(
            !recovered,
            "Home approval effect belongs to an earlier action."
        );
        anyhow::ensure!(
            !path_present(&directory.join(REQUEST))?
                && !path_present(&directory.join(ACTIVE_REQUEST))?,
            "Another Home update is pending."
        );
    } else {
        anyhow::ensure!(!recovered, "Home approval effect has no recovery receipt.");
    }
    write_private(
        &path,
        &OwnerActionReceipt {
            schema: "elastos.home-update-owner/v1".into(),
            effect_id: effect_id.into(),
            request_sha256: request_sha256.into(),
            request: request.clone(),
            queued: false,
        },
    )
}

pub(crate) fn owner_update_is_queued(
    data_dir: &Path,
    request: &UpdateRequest,
    effect_id: &str,
    request_sha256: &str,
) -> Result<bool> {
    let directory = data_dir.join(DIRECTORY);
    let receipt: OwnerActionReceipt = read_private_json(&directory.join("owner-action.json"))?;
    check_owner_effect(&receipt, request, effect_id, request_sha256)?;
    if receipt.queued {
        return Ok(true);
    }
    // Queue creation and marking its receipt are separate durable writes. The
    // controller may consume the request between them; preserve that exact effect.
    let mut dispatched = false;
    for name in [REQUEST, ACTIVE_REQUEST] {
        let path = directory.join(name);
        if path_present(&path)? {
            let existing: UpdateRequest = read_private_json(&path)?;
            anyhow::ensure!(existing == *request, "Another update identity is pending.");
            dispatched = true;
        }
    }
    if dispatched {
        return Ok(true);
    }
    if let Some(status) = status(data_dir)? {
        if status.id.as_deref() == Some(request.id.as_str()) {
            anyhow::ensure!(
                status.new_version.as_deref() == Some(request.new_version.as_str())
                    && (status.current_version == request.current_version
                        || status.current_version == request.new_version)
                    && matches!(
                        status.phase.as_str(),
                        "staging" | "restarting" | "updated" | "restored" | "failed"
                    ),
                "Retained update result differs from the approved intent."
            );
            return Ok(true);
        }
    }
    Ok(false)
}

/// Queue only the intent reserved before approval consumption; an exact
/// lost-response retry returns the retained result without writing a second request.
pub(crate) fn queue_owner_update(
    data_dir: &Path,
    request: UpdateRequest,
    effect_id: &str,
    request_sha256: &str,
) -> Result<()> {
    let (directory, _lock) = owner_action_guard(data_dir)?;
    let path = directory.join("owner-action.json");
    let mut receipt: OwnerActionReceipt = read_private_json(&path)?;
    check_owner_effect(&receipt, &request, effect_id, request_sha256)?;
    if receipt.queued {
        return Ok(());
    }
    queue_update(data_dir, request)?;
    receipt.queued = true;
    write_private(&path, &receipt)
}

struct ControllerBootstrap {
    directory: PathBuf,
    lease: File,
    writer: crate::install_transaction::InstallationGuard,
    signed: Vec<u8>,
    expected: String,
}

fn bootstrap_controller(
    data: &Path,
    binary: &Path,
    source: &TrustedSource,
    directory: impl FnOnce(&Path) -> Result<PathBuf>,
    lease: impl FnOnce(&Path) -> Result<File>,
    writer: impl FnOnce(&Path) -> Result<crate::install_transaction::InstallationGuard>,
) -> Result<Option<ControllerBootstrap>> {
    crate::install_transaction::refuse_pending_home_start(data, binary)?;
    crate::installed_release::read_without_migration(data, binary, source)?;
    let result = (|| {
        let writer = writer(
            binary
                .parent()
                .context("installed Runtime parent missing")?,
        )?;
        let directory = directory(data)?;
        let lease = lease(&directory)?;
        crate::install_transaction::refuse_pending_home_start(data, binary)?;
        let installed = admit_home_inputs(data, binary, source, &writer, &directory)?;
        Ok(ControllerBootstrap {
            directory,
            lease,
            writer,
            signed: installed.release,
            expected: installed.binary_sha256,
        })
    })();
    match result {
        Ok(bootstrap) => Ok(Some(bootstrap)),
        Err(error) if controller_space_error(&error) => {
            crate::install_transaction::refuse_pending_home_start(data, binary)?;
            if first_start_without_controller_authority(data)? {
                Ok(None)
            } else {
                Err(error)
            }
        }
        Err(error) => Err(error),
    }
}

// Ordinary Home may reuse its exact signed controller when only metadata migration
// lacks space. Updates still require migration through load_or_migrate.
fn admit_home_inputs(
    data: &Path,
    binary: &Path,
    source: &TrustedSource,
    writer: &crate::install_transaction::InstallationGuard,
    directory: &Path,
) -> Result<crate::installed_release::InstalledRelease> {
    reuse_home_inputs_after_space_refusal(
        data,
        binary,
        source,
        directory,
        crate::installed_release::load_or_migrate(data, binary, source, writer),
    )
}

fn reuse_home_inputs_after_space_refusal(
    data: &Path,
    binary: &Path,
    source: &TrustedSource,
    directory: &Path,
    result: Result<crate::installed_release::InstalledRelease>,
) -> Result<crate::installed_release::InstalledRelease> {
    match result {
        Err(error) if controller_space_error(&error) => {
            let installed = crate::installed_release::read_without_migration(data, binary, source)?;
            if prepare_controller(directory, binary, &installed.binary_sha256, |_, _| {
                Err(std::io::Error::from_raw_os_error(libc::ENOSPC).into())
            })? {
                Ok(installed)
            } else {
                Err(error)
            }
        }
        result => result,
    }
}

fn first_start_without_controller_authority(data: &Path) -> Result<bool> {
    let directory = data.join(DIRECTORY);
    if !path_present(&directory)? {
        return Ok(true);
    }
    check_controller_directory(&directory)?;
    let mut lease = None;
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        if entry.file_name() != "controller.lock" {
            return Ok(false);
        }
        // An empty safe lock carries no receipt authority, but an active lease
        // still owns startup even before its first controller copy.
        lease = Some(acquire_lease(&directory)?);
    }
    drop(lease);
    Ok(true)
}

/// Used by the existing Home browser entry. Source Homes keep their current launcher.
pub fn enter_browser_home() -> Result<()> {
    if std::env::var(HOST_ENV).as_deref() == Ok("1") {
        return Ok(());
    }
    let data = crate::sources::default_data_dir();
    let Some(source) = load_trusted_sources(&data)?.default_source().cloned() else {
        return Ok(());
    };
    let current = std::env::current_exe()?;
    if source.install_path.is_empty() || Path::new(&source.install_path) != current {
        return Ok(());
    }
    let data = fs::canonicalize(data)?;
    let binary = fs::canonicalize(current)?;
    let Some(ControllerBootstrap {
        directory,
        lease,
        writer,
        signed,
        expected,
    }) = bootstrap_controller(
        &data,
        &binary,
        &source,
        controller_directory,
        acquire_lease,
        crate::install_transaction::InstallationGuard::acquire,
    )?
    else {
        eprintln!("Home will open. Free disk space before updating.");
        return Ok(());
    };
    let controller = directory.join("runtime");
    if !prepare_controller(&directory, &binary, &expected, |path, needed| {
        crate::install_transaction::require_controller_disk_reserve(path, needed)
    })? {
        eprintln!("Home will open. Free disk space before updating.");
        return Ok(());
    }
    let launch = LaunchPlan::capture()?;
    let receipt = Receipt {
        schema: "elastos.update-controller/v1".into(),
        data_dir: data,
        binary,
        controller: controller.clone(),
        controller_sha256: expected,
        signed_controller_release: BASE64.encode(signed),
        trusted_source: source,
        launch_sha256: launch.sha256()?,
        launch,
    };
    let path = directory.join(RECEIPT);
    if let Err(error) = write_private(&path, &receipt) {
        if controller_space_error(&error) {
            eprintln!("Home will open. Free disk space before updating.");
            return Ok(());
        }
        return Err(error);
    }
    drop(writer);
    // Exec keeps the terminal's foreground process and the same lease description.
    let inherited = unsafe { libc::fcntl(lease.as_raw_fd(), libc::F_DUPFD, 3) };
    if inherited < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let error = std::process::Command::new(controller)
        .args(["__update-controller", "--receipt"])
        .arg(path)
        .env(LEASE_ENV, inherited.to_string())
        .exec();
    unsafe {
        libc::close(inherited);
    }
    Err(error.into())
}

pub async fn run(receipt_path: PathBuf) -> Result<()> {
    let receipt: Receipt = read_private_json(&receipt_path)?;
    let directory = controller_directory(&receipt.data_dir)?;
    anyhow::ensure!(
        receipt_path == directory.join(RECEIPT),
        "controller receipt path differs from its data root"
    );
    let _lease = inherited_or_new_lease(&directory)?;
    validate_receipt(&receipt)?;
    let mut controller = Controller {
        receipt,
        directory,
        child: None,
        request: None,
        previous_binary_sha256: String::new(),
        previous_version: String::new(),
        generation: String::new(),
        host_ready: false,
        carrier: None,
        carrier_close: None,
    };
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let result = tokio::select! {
        result = async {
            controller.reconcile_pending().await?;
            controller.complete_reconciliation().await?;
            controller.serve().await
        } => result,
        _ = interrupt.recv() => Ok(()),
        _ = term.recv() => Ok(()),
    };
    let closed = controller.close_carrier().await;
    let stopped = controller.stop_child().await;
    result.and(closed).and(stopped)
}

pub fn status(data_dir: &Path) -> Result<Option<UpdateStatus>> {
    let path = data_dir.join(DIRECTORY).join(STATUS);
    if !path_present(&path)? {
        return Ok(None);
    }
    let status: UpdateStatus = read_private_json(&path)?;
    if process_start(status.controller_pid).as_deref() != Some(status.controller_start.as_str()) {
        return Ok(None);
    }
    Ok(Some(status))
}

pub(crate) fn has_queued_update(data_dir: &Path) -> Result<bool> {
    let directory = data_dir.join(DIRECTORY);
    Ok(path_present(&directory.join(REQUEST))? || path_present(&directory.join(ACTIVE_REQUEST))?)
}

/// The existing owner/passkey action supplies this exact signed release choice.
pub fn queue_update(data_dir: &Path, request: UpdateRequest) -> Result<()> {
    // Only the owner-effect recovery path uses a repeated request identity.
    for name in [REQUEST, ACTIVE_REQUEST] {
        let path = data_dir.join(DIRECTORY).join(name);
        if path_present(&path)? {
            let existing: UpdateRequest = read_private_json(&path)?;
            anyhow::ensure!(
                existing == request,
                "Another update is queued. Wait for it to finish."
            );
            return Ok(());
        }
    }
    let status = status(data_dir)?
        .context("Start Home with the signed installed Runtime before updating.")?;
    if status.id.as_deref() == Some(request.id.as_str()) {
        let owner: OwnerActionReceipt =
            read_private_json(&data_dir.join(DIRECTORY).join("owner-action.json"))?;
        anyhow::ensure!(
            owner.schema == "elastos.home-update-owner/v1"
                && owner.request == request
                && matches!(status.phase.as_str(), "updated" | "restored" | "failed")
                && status.new_version.as_deref() == Some(request.new_version.as_str())
                && (status.current_version == request.current_version
                    || status.current_version == request.new_version),
            "Update recovery identity changed."
        );
        return Ok(());
    }
    anyhow::ensure!(
        matches!(
            status.phase.as_str(),
            "ready" | "updated" | "restored" | "failed"
        ),
        "A Home update is already in progress."
    );
    anyhow::ensure!(
        request.id.len() == 32 && request.id.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid update request identity"
    );
    anyhow::ensure!(
        request.current_version == status.current_version,
        "Installed release changed. Check the update again."
    );
    anyhow::ensure!(
        crate::update::compare_release_versions(&request.current_version, &request.new_version)?
            == std::cmp::Ordering::Greater,
        "update is not newer"
    );
    for cid in [&request.head_cid, &request.release_cid] {
        cid::Cid::try_from(cid.as_str()).context("invalid signed update CID")?;
    }
    let source = installed_source(data_dir)?;
    anyhow::ensure!(
        !path_present(&data_dir.join(DIRECTORY).join(ACTIVE_REQUEST))?
            && !InstallTransaction::has_pending_recovery(Path::new(&source.install_path)),
        "An earlier update requires recovery. Start the retained update controller."
    );
    let path = data_dir.join(DIRECTORY).join(REQUEST);
    let bytes = serde_json::to_vec(&request)?;
    write_new_private(&path, &bytes, 0o600)
        .context("Another update is queued. Wait for it to finish.")?;
    File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}

fn initial_ready_result(
    request: Option<&UpdateRequest>,
    installed_version: &str,
) -> Result<(&'static str, &'static str)> {
    let Some(request) = request else {
        return Ok(("ready", "Home is ready."));
    };
    if installed_version == request.new_version {
        Ok(("updated", "The update is complete. Home is ready."))
    } else if installed_version == request.current_version {
        Ok((
            "restored",
            "The previous Runtime is ready. Check the update and approve it again.",
        ))
    } else {
        bail!("Recovered Runtime version differs from the approved update.")
    }
}

struct Controller {
    receipt: Receipt,
    directory: PathBuf,
    child: Option<child::OwnedChild>,
    request: Option<UpdateRequest>,
    previous_binary_sha256: String,
    previous_version: String,
    generation: String,
    host_ready: bool,
    carrier: Option<Arc<crate::carrier::CarrierClient>>,
    carrier_close: Option<tokio::task::JoinHandle<()>>,
}

impl Controller {
    async fn close_carrier(&mut self) -> Result<()> {
        if self.carrier_close.is_none() {
            if let Some(client) = self.carrier.take() {
                // Keep the same drain task when a signal cancels apply's await.
                self.carrier_close = Some(tokio::spawn(async move { client.close().await }));
            }
        }
        if let Some(task) = self.carrier_close.as_mut() {
            let result = task.await.context("Carrier cleanup task failed");
            self.carrier_close = None;
            result?;
        }
        Ok(())
    }

    fn finish_request(&mut self) -> Result<()> {
        let path = self.directory.join(ACTIVE_REQUEST);
        if path_present(&path)? {
            let active: UpdateRequest = read_private_json(&path)?;
            anyhow::ensure!(
                self.request
                    .as_ref()
                    .is_some_and(|request| request.id == active.id),
                "Consumed update request identity changed."
            );
            fs::remove_file(path)?;
            File::open(&self.directory)?.sync_all()?;
        }
        Ok(())
    }
    async fn complete_reconciliation(&mut self) -> Result<()> {
        if self.child.is_none() {
            self.start_initial().await
        } else {
            self.publish_ready_result()
        }
    }

    fn publish_ready_result(&mut self) -> Result<()> {
        anyhow::ensure!(
            self.child.is_some() && self.host_ready,
            "Recovered Home is not ready."
        );
        let source = installed_source(&self.receipt.data_dir)?;
        let (phase, message) =
            initial_ready_result(self.request.as_ref(), &source.installed_version)?;
        self.previous_version = source.installed_version;
        self.publish(phase, message)
    }

    async fn start_initial(&mut self) -> Result<()> {
        let source = installed_source(&self.receipt.data_dir)?;
        let writer = crate::install_transaction::InstallationGuard::acquire(
            self.receipt
                .binary
                .parent()
                .context("installed Runtime parent missing")?,
        )?;
        let installed = admit_home_inputs(
            &self.receipt.data_dir,
            &self.receipt.binary,
            &source,
            &writer,
            &self.directory,
        )?;
        let expected = installed.binary_sha256;
        drop(writer);
        let generation = hex::encode(rand::random::<[u8; 16]>());
        async {
            self.spawn(&generation, false)?;
            self.wait_ready(&generation, &source.installed_version, &expected, false)
                .await
        }
        .await
        .with_context(|| {
            format!(
                "Home could not start. Check the startup log at {} before starting Home again.",
                self.directory.join("runtime.log").display()
            )
        })?;
        self.publish_ready_result()?;
        println!("Home: http://localhost:8090/home/");
        println!("Keep this terminal open. Press Ctrl+C to stop Home.");
        Ok(())
    }

    fn spawn(&mut self, generation: &str, restarting: bool) -> Result<()> {
        anyhow::ensure!(
            self.child.is_none(),
            "controller already owns a Home process"
        );
        let log = open_private_log(&self.directory.join("runtime.log"))?;
        let mut command =
            self.receipt
                .launch
                .command(&self.receipt.binary, generation, restarting)?;
        command.stdout(log.try_clone()?).stderr(log);
        self.child = Some(child::OwnedChild::spawn(&mut command)?);
        self.generation = generation.into();
        self.host_ready = false;
        Ok(())
    }

    async fn wait_ready(
        &mut self,
        generation: &str,
        version: &str,
        binary_sha256: &str,
        restarting: bool,
    ) -> Result<()> {
        let home_sha256 = home_digest(&self.receipt.data_dir)?;
        let child = self.child.as_ref().context("controller host missing")?;
        let pid = child.pid();
        let start = process_start(pid).context("Home exited before readiness")?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()?;
        let budget = readiness_budget(restarting);
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            if child.observed_exit()?.is_some() {
                bail!("Home exited before it became ready.");
            }
            if process_start(pid).as_deref() != Some(start.as_str()) {
                bail!("Home process generation changed.");
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if tokio::time::timeout(
                remaining,
                prove_ready(
                    &client,
                    &self.receipt.data_dir,
                    pid,
                    generation,
                    version,
                    binary_sha256,
                    &home_sha256,
                ),
            )
            .await
            .is_ok_and(|proof| proof.is_ok())
                && child.observed_exit()?.is_none()
                && process_start(pid).as_deref() == Some(start.as_str())
            {
                self.host_ready = true;
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                bail!(
                    "Home did not become ready within {} seconds.",
                    budget.as_secs()
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn stop_child(&mut self) -> Result<()> {
        // Stopping retires readiness even when later custody cleanup must be retried.
        self.host_ready = false;
        let owner_result = self
            .child
            .as_ref()
            .map(|child| {
                crate::runtime_control::gateway_children::Owner::read_for_generation(
                    &self.receipt.data_dir,
                    child.pid(),
                    &self.generation,
                )
            })
            .transpose();
        if let Some(child) = self.child.as_mut() {
            child.stop().await?;
        }
        let owner = owner_result?.flatten();
        crate::runtime_control::gateway_children::shutdown(owner).await?;
        crate::runtime_control::gateway_children::refuse_unreconciled_groups(
            &self.receipt.data_dir,
        )?;
        anyhow::ensure!(
            crate::host_lock::active_host_process(&self.receipt.data_dir)?.is_none(),
            "A Runtime still owns Home; retain recovery files."
        );
        self.child = None;
        self.generation.clear();
        Ok(())
    }

    fn publish(&self, phase: &str, message: &str) -> Result<()> {
        let source = installed_source(&self.receipt.data_dir)?;
        write_private(
            &self.directory.join(STATUS),
            &UpdateStatus {
                id: self.request.as_ref().map(|request| request.id.clone()),
                phase: phase.into(),
                current_version: source.installed_version,
                new_version: self
                    .request
                    .as_ref()
                    .map(|request| request.new_version.clone()),
                message: message.into(),
                controller_pid: std::process::id(),
                controller_start: process_start(std::process::id())
                    .context("controller identity unavailable")?,
                host_pid: self.child.as_ref().map(|child| child.pid()),
                generation: self.generation.clone(),
            },
        )
    }

    fn publish_apply_result(&self, result: &Result<()>) -> Result<()> {
        if result.is_ok() {
            self.publish("updated", "Home is up to date.")
        } else if self.host_ready && !InstallTransaction::has_pending_recovery(&self.receipt.binary)
        {
            self.publish("restored", "The update could not start. Your previous release is ready. Check the update again.")
        } else {
            self.publish(
                "failed",
                "Home could not restart. Run the retained update controller to recover.",
            )
        }
    }

    async fn serve(&mut self) -> Result<()> {
        loop {
            if self
                .child
                .as_ref()
                .context("Home host missing")?
                .observed_exit()?
                .is_some()
            {
                bail!("Home stopped. Start Home again.");
            }
            let path = self.directory.join(REQUEST);
            if path_present(&path)? {
                let request: UpdateRequest = read_private_json(&path)?;
                anyhow::ensure!(
                    !path_present(&self.directory.join(ACTIVE_REQUEST))?,
                    "An earlier update requires recovery."
                );
                fs::rename(&path, self.directory.join(ACTIVE_REQUEST))?;
                File::open(&self.directory)?.sync_all()?;
                self.request = Some(request);
                self.publish("staging", "Checking the signed update.")?;
                let result = self.apply().await;
                self.publish_apply_result(&result)?;
                if InstallTransaction::has_pending_recovery(&self.receipt.binary)
                    || !self.host_ready
                {
                    return result;
                }
                self.finish_request()?;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn apply(&mut self) -> Result<()> {
        let request = self
            .request
            .as_ref()
            .context("update request missing")?
            .clone();
        let source = installed_source(&self.receipt.data_dir)?;
        anyhow::ensure!(
            request.source_name == source.name
                && request.channel == normalized_channel(&source)
                && source.publisher_dids.first() == Some(&request.publisher_did)
                && request.current_version == source.installed_version,
            "Update choice changed. Check the update again."
        );
        let writer = crate::install_transaction::InstallationGuard::acquire(
            self.receipt
                .binary
                .parent()
                .context("installed Runtime parent missing")?,
        )?;
        self.previous_binary_sha256 = crate::installed_release::load_or_migrate(
            &self.receipt.data_dir,
            &self.receipt.binary,
            &source,
            &writer,
        )?
        .binary_sha256;
        drop(writer);
        self.previous_version = source.installed_version.clone();
        let client =
            Arc::new(crate::carrier::CarrierClient::connect_trusted_source(&source, 15).await?);
        self.carrier = Some(client.clone());
        let fetch_client = client.clone();
        let choice = request.clone();
        let trusted = source.clone();
        let fetch: crate::update::FetchFn = Box::new(move |cid, _| {
            let client = fetch_client.clone();
            let choice = choice.clone();
            let trusted = trusted.clone();
            Box::pin(async move {
                let bytes = client.fetch_content(&cid, None).await?;
                if cid == choice.head_cid {
                    verify_update_choice(&bytes, &choice, &trusted)?;
                }
                Ok(bytes)
            })
        });
        let result = async {
            self.publish("restarting", "Installing the update and restarting Home.")?;
            self.stop_child().await?;
            let data_dir = self.receipt.data_dir.clone();
            crate::update::run_restarting_update(&data_dir, &fetch, request.head_cid, self).await
        }
        .await;
        let closed = self.close_carrier().await;
        // A refusal before activation still needs exactly the unchanged Home.
        if self.child.is_none() && !InstallTransaction::has_pending_recovery(&self.receipt.binary) {
            self.start_initial().await?;
        }
        result.and(closed)
    }

    async fn reconcile_pending(&mut self) -> Result<()> {
        if path_present(&self.directory.join(ACTIVE_REQUEST))? {
            self.request = Some(read_private_json(&self.directory.join(ACTIVE_REQUEST))?);
        }
        if !InstallTransaction::has_pending_recovery(&self.receipt.binary) {
            // A consumed request is never replayed, including a pre-activation crash.
            self.finish_request()?;
            return Ok(());
        }
        let transaction =
            InstallTransaction::acquire(&self.receipt.data_dir, &self.receipt.binary)?;
        anyhow::ensure!(
            self.request.is_some(),
            "Pending update has no consumed request; preserve its recovery owner."
        );
        // Before activation no new host has a start claim. Unchanged CLI files
        // recover in their journal namespace under both locks, then retire the request.
        let Some(record) = transaction.restart_record_if_any()? else {
            let offline = crate::host_lock::acquire_host_process_lock(
                &self.receipt.data_dir,
                "update-recovery",
                "offline",
            )?;
            anyhow::ensure!(
                transaction.recover_before_restart()?,
                "Pre-restart recovery state changed."
            );
            self.finish_request()?;
            drop(offline);
            return Ok(());
        };
        anyhow::ensure!(
            record.plan.controller_sha256 == self.receipt.controller_sha256
                && record.plan.launch_plan_sha256 == self.receipt.launch_sha256
                && self
                    .request
                    .as_ref()
                    .is_some_and(|request| request.id == record.plan.request_id),
            "Pending recovery belongs to a different controller."
        );
        if matches!(record.phase, RestartPhase::CandidateStartClaimed) {
            bail!("Home start was claimed without a process receipt. Retain recovery files for operator repair.");
        }
        // The pipe closes on controller death. A new owner proves exact generation absence.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while crate::host_lock::active_host_process(&self.receipt.data_dir)?.is_some() {
            if tokio::time::Instant::now() >= deadline {
                bail!("Previous Home generation is still active. Retain recovery files.");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if let (Some(pid), Some(start)) = (record.pid, record.process_start.as_deref()) {
            while !child::generation_gone(pid, start)? {
                anyhow::ensure!(
                    tokio::time::Instant::now() < deadline,
                    "Previous Home group is still active or changed. Retain recovery files."
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            let owner = crate::runtime_control::gateway_children::Owner::read_for_generation(
                &self.receipt.data_dir,
                pid,
                &record.generation,
            )?;
            crate::runtime_control::gateway_children::shutdown(owner).await?;
        }
        crate::runtime_control::gateway_children::refuse_unreconciled_groups(
            &self.receipt.data_dir,
        )?;
        if matches!(
            record.phase,
            RestartPhase::CandidateReady | RestartPhase::PreviousReady
        ) {
            crate::update::verify_restart_support(&transaction)?;
            transaction.finish_restart()?;
            self.finish_request()?;
            return Ok(());
        }
        if matches!(
            record.phase,
            RestartPhase::PreviousStartClaimed | RestartPhase::PreviousRunning
        ) {
            bail!("The previous Home start was already attempted. Retain recovery files for operator repair.");
        }
        let offline = crate::host_lock::acquire_host_process_lock(
            &self.receipt.data_dir,
            "update-recovery",
            "offline",
        )?;
        transaction.restore_for_restart()?;
        crate::update::verify_restart_support(&transaction)?;
        drop(offline);
        let record = transaction.claim_start(true)?;
        if let Err(error) = crate::update::RestartOwner::start(self, &transaction, record).await {
            self.stop_child().await?;
            return Err(
                error.context("Previous Home start failed; retain its consumed recovery claim.")
            );
        }
        crate::update::finish_ready_restart(&transaction)?;
        transaction.finish_restart()?;
        self.finish_request()?;
        Ok(())
    }
}

impl crate::update::RestartOwner for Controller {
    fn plan(
        &self,
        support_sha256: String,
        previous_version: &str,
        candidate_version: &str,
    ) -> Result<RestartPlan> {
        let request = self.request.as_ref().context("update request missing")?;
        anyhow::ensure!(
            previous_version == self.previous_version && candidate_version == request.new_version,
            "Signed update differs from the approved versions."
        );
        Ok(RestartPlan {
            request_id: request.id.clone(),
            controller_sha256: self.receipt.controller_sha256.clone(),
            launch_plan_sha256: self.receipt.launch_sha256.clone(),
            support_sha256,
            previous_version: previous_version.into(),
            candidate_version: candidate_version.into(),
            previous_binary_sha256: self.previous_binary_sha256.clone(),
        })
    }

    fn start<'a>(
        &'a mut self,
        transaction: &'a InstallTransaction,
        record: RestartRecord,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let previous = record.phase == RestartPhase::PreviousStartClaimed;
            anyhow::ensure!(
                previous || record.phase == RestartPhase::CandidateStartClaimed,
                "Home start has no durable claim"
            );
            let source = installed_source(&self.receipt.data_dir)?;
            let version = if previous {
                &record.plan.previous_version
            } else {
                &record.plan.candidate_version
            };
            anyhow::ensure!(
                &source.installed_version == version,
                "Installed version differs from the start claim"
            );
            let expected =
                crate::installed_release::read_for_transaction(transaction, &source)?.binary_sha256;
            if previous {
                anyhow::ensure!(
                    expected == record.plan.previous_binary_sha256,
                    "Previous Runtime differs from verified rollback"
                );
            }
            self.spawn(&record.generation, true)?;
            let pid = self.child.as_ref().unwrap().pid();
            transaction.record_started(
                &record.generation,
                pid,
                process_start(pid).context("Home exited during start")?,
            )?;
            self.wait_ready(&record.generation, version, &expected, true)
                .await?;
            Ok(())
        })
    }

    fn stop<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(self.stop_child())
    }
}

fn verify_update_choice(
    bytes: &[u8],
    choice: &UpdateRequest,
    source: &TrustedSource,
) -> Result<()> {
    crate::update::verify_release_metadata_cid(&choice.head_cid, bytes)?;
    let (head, signer) = crate::crypto::verify_release_envelope_against_dids(
        bytes,
        "elastos.release.head.v1",
        &source.publisher_dids,
    )?;
    anyhow::ensure!(
        signer == choice.publisher_did
            && head["payload"]["latest_release_cid"].as_str() == Some(choice.release_cid.as_str())
            && head["payload"]["version"].as_str() == Some(choice.new_version.as_str()),
        "Signed release differs from the owner update choice."
    );
    crate::update::verify_source_channel(source, head["payload"]["channel"].as_str().unwrap_or(""))
}

fn path_present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn installed_source(data_dir: &Path) -> Result<TrustedSource> {
    load_trusted_sources(data_dir)?
        .default_source()
        .cloned()
        .context("Installed trusted source is unavailable.")
}

fn normalized_channel(source: &TrustedSource) -> &str {
    if source.channel.trim().is_empty() {
        "stable"
    } else {
        source.channel.trim()
    }
}

#[cfg(test)]
fn admit_installed_release(signed: &[u8], source: &TrustedSource, binary: &Path) -> Result<String> {
    let expected = admit_release_digest(signed, source)?;
    anyhow::ensure!(
        crate::runtime_control::sha256_file(binary)? == expected,
        "Installed Runtime differs from its signed release."
    );
    Ok(expected)
}

fn admit_release_digest(signed: &[u8], source: &TrustedSource) -> Result<String> {
    let (release, _) = crate::crypto::verify_release_envelope_against_dids(
        signed,
        "elastos.release.v1",
        &source.publisher_dids,
    )?;
    let payload = &release["payload"];
    anyhow::ensure!(
        payload["schema"] == "elastos.release/v1"
            && payload["version"].as_str() == Some(source.installed_version.as_str()),
        "Installed signed release version is incompatible."
    );
    crate::update::verify_source_channel(source, payload["channel"].as_str().unwrap_or(""))?;
    let expected = payload["platforms"][crate::update::detect_release_platform()]["binary"]
        ["sha256"]
        .as_str()
        .filter(|hash| valid_hash(hash))
        .context("Signed controller Runtime checksum missing")?
        .to_owned();
    Ok(expected)
}

fn validate_receipt(receipt: &Receipt) -> Result<()> {
    validate_retained_receipt(receipt)?;
    anyhow::ensure!(
        crate::runtime_control::sha256_file(&std::env::current_exe()?)?
            == receipt.controller_sha256,
        "Controller process differs from its signed receipt."
    );
    Ok(())
}

fn validate_retained_receipt(receipt: &Receipt) -> Result<()> {
    validate_retained_receipt_record(receipt)?;
    anyhow::ensure!(
        controller_file_digest(&receipt.controller)?.as_deref()
            == Some(receipt.controller_sha256.as_str()),
        "Controller binary differs from its signed receipt."
    );
    Ok(())
}

fn validate_retained_receipt_record(receipt: &Receipt) -> Result<()> {
    anyhow::ensure!(
        receipt.schema == "elastos.update-controller/v1"
            && receipt.data_dir.is_absolute()
            && receipt.binary.is_absolute()
            && receipt.controller == receipt.data_dir.join(DIRECTORY).join("runtime")
            && receipt.launch.sha256()? == receipt.launch_sha256,
        "Installed controller receipt is incompatible."
    );
    let signed = BASE64.decode(&receipt.signed_controller_release)?;
    let digest = admit_release_digest(&signed, &receipt.trusted_source)?;
    anyhow::ensure!(
        digest == receipt.controller_sha256,
        "Controller binary differs from its signed receipt."
    );
    Ok(())
}

fn controller_file_digest(path: &Path) -> Result<Option<String>> {
    if !path_present(path)? {
        return Ok(None);
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.nlink() == 1
            && metadata.mode() & 0o7777 == 0o700,
        "Retained controller file is unsafe."
    );
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let length = file.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        digest.update(&buffer[..length]);
    }
    Ok(Some(hex::encode(digest.finalize())))
}

fn controller_space_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<crate::install_transaction::DiskReserveError>()
        .is_some()
        || error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.raw_os_error() == Some(libc::ENOSPC))
}

// Caller holds the controller lease and installation lock through receipt publication.
fn prepare_controller(
    directory: &Path,
    binary: &Path,
    expected: &str,
    reserve: impl FnOnce(&Path, u64) -> Result<()>,
) -> Result<bool> {
    crate::install_transaction::refuse_pending_home_start(directory.parent().unwrap(), binary)?;
    let controller = directory.join("runtime");
    controller_file_digest(&controller.with_extension("partial"))?;
    let existing = controller_file_digest(&controller)?;
    let path = directory.join(RECEIPT);
    if path_present(&path)? {
        let receipt: Receipt = read_private_json(&path)?;
        validate_retained_receipt_record(&receipt)?;
        anyhow::ensure!(
            receipt.data_dir == directory.parent().unwrap()
                && receipt.binary == binary
                && existing
                    .as_deref()
                    .is_some_and(|hash| hash == receipt.controller_sha256 || hash == expected),
            "Retained controller identity changed. Preserve its files for repair."
        );
    } else if let Some(hash) = existing.as_deref() {
        anyhow::ensure!(
            hash == expected,
            "Existing controller has no signed receipt. Preserve it for repair."
        );
    }
    if existing.as_deref() == Some(expected) {
        // Also repairs a crash after current signed bytes replaced an older receipt's bytes.
        return Ok(true);
    }
    let result = reserve(directory, fs::metadata(binary)?.len() + MAX_PRIVATE_JSON)
        .and_then(|()| copy_controller(binary, &controller, expected));
    match result {
        Ok(()) => Ok(true),
        Err(error) if controller_space_error(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

/// The retained receipt locates the writer lock even during an interrupted sources rename.
pub(crate) fn installed_writer_binary(data_dir: &Path) -> Result<Option<PathBuf>> {
    let path = data_dir.join(DIRECTORY).join(RECEIPT);
    if !path_present(&path)? {
        return Ok(None);
    }
    let receipt: Receipt = read_private_json(&path)?;
    anyhow::ensure!(
        receipt.data_dir == fs::canonicalize(data_dir)?,
        "Controller writer receipt belongs to another data root."
    );
    validate_retained_receipt(&receipt)?;
    Ok(Some(receipt.binary))
}

/// Host preflight keeps the prior receipt's installation while a signed controller repair is pending.
/// Controller execution and ordinary writers retain their full receipt admission gates.
pub(crate) fn installed_host_binary(data_dir: &Path) -> Result<Option<PathBuf>> {
    let path = data_dir.join(DIRECTORY).join(RECEIPT);
    if !path_present(&path)? {
        return Ok(None);
    }
    check_controller_directory(path.parent().context("controller receipt parent missing")?)?;
    let receipt: Receipt = read_private_json(&path)?;
    anyhow::ensure!(
        receipt.data_dir == fs::canonicalize(data_dir)?,
        "Controller host receipt belongs to another data root."
    );
    validate_retained_receipt_record(&receipt)?;
    anyhow::ensure!(
        Path::new(&receipt.trusted_source.install_path).is_absolute()
            && fs::canonicalize(&receipt.trusted_source.install_path)? == receipt.binary,
        "Controller host receipt belongs to another installed binary."
    );
    let existing = controller_file_digest(&receipt.controller)?
        .context("Retained controller binary is missing. Preserve its files for repair.")?;
    if existing != receipt.controller_sha256 {
        let sources = load_trusted_sources(data_dir)?;
        let source = sources
            .default_source()
            .context("Installed trusted source missing")?;
        let installed =
            crate::installed_release::read_without_migration(data_dir, &receipt.binary, source)?;
        anyhow::ensure!(
            existing == installed.binary_sha256,
            "Controller binary differs from its signed receipt and current installed release."
        );
    }
    Ok(Some(receipt.binary))
}

fn controller_directory(data_dir: &Path) -> Result<PathBuf> {
    anyhow::ensure!(
        data_dir.is_absolute() && fs::canonicalize(data_dir)? == data_dir,
        "controller data root must be resolved"
    );
    let path = data_dir.join(DIRECTORY);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::DirBuilder::new().mode(0o700).create(&path)?;
            File::open(data_dir)?.sync_all()?;
        }
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    check_controller_directory(&path)?;
    Ok(path)
}

fn check_controller_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o7777 == 0o700,
        "controller directory must be an owner-only real directory"
    );
    Ok(())
}

fn acquire_lease(directory: &Path) -> Result<File> {
    let path = directory.join("controller.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)?;
    check_private_file(&file)?;
    anyhow::ensure!(
        file.metadata()?.len() == 0,
        "controller lease file must be empty"
    );
    anyhow::ensure!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "Home already has an update controller. Keep its terminal open."
    );
    Ok(file)
}

fn inherited_or_new_lease(directory: &Path) -> Result<File> {
    let Ok(value) = std::env::var(LEASE_ENV) else {
        return acquire_lease(directory);
    };
    std::env::remove_var(LEASE_ENV);
    let fd: i32 = value.parse()?;
    anyhow::ensure!(
        fd >= 3 && unsafe { libc::fcntl(fd, libc::F_GETFD) } >= 0,
        "controller lease descriptor is unavailable"
    );
    let file = unsafe { File::from_raw_fd(fd) };
    check_private_file(&file)?;
    let expected = fs::symlink_metadata(directory.join("controller.lock"))?;
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.dev() == expected.dev() && metadata.ino() == expected.ino(),
        "controller lease identity changed"
    );
    anyhow::ensure!(
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == 0,
        "protect controller lease descriptor"
    );
    anyhow::ensure!(
        unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "controller lease is busy"
    );
    Ok(file)
}

fn copy_controller(binary: &Path, controller: &Path, expected: &str) -> Result<()> {
    let temporary = controller.with_extension("partial");
    if fs::symlink_metadata(&temporary).is_ok() {
        let scratch = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        let metadata = scratch.metadata()?;
        anyhow::ensure!(
            metadata.is_file()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.nlink() == 1
                && metadata.mode() & 0o7777 == 0o700,
            "Controller copy scratch is unsafe; retain it for repair."
        );
        fs::remove_file(&temporary)?;
        File::open(controller.parent().unwrap())?.sync_all()?;
    }
    let mut input = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(binary)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)?;
    let result = (|| {
        let mut buffer = [0; 64 * 1024];
        loop {
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            crate::install_transaction::require_controller_disk_reserve(
                controller.parent().unwrap(),
                count as u64,
            )?;
            output.write_all(&buffer[..count])?;
        }
        output.sync_all()?;
        anyhow::ensure!(
            crate::runtime_control::sha256_file(&temporary)? == expected,
            "Controller copy differs from the signed Runtime."
        );
        if fs::symlink_metadata(controller).is_ok() {
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(controller)?;
            let metadata = file.metadata()?;
            anyhow::ensure!(
                metadata.is_file()
                    && metadata.uid() == unsafe { libc::geteuid() }
                    && metadata.nlink() == 1
                    && metadata.mode() & 0o7777 == 0o700,
                "Existing controller is unsafe"
            );
        }
        fs::rename(&temporary, controller)?;
        File::open(controller.parent().unwrap())?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn check_private_file(file: &File) -> Result<()> {
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.nlink() == 1
            && metadata.mode() & 0o7777 == 0o600,
        "private controller file is unsafe"
    );
    Ok(())
}

fn read_regular_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() <= limit,
        "controller input is unsafe or too large"
    );
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= limit,
        "controller input grew beyond its limit"
    );
    Ok(bytes)
}

fn read_private_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    check_private_file(&file)?;
    anyhow::ensure!(
        file.metadata()?.len() <= MAX_PRIVATE_JSON,
        "private controller record is too large"
    );
    let mut bytes = Vec::new();
    file.take(MAX_PRIVATE_JSON + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_PRIVATE_JSON,
        "private controller record grew beyond its limit"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

fn write_new_private(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_PRIVATE_JSON,
        "private controller record is too large"
    );
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn write_private(path: &Path, value: &impl Serialize) -> Result<()> {
    if fs::symlink_metadata(path).is_ok() {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        check_private_file(&file)?;
    }
    let temporary = path.with_extension(format!("{}.tmp", hex::encode(rand::random::<[u8; 16]>())));
    let result = (|| {
        write_new_private(&temporary, &serde_json::to_vec(value)?, 0o600)?;
        fs::rename(&temporary, path)?;
        File::open(path.parent().unwrap())?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn open_private_log(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .truncate(false)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    check_private_file(&file)?;
    file.set_len(0)?;
    Ok(file)
}

fn process_start(pid: u32) -> Option<String> {
    child::process_start(pid)
}

fn home_digest(data_dir: &Path) -> Result<String> {
    let path = crate::api::browser_capsules::installed_home_document(data_dir)
        .context("Installed Home document is unavailable")?;
    Ok(digest(&read_regular_bounded(&path, 2 * 1024 * 1024)?))
}

async fn response_bytes(response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    anyhow::ensure!(
        response.status().is_success(),
        "Home readiness request refused"
    );
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            bytes.len().saturating_add(chunk.len()) <= limit,
            "Home readiness response is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn approved_home_url(value: &str) -> Result<url::Url> {
    anyhow::ensure!(
        matches!(
            value,
            "http://localhost:8090/home/"
                | "http://127.0.0.1:8090/home/"
                | "http://[::1]:8090/home/"
        ),
        "Home listener has not reported an approved readiness URL"
    );
    Ok(url::Url::parse(value)?)
}

#[allow(clippy::too_many_arguments)]
async fn prove_ready(
    client: &reqwest::Client,
    data_dir: &Path,
    pid: u32,
    generation: &str,
    version: &str,
    binary_sha256: &str,
    home_sha256: &str,
) -> Result<()> {
    let coords: crate::runtime_control::RuntimeCoords = read_private_json(
        &crate::runtime_control::gateway_runtime_coord_path(data_dir),
    )?;
    anyhow::ensure!(
        coords.pid == pid
            && coords.is_gateway_runtime()
            && coords.generation == generation
            && coords.binary_sha256 == binary_sha256
            && !coords.attach_secret.is_empty(),
        "Home readiness coordinates differ from the claimed host"
    );
    let metadata: serde_json::Value = serde_json::from_slice(&read_regular_bounded(
        &data_dir.join("host-process.lock"),
        4096,
    )?)?;
    anyhow::ensure!(
        metadata["pid"].as_u64() == Some(u64::from(pid))
            && metadata["generation"].as_str() == Some(generation)
            && metadata["role"] == "gateway",
        "Home host lock differs from the claimed generation"
    );
    let home_url = approved_home_url(&coords.home_url)?;
    let base = crate::local_http::LoopbackHttpBaseUrl::parse(&coords.api_url)?;
    let health: serde_json::Value = serde_json::from_slice(
        &response_bytes(client.get(base.join("/api/health")?).send().await?, 4096).await?,
    )?;
    anyhow::ensure!(
        health["version"].as_str() == Some(version),
        "Home health version differs from the signed release"
    );
    let attached: serde_json::Value = serde_json::from_slice(
        &response_bytes(
            client
                .post(base.join("/api/auth/attach")?)
                .json(&serde_json::json!({"secret":coords.attach_secret,"scope":"client"}))
                .send()
                .await?,
            16 * 1024,
        )
        .await?,
    )?;
    anyhow::ensure!(
        attached["token"]
            .as_str()
            .is_some_and(|token| !token.is_empty()),
        "Home control identity refused the private attach secret"
    );
    let served = response_bytes(client.get(home_url).send().await?, 2 * 1024 * 1024).await?;
    anyhow::ensure!(
        digest(&served) == home_sha256,
        "Served Home differs from the installed capsule"
    );
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests;
