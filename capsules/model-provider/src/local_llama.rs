use crate::config::{revalidate_local_artifact, LocalArtifactConfig, LocalLlamaSettings};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Read as _;
use std::net::TcpListener;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
#[cfg(not(test))]
use tokio::io::AsyncWriteExt as _;
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;
use tokio::time::sleep;

const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(25);
const HEALTH_REQUEST_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_HEALTH_RESPONSE_BYTES: usize = 16 * 1024;
const MAX_GUARD_CONFIG_BYTES: usize = 12 * 1024;
#[cfg(not(test))]
const GUARD_START_TIMEOUT: Duration = Duration::from_secs(2);
const GUARD_EXIT_GRACE: Duration = Duration::from_secs(1);
pub(crate) const INTERNAL_GUARD_ARG: &str = "--internal-local-llama-guard";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalLlamaFault {
    Failed,
    Timeout,
}

#[derive(Clone, Default)]
pub(crate) struct LocalLlamaEngines {
    engines: Arc<Mutex<BTreeMap<String, RunningEngine>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalLlamaEndpoint {
    pub api_url: String,
    pub model: String,
    pub enable_thinking: bool,
}

struct RunningEngine {
    child: Child,
    liveness: Option<ChildStdin>,
    guard_group: Option<libc::pid_t>,
    endpoint: LocalLlamaEndpoint,
    models_url: String,
    shutdown_timeout: Duration,
    closing: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GuardConfig {
    engine_path: String,
    model_path: String,
    port: u16,
    alias: String,
    settings: LocalLlamaSettings,
}

impl LocalLlamaEngines {
    pub(crate) async fn retains_artifacts(&self) -> bool {
        // Idle engines can still map model bytes. Only the existing lifecycle
        // owner can prove closure; refresh does not attempt eviction.
        !self.engines.lock().await.is_empty()
    }

    #[cfg(test)]
    pub(crate) async fn endpoint(
        &self,
        offer_id: &str,
        engine: &LocalArtifactConfig,
        model: &LocalArtifactConfig,
        settings: &LocalLlamaSettings,
    ) -> Result<LocalLlamaEndpoint, LocalLlamaFault> {
        self.endpoint_with_timeout(offer_id, engine, model, settings, Duration::from_secs(30))
            .await
    }

    pub(crate) async fn endpoint_with_timeout(
        &self,
        offer_id: &str,
        engine: &LocalArtifactConfig,
        model: &LocalArtifactConfig,
        settings: &LocalLlamaSettings,
        timeout: Duration,
    ) -> Result<LocalLlamaEndpoint, LocalLlamaFault> {
        let deadline = Instant::now() + timeout;
        let mut engines = tokio::time::timeout(timeout, self.engines.lock())
            .await
            .map_err(|_| LocalLlamaFault::Timeout)?;
        let stale = match engines.get_mut(offer_id) {
            Some(running) if running.closing => true,
            Some(running) => match running_engine_is_ready(running, deadline).await? {
                true => return Ok(running.endpoint.clone()),
                false => true,
            },
            None => false,
        };
        if stale {
            close_engine(&mut engines, offer_id).await?;
        }

        revalidate_local_artifact(engine, true, deadline).map_err(|_| deadline_fault(deadline))?;
        revalidate_local_artifact(model, false, deadline).map_err(|_| deadline_fault(deadline))?;
        if Instant::now() >= deadline {
            return Err(LocalLlamaFault::Timeout);
        }
        let port = reserve_loopback_port()?;
        let alias = random_alias()?;
        let endpoint = LocalLlamaEndpoint {
            api_url: format!("http://127.0.0.1:{port}/v1/chat/completions"),
            model: alias.clone(),
            enable_thinking: settings.enable_thinking,
        };
        let models_url = format!("http://127.0.0.1:{port}/v1/models");
        let (child, liveness, guard_group) =
            spawn_managed_engine(engine, model, settings, port, &alias).await?;
        let health_timeout = Duration::from_millis(settings.health_timeout_ms)
            .min(deadline.saturating_duration_since(Instant::now()));
        let shutdown_timeout = Duration::from_millis(settings.shutdown_timeout_ms);
        engines.insert(
            offer_id.to_string(),
            RunningEngine {
                child,
                liveness,
                guard_group,
                endpoint: endpoint.clone(),
                models_url,
                shutdown_timeout,
                closing: false,
            },
        );
        let health_result = match engines.get_mut(offer_id) {
            Some(running) => {
                #[cfg(not(test))]
                let initialized =
                    initialize_guard(running, engine, model, settings, port, &alias, deadline)
                        .await;
                #[cfg(test)]
                let initialized = Ok(());
                match initialized {
                    Err(fault) => Err(fault),
                    Ok(()) => {
                        wait_until_healthy(
                            &mut running.child,
                            running.guard_group,
                            port,
                            &alias,
                            health_timeout.min(deadline.saturating_duration_since(Instant::now())),
                        )
                        .await
                    }
                }
            }
            None => Err(LocalLlamaFault::Failed),
        };
        if let Err(fault) = health_result {
            close_engine(&mut engines, offer_id).await?;
            return Err(fault);
        }
        Ok(endpoint)
    }

    pub(crate) async fn close_offer(&self, offer_id: &str) -> Result<(), LocalLlamaFault> {
        let mut engines = tokio::time::timeout(GUARD_EXIT_GRACE, self.engines.lock())
            .await
            .map_err(|_| LocalLlamaFault::Timeout)?;
        close_engine(&mut engines, offer_id).await
    }

    pub(crate) async fn shutdown(&self) -> Result<(), LocalLlamaFault> {
        let mut engines = tokio::time::timeout(GUARD_EXIT_GRACE, self.engines.lock())
            .await
            .map_err(|_| LocalLlamaFault::Timeout)?;
        let ids: Vec<_> = engines.keys().cloned().collect();
        let mut result = Ok(());
        for id in ids {
            if let Err(fault) = close_engine(&mut engines, &id).await {
                result = Err(fault);
            }
        }
        result
    }
}

async fn close_engine(
    engines: &mut BTreeMap<String, RunningEngine>,
    offer_id: &str,
) -> Result<(), LocalLlamaFault> {
    if let Some(running) = engines.get_mut(offer_id) {
        running.closing = true;
        terminate_child(
            &mut running.child,
            &mut running.liveness,
            running.guard_group,
            running.shutdown_timeout,
        )
        .await?;
        engines.remove(offer_id);
    }
    Ok(())
}

async fn spawn_managed_engine(
    engine: &LocalArtifactConfig,
    model: &LocalArtifactConfig,
    settings: &LocalLlamaSettings,
    port: u16,
    alias: &str,
) -> Result<(Child, Option<ChildStdin>, Option<libc::pid_t>), LocalLlamaFault> {
    #[cfg(test)]
    {
        let mut command = Command::new(&engine.path);
        configure_tokio_engine_command(&mut command, &model.path, settings, port, alias);
        let child = command.spawn().map_err(|_| LocalLlamaFault::Failed)?;
        Ok((child, None, None))
    }

    #[cfg(not(test))]
    {
        let _ = (engine, model, settings, port, alias);
        spawn_guarded_engine()
    }
}

#[cfg(not(test))]
async fn initialize_guard(
    running: &mut RunningEngine,
    engine: &LocalArtifactConfig,
    model: &LocalArtifactConfig,
    settings: &LocalLlamaSettings,
    port: u16,
    alias: &str,
    deadline: Instant,
) -> Result<(), LocalLlamaFault> {
    let config = GuardConfig {
        engine_path: engine.path.clone(),
        model_path: model.path.clone(),
        port,
        alias: alias.to_string(),
        settings: settings.clone(),
    };
    let mut frame = serde_json::to_vec(&config).map_err(|_| LocalLlamaFault::Failed)?;
    frame.push(b'\n');
    if frame.len() > MAX_GUARD_CONFIG_BYTES {
        return Err(LocalLlamaFault::Failed);
    }
    let liveness = running.liveness.as_mut().ok_or(LocalLlamaFault::Failed)?;
    tokio::time::timeout(
        GUARD_START_TIMEOUT.min(deadline.saturating_duration_since(Instant::now())),
        async {
            liveness
                .write_all(&frame)
                .await
                .map_err(|_| LocalLlamaFault::Failed)?;
            liveness.flush().await.map_err(|_| LocalLlamaFault::Failed)
        },
    )
    .await
    .map_err(|_| LocalLlamaFault::Timeout)?
}

#[cfg(not(test))]
fn spawn_guarded_engine(
) -> Result<(Child, Option<ChildStdin>, Option<libc::pid_t>), LocalLlamaFault> {
    let executable = std::env::current_exe().map_err(|_| LocalLlamaFault::Failed)?;
    let mut command = Command::new(executable);
    command
        .env_clear()
        .arg(INTERNAL_GUARD_ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.as_std_mut().process_group(0);
    }
    let mut child = command.spawn().map_err(|_| LocalLlamaFault::Failed)?;
    let guard_group = child.id().map(|pid| pid as libc::pid_t);
    let liveness = child.stdin.take();
    Ok((child, liveness, guard_group))
}

#[cfg(test)]
fn configure_tokio_engine_command(
    command: &mut Command,
    model_path: &str,
    settings: &LocalLlamaSettings,
    port: u16,
    alias: &str,
) {
    command
        .env_clear()
        .args(engine_arguments(model_path, settings, port, alias))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
}

fn engine_arguments(
    model_path: &str,
    settings: &LocalLlamaSettings,
    port: u16,
    alias: &str,
) -> Vec<OsString> {
    [
        "-m".into(),
        model_path.into(),
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string().into(),
        "--ctx-size".into(),
        settings.context_size.to_string().into(),
        "--parallel".into(),
        settings.parallel.to_string().into(),
        "--threads".into(),
        settings.threads.to_string().into(),
        "--threads-batch".into(),
        settings.batch_threads.to_string().into(),
        "--gpu-layers".into(),
        settings.gpu_layers.to_string().into(),
        "--alias".into(),
        alias.into(),
    ]
    .into()
}

async fn running_engine_is_ready(
    running: &mut RunningEngine,
    deadline: Instant,
) -> Result<bool, LocalLlamaFault> {
    if !managed_child_is_running(&mut running.child, running.guard_group) {
        return Ok(false);
    }
    let Ok(client) = health_client() else {
        return Ok(false);
    };
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(LocalLlamaFault::Timeout);
    }
    let ready = matches!(
        tokio::time::timeout(
            remaining.min(HEALTH_REQUEST_TIMEOUT),
            endpoint_reports_alias(&client, &running.models_url, &running.endpoint.model),
        )
        .await,
        Ok(Ok(true))
    ) && managed_child_is_running(&mut running.child, running.guard_group);
    if !ready && Instant::now() >= deadline {
        return Err(LocalLlamaFault::Timeout);
    }
    Ok(ready)
}

fn deadline_fault(deadline: Instant) -> LocalLlamaFault {
    if Instant::now() >= deadline {
        LocalLlamaFault::Timeout
    } else {
        LocalLlamaFault::Failed
    }
}

fn managed_child_is_running(child: &mut Child, guard_group: Option<libc::pid_t>) -> bool {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Some(guard_pid) = guard_group {
        return matches!(child_exited_without_reaping(guard_pid), Ok(false));
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let _ = guard_group;
    matches!(child.try_wait(), Ok(None))
}

fn reserve_loopback_port() -> Result<u16, LocalLlamaFault> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|_| LocalLlamaFault::Failed)?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|_| LocalLlamaFault::Failed)
}

#[cfg(unix)]
fn random_alias() -> Result<String, LocalLlamaFault> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .map_err(|_| LocalLlamaFault::Failed)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(not(unix))]
fn random_alias() -> Result<String, LocalLlamaFault> {
    Err(LocalLlamaFault::Failed)
}

async fn wait_until_healthy(
    child: &mut Child,
    guard_group: Option<libc::pid_t>,
    port: u16,
    alias: &str,
    timeout: Duration,
) -> Result<(), LocalLlamaFault> {
    let client = health_client()?;
    let deadline = Instant::now() + timeout;
    let models_url = format!("http://127.0.0.1:{port}/v1/models");
    loop {
        if !managed_child_is_running(child, guard_group) {
            return Err(LocalLlamaFault::Failed);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(LocalLlamaFault::Timeout);
        }
        let request_timeout = remaining.min(HEALTH_REQUEST_TIMEOUT);
        if matches!(
            tokio::time::timeout(
                request_timeout,
                endpoint_reports_alias(&client, &models_url, alias),
            )
            .await,
            Ok(Ok(true))
        ) && managed_child_is_running(child, guard_group)
        {
            return Ok(());
        }
        sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(HEALTH_POLL_INTERVAL),
        )
        .await;
    }
}

fn health_client() -> Result<reqwest::Client, LocalLlamaFault> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(HEALTH_REQUEST_TIMEOUT)
        .read_timeout(HEALTH_REQUEST_TIMEOUT)
        .build()
        .map_err(|_| LocalLlamaFault::Failed)
}

async fn endpoint_reports_alias(
    client: &reqwest::Client,
    url: &str,
    alias: &str,
) -> Result<bool, ()> {
    let mut response = client.get(url).send().await.map_err(|_| ())?;
    if !response.status().is_success() {
        return Ok(false);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
        if bytes.len().saturating_add(chunk.len()) > MAX_HEALTH_RESPONSE_BYTES {
            return Ok(false);
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| ())?;
    Ok(value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .map(|models| {
            models
                .iter()
                .any(|model| model.get("id").and_then(serde_json::Value::as_str) == Some(alias))
        })
        .unwrap_or(false))
}

async fn terminate_child(
    child: &mut Child,
    liveness: &mut Option<ChildStdin>,
    guard_group: Option<libc::pid_t>,
    timeout: Duration,
) -> Result<(), LocalLlamaFault> {
    if timeout.is_zero() {
        return Err(LocalLlamaFault::Timeout);
    }
    if let Some(group) = guard_group {
        if group <= 1 || child.id().is_some_and(|pid| pid != group as u32) {
            return Err(guard_close_failed("guard_identity", None));
        }
        liveness.take();
        return finish_guard_shutdown(child, group, timeout).await;
    }
    if child
        .try_wait()
        .map_err(|_| LocalLlamaFault::Failed)?
        .is_some()
    {
        return Ok(());
    }
    if let Err(error) = send_graceful_termination(child) {
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(LocalLlamaFault::Failed);
        }
    }
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(result) => result.map(|_| ()).map_err(|_| LocalLlamaFault::Failed),
        Err(_) => {
            child.start_kill().map_err(|_| LocalLlamaFault::Failed)?;
            tokio::time::timeout(GUARD_EXIT_GRACE, child.wait())
                .await
                .map_err(|_| LocalLlamaFault::Timeout)?
                .map(|_| ())
                .map_err(|_| LocalLlamaFault::Failed)
        }
    }
}

#[cfg(unix)]
async fn finish_guard_shutdown(
    child: &mut Child,
    guard_group: libc::pid_t,
    timeout: Duration,
) -> Result<(), LocalLlamaFault> {
    if guard_group == unsafe { libc::getpgrp() } {
        return Err(guard_close_failed("guard_own_group", None));
    }
    if child.id().is_some() {
        let deadline = Instant::now()
            .checked_add(timeout.saturating_add(GUARD_EXIT_GRACE))
            .ok_or(LocalLlamaFault::Timeout)?;
        let guard_exited = loop {
            if child_exited_without_reaping(guard_group)
                .map_err(|error| guard_close_failed("guard_waitid", error.raw_os_error()))?
            {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            sleep(
                Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
            )
            .await;
        };
        // Keep the guard PID unreaped while signalling its group. A retry after
        // reaping only observes absence; it must not signal a recycled group ID.
        if let Err(error) = signal_process_group(guard_group, libc::SIGKILL) {
            if !exited_guard_signal_can_settle(guard_exited, error.raw_os_error()) {
                return Err(guard_close_failed("guard_signal", error.raw_os_error()));
            }
            // Darwin excludes zombies from killpg recipients and can return
            // EPERM for a zombie-only group (XNU bsd/kern/kern_sig.c, killpg1).
            // This permits only the owned reap below, not successful closure:
            // the post-reap group probe must still establish ESRCH.
        }
    }
    let deadline = tokio::time::Instant::now() + GUARD_EXIT_GRACE;
    tokio::time::timeout_at(deadline, child.wait())
        .await
        .map_err(|_| LocalLlamaFault::Timeout)?
        .map_err(|error| guard_close_failed("guard_wait", error.raw_os_error()))?;
    loop {
        if unsafe { libc::kill(-guard_group, 0) } != 0 {
            let errno = std::io::Error::last_os_error().raw_os_error();
            return if errno == Some(libc::ESRCH) {
                Ok(())
            } else {
                Err(guard_close_failed("guard_absence", errno))
            };
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(LocalLlamaFault::Timeout);
        }
        sleep(
            Duration::from_millis(10)
                .min(deadline.saturating_duration_since(tokio::time::Instant::now())),
        )
        .await;
    }
}

#[cfg(not(unix))]
async fn finish_guard_shutdown(
    _child: &mut Child,
    _guard_group: libc::pid_t,
    _timeout: Duration,
) -> Result<(), LocalLlamaFault> {
    Err(LocalLlamaFault::Failed)
}

#[cfg(unix)]
fn exited_guard_signal_can_settle(guard_exited: bool, errno: Option<i32>) -> bool {
    cfg!(target_os = "macos") && guard_exited && errno == Some(libc::EPERM)
}

fn guard_close_failed(stage: &'static str, errno: Option<i32>) -> LocalLlamaFault {
    // Private diagnostics carry only a fixed stage and OS error number, never
    // artifact paths, model text, or caller identity. Failure semantics stay exact.
    eprintln!("[model-provider] local engine closure stage={stage} errno={errno:?}");
    LocalLlamaFault::Failed
}

#[cfg(unix)]
fn send_graceful_termination(child: &mut Child) -> std::io::Result<()> {
    let pid = child
        .id()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "child already exited"))?;
    if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn send_graceful_termination(child: &mut Child) -> std::io::Result<()> {
    child.start_kill()
}

pub(crate) fn run_internal_guard_if_requested() -> Option<bool> {
    let mut arguments = std::env::args_os();
    let _ = arguments.next();
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new(INTERNAL_GUARD_ARG)) {
        return None;
    }
    if arguments.next().is_some() {
        return Some(false);
    }
    Some(run_internal_guard().is_ok())
}

#[cfg(unix)]
enum GuardEvent {
    ParentEof,
    ChildExited,
    MonitorFailed,
}

#[cfg(unix)]
fn run_internal_guard() -> Result<(), ()> {
    let config = read_guard_config()?;
    let guard_group = unsafe { libc::getpid() };
    if unsafe { libc::getpgrp() } != guard_group {
        return Err(());
    }
    let mut command = std::process::Command::new(&config.engine_path);
    command
        .env_clear()
        .args(engine_arguments(
            &config.model_path,
            &config.settings,
            config.port,
            &config.alias,
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut engine = command.spawn().map_err(|_| ())?;
    let engine_pid = engine.id() as libc::pid_t;
    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let child_event_tx = event_tx.clone();
    if std::thread::Builder::new()
        .spawn(move || {
            let event = match engine.wait() {
                Ok(_) => GuardEvent::ChildExited,
                Err(_) => GuardEvent::MonitorFailed,
            };
            let _ = child_event_tx.send(event);
        })
        .is_err()
    {
        kill_guard_process_group(guard_group);
    }
    if std::thread::Builder::new()
        .spawn(move || {
            let mut input = std::io::stdin();
            let mut byte = [0u8; 1];
            loop {
                match input.read(&mut byte) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            let _ = event_tx.send(GuardEvent::ParentEof);
        })
        .is_err()
    {
        kill_guard_process_group(guard_group);
    }

    if matches!(event_rx.recv(), Ok(GuardEvent::ParentEof)) {
        let _ = unsafe { libc::kill(engine_pid, libc::SIGTERM) };
        match event_rx.recv_timeout(Duration::from_millis(config.settings.shutdown_timeout_ms)) {
            Ok(GuardEvent::ChildExited | GuardEvent::MonitorFailed) | Err(_) => {}
            Ok(GuardEvent::ParentEof) => {}
        }
    }
    kill_guard_process_group(guard_group);
}

#[cfg(unix)]
fn kill_guard_process_group(process_group: libc::pid_t) -> ! {
    let _ = signal_process_group(process_group, libc::SIGKILL);
    unsafe { libc::_exit(1) }
}

#[cfg(not(unix))]
fn run_internal_guard() -> Result<(), ()> {
    Err(())
}

#[cfg(unix)]
fn read_guard_config() -> Result<GuardConfig, ()> {
    let mut input = std::io::stdin().lock();
    let mut frame = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        if frame.len() >= MAX_GUARD_CONFIG_BYTES {
            return Err(());
        }
        match input.read(&mut byte) {
            Ok(0) | Err(_) => return Err(()),
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) => frame.push(byte[0]),
        }
    }
    serde_json::from_slice(&frame).map_err(|_| ())
}

#[cfg(unix)]
fn signal_process_group(process_group: libc::pid_t, signal: libc::c_int) -> std::io::Result<()> {
    if unsafe { libc::kill(-process_group, signal) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    Err(error)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn child_exited_without_reaping(pid: libc::pid_t) -> std::io::Result<bool> {
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    Ok(unsafe { info.si_pid() } == pid)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn child_exited_without_reaping(_pid: libc::pid_t) -> std::io::Result<bool> {
    Ok(false)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::config::{LocalArtifactConfig, LocalLlamaSettings};
    use crate::test_support::{sha256_file, temp_root_path, write_fake_llama_server};
    use std::io::Write as _;
    use std::net::{TcpListener, TcpStream};
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Instant as StdInstant;

    fn fixture(
        mode: &str,
    ) -> (
        LocalLlamaEngines,
        LocalArtifactConfig,
        LocalArtifactConfig,
        LocalLlamaSettings,
        std::path::PathBuf,
    ) {
        let root = temp_root_path("model-provider-llama", mode);
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (engine_path, model_path, events) = write_fake_llama_server(&root, mode);
        let engine = LocalArtifactConfig {
            sha256: sha256_file(&engine_path),
            path: engine_path.to_string_lossy().into_owned(),
        };
        let model = LocalArtifactConfig {
            sha256: sha256_file(&model_path),
            path: model_path.to_string_lossy().into_owned(),
        };
        (
            LocalLlamaEngines::default(),
            engine,
            model,
            LocalLlamaSettings {
                context_size: 256,
                parallel: 1,
                threads: 1,
                batch_threads: 1,
                gpu_layers: 0,
                health_timeout_ms: 2_000,
                shutdown_timeout_ms: 250,
                enable_thinking: false,
            },
            events,
        )
    }

    fn event_lines(path: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn wait_for_event(path: &std::path::Path, prefix: &str) {
        let deadline = StdInstant::now() + Duration::from_secs(3);
        while !event_lines(path)
            .iter()
            .any(|line| line.starts_with(prefix))
        {
            assert!(
                StdInstant::now() < deadline,
                "missing fake engine event {prefix}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn recorded_pid(path: &std::path::Path) -> i32 {
        wait_for_event(path, "start:");
        event_lines(path)
            .into_iter()
            .find_map(|line| line.strip_prefix("start:").and_then(|pid| pid.parse().ok()))
            .unwrap()
    }

    fn process_exists(pid: i32) -> bool {
        (unsafe { libc::kill(pid, 0) }) == 0
    }

    #[test]
    fn engine_argument_vector_is_exact() {
        let settings = LocalLlamaSettings {
            context_size: 256,
            parallel: 2,
            threads: 3,
            batch_threads: 4,
            gpu_layers: 5,
            health_timeout_ms: 2_000,
            shutdown_timeout_ms: 250,
            enable_thinking: false,
        };
        let expected: Vec<OsString> = [
            "-m",
            "/models/model.gguf",
            "--host",
            "127.0.0.1",
            "--port",
            "11434",
            "--ctx-size",
            "256",
            "--parallel",
            "2",
            "--threads",
            "3",
            "--threads-batch",
            "4",
            "--gpu-layers",
            "5",
            "--alias",
            "private-alias",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();

        assert_eq!(
            engine_arguments("/models/model.gguf", &settings, 11434, "private-alias"),
            expected
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn exact_close_reaps_only_selected_engine_and_repeats_safely() {
        let (engines, engine, model, settings, events) = fixture("healthy");
        let (_, other_engine, other_model, other_settings, other_events) = fixture("healthy");
        engines
            .endpoint("first", &engine, &model, &settings)
            .await
            .unwrap();
        let other = engines
            .endpoint("second", &other_engine, &other_model, &other_settings)
            .await
            .unwrap();
        let first_pid = recorded_pid(&events);
        let second_pid = recorded_pid(&other_events);
        {
            let mut owned = engines.engines.lock().await;
            close_engine(&mut owned, "first").await.unwrap();
            close_engine(&mut owned, "first").await.unwrap();
            assert!(!owned.contains_key("first"));
            assert!(owned.contains_key("second"));
        }
        assert!(!process_exists(first_pid));
        assert!(process_exists(second_pid));
        assert!(engines.retains_artifacts().await);
        assert_eq!(
            engines
                .endpoint("second", &other_engine, &other_model, &other_settings)
                .await
                .unwrap(),
            other
        );
        engines.shutdown().await.unwrap();
        assert!(!process_exists(second_pid));
        assert!(!engines.retains_artifacts().await);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn uncertain_close_retains_exact_child_and_blocks_replacement() {
        let (engines, engine, model, settings, events) = fixture("healthy");
        engines
            .endpoint("offer", &engine, &model, &settings)
            .await
            .unwrap();
        let pid = recorded_pid(&events);
        let started = StdInstant::now();
        {
            let mut owned = engines.engines.lock().await;
            owned.get_mut("offer").unwrap().shutdown_timeout = Duration::ZERO;
            assert_eq!(
                close_engine(&mut owned, "offer").await,
                Err(LocalLlamaFault::Timeout)
            );
            let retained = owned.get("offer").unwrap();
            assert_eq!(retained.child.id(), Some(pid as u32));
            assert!(retained.closing);
        }
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(engines.retains_artifacts().await);
        assert_eq!(
            engines.endpoint("offer", &engine, &model, &settings).await,
            Err(LocalLlamaFault::Timeout)
        );
        assert!(process_exists(pid));
        assert_eq!(
            event_lines(&events)
                .iter()
                .filter(|line| line.starts_with("start:"))
                .count(),
            1
        );
        engines
            .engines
            .lock()
            .await
            .get_mut("offer")
            .unwrap()
            .shutdown_timeout = Duration::from_millis(250);
        engines.shutdown().await.unwrap();
        assert!(!process_exists(pid));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn wait_error_after_external_reap_retains_entry_and_denies_respawn() {
        let (engines, engine, model, settings, events) = fixture("healthy");
        engines
            .endpoint("offer", &engine, &model, &settings)
            .await
            .unwrap();
        let pid = recorded_pid(&events);
        assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
        let deadline = StdInstant::now() + Duration::from_secs(3);
        // Inject a real ECHILD wait failure by reaping outside Tokio's owner.
        // This is fault injection, not the production guard/group path.
        loop {
            let result = unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) };
            if result == pid {
                break;
            }
            assert_eq!(result, 0);
            assert!(StdInstant::now() < deadline);
            sleep(Duration::from_millis(10)).await;
        }
        assert!(!process_exists(pid));
        assert_eq!(engines.shutdown().await, Err(LocalLlamaFault::Failed));
        assert!(engines.retains_artifacts().await);
        assert_eq!(
            engines.endpoint("offer", &engine, &model, &settings).await,
            Err(LocalLlamaFault::Failed)
        );
        assert_eq!(
            event_lines(&events)
                .iter()
                .filter(|line| line.starts_with("start:"))
                .count(),
            1
        );
        let mut owned = engines.engines.lock().await;
        assert!(owned.get("offer").unwrap().closing);
        // The fixture already reaped this exact PID; only test cleanup removes it.
        owned.remove("offer");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn invalid_guard_identity_retains_ownership_without_signalling() {
        let (engines, engine, model, settings, events) = fixture("healthy");
        engines
            .endpoint("offer", &engine, &model, &settings)
            .await
            .unwrap();
        let pid = recorded_pid(&events);
        {
            let mut owned = engines.engines.lock().await;
            owned.get_mut("offer").unwrap().guard_group = Some(0);
            assert_eq!(
                close_engine(&mut owned, "offer").await,
                Err(LocalLlamaFault::Failed)
            );
            assert_eq!(owned.get("offer").unwrap().child.id(), Some(pid as u32));
            owned.get_mut("offer").unwrap().guard_group = None;
        }
        assert!(process_exists(pid));
        engines.shutdown().await.unwrap();
    }

    #[test]
    fn exited_guard_signal_exception_is_platform_and_state_scoped() {
        for guard_exited in [false, true] {
            for errno in [
                None,
                Some(libc::EPERM),
                Some(libc::EACCES),
                Some(libc::ESRCH),
            ] {
                assert_eq!(
                    exited_guard_signal_can_settle(guard_exited, errno),
                    cfg!(target_os = "macos") && guard_exited && errno == Some(libc::EPERM),
                );
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reaped_guard_does_not_signal_or_accept_surviving_group_member() {
        use std::os::unix::process::CommandExt as _;

        // Both fixture descendants are direct children of the test, so it can
        // reap each exactly. Group membership is the shutdown ownership boundary.
        let mut guard_command = Command::new("/bin/cat");
        guard_command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        guard_command.as_std_mut().process_group(0);
        let mut guard = guard_command.spawn().unwrap();
        let group = guard.id().unwrap() as libc::pid_t;
        let mut member_command = Command::new("/bin/cat");
        member_command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        member_command.as_std_mut().process_group(group);
        let mut member = member_command.spawn().unwrap();
        let member_pid = member.id().unwrap() as libc::pid_t;

        let guard_exit = guard.kill().await; // Owned wait leaves child.id() == None.
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            finish_guard_shutdown(&mut guard, group, Duration::from_millis(100)),
        )
        .await;
        let member_survived = process_exists(member_pid);
        // Clean up before checking outcomes, including the failure path. The
        // test owns these exact Child handles; it never signals an unowned group.
        let member_exit = member.kill().await;
        let guard_cleanup = if guard.id().is_some() {
            guard.kill().await
        } else {
            Ok(())
        };
        guard_exit.unwrap();
        member_exit.unwrap();
        guard_cleanup.unwrap();
        assert_eq!(result.unwrap(), Err(LocalLlamaFault::Timeout));
        assert!(
            member_survived,
            "post-reap shutdown signalled the surviving member"
        );
        assert!(!process_exists(member_pid));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_lock_wait_is_bounded_and_retains_engine() {
        let (engines, engine, model, settings, events) = fixture("healthy");
        engines
            .endpoint("offer", &engine, &model, &settings)
            .await
            .unwrap();
        let pid = recorded_pid(&events);
        let owned = engines.engines.lock().await;
        let started = StdInstant::now();
        assert_eq!(engines.shutdown().await, Err(LocalLlamaFault::Timeout));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(owned.get("offer").unwrap().child.id(), Some(pid as u32));
        drop(owned);
        engines.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn concurrent_start_reuses_one_child_and_repeated_stop_is_idempotent() {
        let (engines, engine, model, settings, events) = fixture("healthy");
        let (first, second) = tokio::join!(
            engines.endpoint("offer", &engine, &model, &settings),
            engines.endpoint("offer", &engine, &model, &settings),
        );
        let first = first.unwrap();
        let second = second.unwrap();
        assert_eq!(first.api_url, second.api_url);
        assert_eq!(first.model, second.model);
        let pid = recorded_pid(&events);
        assert_eq!(
            event_lines(&events)
                .iter()
                .filter(|line| line.starts_with("start:"))
                .count(),
            1
        );

        engines.shutdown().await.unwrap();
        engines.shutdown().await.unwrap();

        wait_for_event(&events, "term");
        assert_eq!(
            event_lines(&events)
                .iter()
                .filter(|line| *line == "term")
                .count(),
            1
        );
        assert!(!process_exists(pid));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dropping_last_manager_kills_its_child() {
        let (engines, engine, model, settings, events) = fixture("healthy");
        engines
            .endpoint("offer", &engine, &model, &settings)
            .await
            .unwrap();
        let pid = recorded_pid(&events);

        drop(engines);

        let deadline = StdInstant::now() + Duration::from_secs(3);
        while process_exists(pid) && StdInstant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!process_exists(pid));
    }

    async fn assert_unhealthy_reuse_restarts(marker_extension: &str) {
        let (engines, engine, model, settings, events) = fixture("mutable_readiness");
        let first = engines
            .endpoint("offer", &engine, &model, &settings)
            .await
            .unwrap();
        let first_pid = recorded_pid(&events);
        std::fs::write(
            std::path::Path::new(&model.path).with_extension(marker_extension),
            b"stale",
        )
        .unwrap();

        let second = engines
            .endpoint("offer", &engine, &model, &settings)
            .await
            .unwrap();

        assert_ne!(second.model, first.model);
        assert!(!process_exists(first_pid));
        assert_eq!(
            event_lines(&events)
                .iter()
                .filter(|line| line.starts_with("start:"))
                .count(),
            2
        );
        engines.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn live_unresponsive_child_is_reaped_and_restarted_once() {
        assert_unhealthy_reuse_restarts("unresponsive").await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn live_wrong_alias_child_is_reaped_and_restarted_once() {
        assert_unhealthy_reuse_restarts("wrong-alias").await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn failed_replacement_returns_once_and_leaves_no_child() {
        let (engines, engine, model, mut settings, events) = fixture("healthy");
        engines
            .endpoint("offer", &engine, &model, &settings)
            .await
            .unwrap();
        std::fs::write(
            std::path::Path::new(&model.path).with_extension("unresponsive"),
            b"stale",
        )
        .unwrap();
        settings.health_timeout_ms = 300;
        settings.shutdown_timeout_ms = 100;

        assert_eq!(
            engines.endpoint("offer", &engine, &model, &settings).await,
            Err(LocalLlamaFault::Timeout)
        );

        let pids: Vec<i32> = event_lines(&events)
            .into_iter()
            .filter_map(|line| line.strip_prefix("start:")?.parse().ok())
            .collect();
        assert_eq!(pids.len(), 2);
        assert!(pids.into_iter().all(|pid| !process_exists(pid)));
        engines.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn health_timeout_forces_reap_when_child_ignores_termination() {
        let (engines, engine, model, mut settings, events) = fixture("timeout_ignore_term");
        settings.health_timeout_ms = 1_000;
        settings.shutdown_timeout_ms = 50;

        assert_eq!(
            engines.endpoint("offer", &engine, &model, &settings).await,
            Err(LocalLlamaFault::Timeout)
        );
        let pid = recorded_pid(&events);
        assert!(!process_exists(pid));
        engines.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn wrong_model_digest_fails_before_child_start() {
        let (engines, engine, mut model, settings, events) = fixture("healthy");
        model.sha256 =
            "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_string();

        assert_eq!(
            engines.endpoint("offer", &engine, &model, &settings).await,
            Err(LocalLlamaFault::Failed)
        );
        assert!(event_lines(&events).is_empty());
        engines.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn wrong_server_on_selected_port_cannot_satisfy_child_readiness() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = stop.clone();
        let server = thread::spawn(move || {
            while !stop_for_thread.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => respond_with_wrong_model(&mut stream),
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(err) => panic!("wrong-server fixture failed: {err}"),
                }
            }
        });
        let mut child = Command::new("/usr/bin/python3")
            .arg("-c")
            .arg("import time; time.sleep(5)")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        assert_eq!(
            wait_until_healthy(
                &mut child,
                None,
                port,
                "unpredictable-expected-alias",
                Duration::from_millis(100),
            )
            .await,
            Err(LocalLlamaFault::Timeout)
        );
        terminate_child(&mut child, &mut None, None, Duration::from_millis(100))
            .await
            .unwrap();
        stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(("127.0.0.1", port));
        server.join().unwrap();
    }

    fn respond_with_wrong_model(stream: &mut TcpStream) {
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request);
        let body = br#"{"object":"list","data":[{"id":"wrong-model"}]}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
    }
}
