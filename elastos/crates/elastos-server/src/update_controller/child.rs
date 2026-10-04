//! The update controller owns one unreaped child and its process group.

use anyhow::{Context, Result};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::time::Duration;
use tokio::process::{Child, Command};

const PARENT_PIPE_ENV: &str = "ELASTOS_UPDATE_PARENT_PIPE";
const GRACE_PERIOD: Duration = Duration::from_secs(12);
const REAP_PERIOD: Duration = Duration::from_secs(2);
const POLL_PERIOD: Duration = Duration::from_millis(20);
const DROP_PERIOD: Duration = Duration::from_millis(200);

pub(crate) struct OwnedChild {
    child: Option<Child>,
    pid: u32,
    parent_lifetime: Option<OwnedFd>,
    status: Option<ExitStatus>,
}

impl OwnedChild {
    /// `command` is a fresh, single-use command for an already verified binary.
    pub(crate) fn spawn(command: &mut Command) -> Result<Self> {
        let (reader, writer) = parent_pipe().context("create child ownership pipe")?;
        let reader_fd = reader.as_raw_fd();
        let writer_fd = writer.as_raw_fd();
        let identity = pipe_identity(reader_fd)?;
        command
            .env(
                PARENT_PIPE_ENV,
                format!("{reader_fd}:{}:{}", identity.0, identity.1),
            )
            .process_group(0)
            .kill_on_drop(false);
        #[cfg(target_os = "linux")]
        let parent_pid = std::process::id() as libc::pid_t;
        unsafe {
            command.pre_exec(move || {
                // Only the reader survives exec. Other children retain CLOEXEC.
                libc::close(writer_fd);
                set_cloexec(reader_fd, false)?;
                #[cfg(target_os = "linux")]
                {
                    // Covers parent loss in the loader, before Rust can watch EOF.
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::getppid() != parent_pid {
                        libc::kill(libc::getpid(), libc::SIGTERM);
                    }
                }
                Ok(())
            });
        }
        let child = command.spawn().context("spawn owned update child")?;
        let pid = child.id().expect("newly spawned child has a process ID");
        drop(reader);
        Ok(Self {
            child: Some(child),
            pid,
            parent_lifetime: Some(writer),
            status: None,
        })
    }

    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }

    /// Observes exit without releasing the PID which anchors our process group.
    pub(crate) fn observed_exit(&self) -> Result<Option<ExitStatus>> {
        if self.child.is_none() {
            return Ok(self.status);
        }
        observe_exit(self.pid).context("observe owned update child")
    }

    /// Sends the final group signal before reaping, including on early exit.
    pub(crate) async fn stop(&mut self) -> Result<()> {
        self.stop_with_limits(GRACE_PERIOD, REAP_PERIOD).await
    }

    async fn stop_with_limits(&mut self, grace: Duration, reap: Duration) -> Result<()> {
        if self.child.is_none() {
            return Ok(());
        }
        let deadline = tokio::time::Instant::now() + grace + reap;
        let graceful_deadline = deadline - reap;
        // Successful waitid also proves that this PID is still our direct child.
        self.observed_exit()?;
        self.parent_lifetime.take();
        signal_group(self.pid, libc::SIGTERM)?;
        while self.observed_exit()?.is_none() && tokio::time::Instant::now() < graceful_deadline {
            tokio::time::sleep_until(
                (tokio::time::Instant::now() + POLL_PERIOD).min(graceful_deadline),
            )
            .await;
        }
        // A crashed leader can leave helpers with inherited stdout or stderr.
        // Its unreaped PID reserves the group ID until this last group signal.
        self.observed_exit()?;
        signal_group(self.pid, libc::SIGKILL)?;
        // Keep the anchor until helpers, including adopted zombies, are gone.
        // An uninterruptible helper holds the transaction at this same deadline.
        loop {
            let members = group_descendants(self.pid)?;
            if members.is_empty() {
                break;
            }
            anyhow::ensure!(
                tokio::time::Instant::now() < deadline,
                "owned update group still has {} descendants at shutdown deadline",
                members.len()
            );
            tokio::time::sleep_until((tokio::time::Instant::now() + POLL_PERIOD).min(deadline))
                .await;
        }
        let child = self.child.as_mut().expect("owned child remains until reap");
        let status = tokio::time::timeout_at(deadline, child.wait())
            .await
            .context("owned update child exceeded shutdown deadline")?
            .context("reap owned update child")?;
        self.status = Some(status);
        self.child.take();
        Ok(())
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.child.is_none() {
            return;
        }
        // The same ownership check protects cancellation and unwinding paths.
        if self.observed_exit().is_ok() {
            let _ = signal_group(self.pid, libc::SIGKILL);
        }
        self.parent_lifetime.take();
        let deadline = std::time::Instant::now() + DROP_PERIOD;
        while std::time::Instant::now() < deadline {
            match group_descendants(self.pid) {
                Ok(members) if !members.is_empty() => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                _ => break,
            }
        }
        let child = self.child.as_mut().expect("owned child remains until reap");
        loop {
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(None) => break,
            }
        }
        // Tokio owns final reaping if the kernel cannot finish within this bound.
        // kill_on_drop is disabled: only our checked group receives signals.
    }
}

pub(crate) fn observe_exit(pid: u32) -> io::Result<Option<ExitStatus>> {
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result != 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if unsafe { info.si_pid() } == 0 {
            return Ok(None);
        }
        let status = unsafe { info.si_status() };
        let raw_status = match info.si_code {
            libc::CLD_EXITED => status << 8,
            libc::CLD_KILLED => status,
            libc::CLD_DUMPED => status | 0x80,
            _ => return Err(io::Error::other("unexpected owned child exit event")),
        };
        return Ok(Some(ExitStatus::from_raw(raw_status)));
    }
}

fn signal_group(pid: u32, signal: libc::c_int) -> io::Result<()> {
    if unsafe { libc::kill(-(pid as libc::pid_t), signal) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

/// Read process birth from the kernel without starting a helper process.
pub(crate) fn process_start(pid: u32) -> Option<String> {
    process_identity(pid).ok().flatten()
}

/// Recovery observes an old generation; it gives signal authority only to owners.
/// A reused PID or incomplete kernel view keeps restoration blocked.
pub(crate) fn generation_gone(pid: u32, start: &str) -> Result<bool> {
    for inspection in 0..2 {
        if let Some(current) = process_identity(pid)? {
            anyhow::ensure!(
                current == start,
                "update child process ID belongs to another generation"
            );
            return Ok(false);
        }
        // macOS hides zombie birth records from libproc. A direct-child event
        // still proves that this process has a reserved PID and needs reaping.
        match observe_exit(pid) {
            Ok(_) => return Ok(false),
            Err(error) if error.raw_os_error() == Some(libc::ECHILD) => {}
            Err(error) => return Err(error.into()),
        }
        if inspection == 0 && !group_descendants(pid)?.is_empty() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn valid_pid(pid: u32) -> io::Result<()> {
    if pid == 0 || pid > libc::pid_t::MAX as u32 {
        return Err(io::Error::other("invalid update child process ID"));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn process_identity(pid: u32) -> io::Result<Option<String>> {
    valid_pid(pid)?;
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let start = linux_stat_start(&stat)?;
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|boot| boot.trim().to_owned())
        .filter(|boot| !boot.is_empty());
    Ok(Some(match boot {
        Some(boot) => format!("linux:{boot}:{start}"),
        None => format!("linux:{start}"),
    }))
}

#[cfg(target_os = "macos")]
pub(crate) fn process_identity(pid: u32) -> io::Result<Option<String>> {
    valid_pid(pid)?;
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let expected = std::mem::size_of_val(&info) as libc::c_int;
    unsafe { *libc::__error() = 0 };
    let bytes = unsafe {
        libc::proc_pidinfo(
            pid as libc::pid_t,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            expected,
        )
    };
    if bytes == expected && info.pbi_pid == pid {
        return Ok(Some(format!(
            "macos:{}:{}",
            info.pbi_start_tvsec, info.pbi_start_tvusec
        )));
    }
    let error = io::Error::last_os_error();
    if bytes == 0 && error.raw_os_error() == Some(libc::ESRCH) {
        // An inaccessible or unreaped process is ambiguous even if libproc
        // cannot supply its birth record. Signal zero is an existence check.
        if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0
            && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            return Ok(None);
        }
    }
    Err(if error.raw_os_error() != Some(0) {
        error
    } else {
        io::Error::other("incomplete update child birth record")
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn process_identity(_pid: u32) -> io::Result<Option<String>> {
    Err(io::Error::other(
        "update process birth proof is unsupported on this platform",
    ))
}

#[cfg(target_os = "linux")]
pub(crate) fn group_descendants(leader: u32) -> io::Result<Vec<u32>> {
    let mut members = Vec::new();
    for (index, entry) in std::fs::read_dir("/proc")?.enumerate() {
        if index >= 16_384 {
            return Err(io::Error::other("process group scan exceeded its bound"));
        }
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == leader {
            continue;
        }
        let stat = match std::fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if linux_stat_group(&stat)? == leader {
            members.push(pid);
        }
    }
    Ok(members)
}

#[cfg(any(target_os = "linux", test))]
fn linux_stat_group(stat: &str) -> io::Result<u32> {
    // A process name can contain spaces and closing parentheses.
    let group = stat
        .rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(2));
    group
        .and_then(|group| group.parse().ok())
        .ok_or_else(|| io::Error::other("invalid process group in proc stat"))
}

#[cfg(any(target_os = "linux", test))]
fn linux_stat_start(stat: &str) -> io::Result<u64> {
    stat.rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .and_then(|start| start.parse().ok())
        .ok_or_else(|| io::Error::other("invalid process birth in proc stat"))
}

#[cfg(target_os = "macos")]
pub(crate) fn group_descendants(leader: u32) -> io::Result<Vec<u32>> {
    // PROC_PGRP_ONLY from the SDK's sys/proc_info.h; libc exports the API.
    const PROC_PGRP_ONLY: u32 = 2;
    let mut pids = [0 as libc::pid_t; 4096];
    let buffer_size = std::mem::size_of_val(&pids) as libc::c_int;
    unsafe { *libc::__error() = 0 };
    let bytes = unsafe {
        libc::proc_listpids(
            PROC_PGRP_ONLY,
            leader,
            pids.as_mut_ptr().cast(),
            buffer_size,
        )
    };
    if bytes < 0 || (bytes == 0 && io::Error::last_os_error().raw_os_error() != Some(0)) {
        return Err(io::Error::last_os_error());
    }
    if bytes >= buffer_size || bytes as usize % std::mem::size_of::<libc::pid_t>() != 0 {
        return Err(io::Error::other(
            "owned process group listing exceeded its bound",
        ));
    }
    let mut members = Vec::new();
    for &pid in &pids[..bytes as usize / std::mem::size_of::<libc::pid_t>()] {
        if pid <= 0 || pid as u32 == leader {
            continue;
        }
        let mut info: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
        let expected = std::mem::size_of_val(&info) as libc::c_int;
        unsafe { *libc::__error() = 0 };
        let bytes = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDT_SHORTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdshortinfo).cast(),
                expected,
            )
        };
        if bytes != expected {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                // A member can exit between list and info. Keep it pending until
                // the next group list also proves that its PID has disappeared.
                members.push(pid as u32);
                continue;
            }
            return Err(if bytes == 0 && error.raw_os_error() != Some(0) {
                error
            } else {
                io::Error::other("incomplete owned process group member info")
            });
        }
        if info.pbsi_pgid == leader {
            members.push(pid as u32);
        }
    }
    Ok(members)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn group_descendants(_leader: u32) -> io::Result<Vec<u32>> {
    Err(io::Error::other(
        "owned process group proof is unsupported on this platform",
    ))
}

pub(crate) fn process_group(pid: u32) -> io::Result<Option<u32>> {
    valid_pid(pid)?;
    let group = unsafe { libc::getpgid(pid as libc::pid_t) };
    if group >= 0 {
        return Ok(Some(group as u32));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(None)
    } else {
        Err(error)
    }
}

fn set_cloexec(fd: libc::c_int, enabled: bool) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let flags = if enabled {
        flags | libc::FD_CLOEXEC
    } else {
        flags & !libc::FD_CLOEXEC
    };
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn parent_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    #[cfg(target_os = "linux")]
    let result = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    #[cfg(not(target_os = "linux"))]
    let result = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    let reader = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let writer = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    #[cfg(not(target_os = "linux"))]
    {
        set_cloexec(reader.as_raw_fd(), true)?;
        set_cloexec(writer.as_raw_fd(), true)?;
    }
    // A parent with a closed standard descriptor still passes a separate pipe.
    Ok((above_stdio(reader)?, above_stdio(writer)?))
}

fn above_stdio(fd: OwnedFd) -> io::Result<OwnedFd> {
    if fd.as_raw_fd() >= 3 {
        return Ok(fd);
    }
    let duplicate = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
}

fn pipe_identity(fd: libc::c_int) -> io::Result<(u64, u64)> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if stat.st_mode & libc::S_IFMT != libc::S_IFIFO {
        return Err(io::Error::other("update parent descriptor is not a pipe"));
    }
    #[cfg(target_os = "macos")]
    let device = stat.st_dev as u64;
    #[cfg(not(target_os = "macos"))]
    let device = stat.st_dev;
    Ok((device, stat.st_ino))
}

/// Call after installing SIGTERM handling and before launching subordinate groups.
/// The inherited descriptor belongs only to the controller's direct child.
/// Graceful exit drains owned groups before returning; its process exit also
/// ends this watcher's force timer. Recovery retains incomplete group cleanup.
pub(crate) fn watch_parent() -> Result<()> {
    let Ok(value) = std::env::var(PARENT_PIPE_ENV) else {
        return Ok(());
    };
    let mut fields = value.split(':');
    let fd: libc::c_int = fields
        .next()
        .context("missing update parent pipe descriptor")?
        .parse()?;
    let device: u64 = fields
        .next()
        .context("missing update parent pipe device")?
        .parse()?;
    let inode: u64 = fields
        .next()
        .context("missing update parent pipe inode")?
        .parse()?;
    anyhow::ensure!(
        fd >= 3 && fields.next().is_none(),
        "invalid update parent pipe"
    );
    // Descendant execs keep the marker in their environment, but CLOEXEC closes
    // the reader. Its exact identity also rejects a reused descriptor number.
    if pipe_identity(fd).ok() != Some((device, inode)) {
        return Ok(());
    }
    set_cloexec(fd, true)?;
    let reader = unsafe { OwnedFd::from_raw_fd(fd) };
    std::thread::Builder::new()
        .name("update-parent-watch".into())
        .spawn(move || {
            let mut byte = 0u8;
            loop {
                let result =
                    unsafe { libc::read(reader.as_raw_fd(), (&mut byte as *mut u8).cast(), 1) };
                if result > 0 {
                    continue;
                }
                if result < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            // This thread has the same lifetime as this process, so self-signals
            // remain tied to the owned child even if its controller has exited.
            unsafe { libc::kill(libc::getpid(), libc::SIGTERM) };
            std::thread::sleep(GRACE_PERIOD);
            unsafe {
                let pid = libc::getpid();
                if libc::getpgrp() == pid {
                    libc::kill(-pid, libc::SIGKILL);
                } else {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        })
        .context("start update parent watcher")?;
    #[cfg(target_os = "linux")]
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, 0) } != 0 {
        return Err(io::Error::last_os_error()).context("transfer parent watch to ownership pipe");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::Stdio;

    async fn wait_for_file(path: &Path) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while !path.exists() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "child did not become ready"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn wait_for_exit(child: &OwnedChild) -> ExitStatus {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = child.observed_exit().unwrap() {
                return status;
            }
            assert!(tokio::time::Instant::now() < deadline, "child did not exit");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn stop_reaps_helpers_with_inherited_output_and_preserves_another_group() {
        let temp = tempfile::tempdir().unwrap();
        let ready = temp.path().join("ready");
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 60 & helper=$!; trap 'kill $helper 2>/dev/null; wait $helper 2>/dev/null; exit 0' TERM; touch \"$1\"; wait $helper", "owned-child"])
            .arg(&ready).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = OwnedChild::spawn(&mut command).unwrap();
        let pid = child.pid();
        let birth = process_start(pid).unwrap();
        assert!(!generation_gone(pid, &birth).unwrap());
        let mut other_command = Command::new("sleep");
        other_command.arg("60");
        let mut other = OwnedChild::spawn(&mut other_command).unwrap();
        wait_for_file(&ready).await;
        child.stop().await.unwrap();
        assert!(generation_gone(pid, &birth).unwrap());
        assert_eq!(
            observe_exit(pid).unwrap_err().raw_os_error(),
            Some(libc::ECHILD)
        );
        assert!(other.observed_exit().unwrap().is_none());
        other.stop().await.unwrap();
    }

    #[tokio::test]
    async fn hung_child_is_killed_within_the_shutdown_bound() {
        let temp = tempfile::tempdir().unwrap();
        let ready = temp.path().join("ready");
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                "trap '' TERM; touch \"$1\"; while :; do :; done",
                "owned-child",
            ])
            .arg(&ready);
        let mut child = OwnedChild::spawn(&mut command).unwrap();
        let pid = child.pid();
        wait_for_file(&ready).await;
        tokio::time::timeout(
            Duration::from_millis(500),
            child.stop_with_limits(Duration::from_millis(30), Duration::from_millis(200)),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            child.observed_exit().unwrap().unwrap().signal(),
            Some(libc::SIGKILL)
        );
        assert_eq!(
            observe_exit(pid).unwrap_err().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[tokio::test]
    async fn crash_stays_unreaped_until_group_cleanup_and_drop_reaps() {
        let mut command = Command::new("sh");
        command.args(["-c", "kill -KILL $$"]);
        let child = OwnedChild::spawn(&mut command).unwrap();
        let pid = child.pid();
        assert_eq!(wait_for_exit(&child).await.signal(), Some(libc::SIGKILL));
        assert!(observe_exit(pid).unwrap().is_some());
        drop(child);
        assert_eq!(
            observe_exit(pid).unwrap_err().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[tokio::test]
    async fn crashed_leader_keeps_its_anchor_until_term_resistant_helper_is_gone() {
        let temp = tempfile::tempdir().unwrap();
        let helper_ready = temp.path().join("helper");
        let mut command = Command::new("sh");
        command.args(["-c", "sh -c 'trap \"\" TERM; touch \"$1\"; while :; do :; done' owned-helper \"$1\" & while [ ! -f \"$1\" ]; do :; done; kill -KILL $$", "owned-child"])
            .arg(&helper_ready);
        let mut child = OwnedChild::spawn(&mut command).unwrap();
        let pid = child.pid();
        assert_eq!(wait_for_exit(&child).await.signal(), Some(libc::SIGKILL));
        assert!(!group_descendants(pid).unwrap().is_empty());
        child.stop().await.unwrap();
        assert!(group_descendants(pid).unwrap().is_empty());
        assert_eq!(
            observe_exit(pid).unwrap_err().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[test]
    fn proc_stat_parser_handles_spaces_and_parentheses_in_process_names() {
        assert_eq!(
            linux_stat_group("12 (strange ) process) Z 1 17 19 0").unwrap(),
            17
        );
        assert!(linux_stat_group("12 (unfinished").is_err());
        assert_eq!(
            linux_stat_start(
                "12 (strange ) process) Z 1 17 19 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 12345"
            )
            .unwrap(),
            12345
        );
        assert!(linux_stat_start("12 (unfinished").is_err());
    }

    #[test]
    fn generation_observation_preserves_live_and_reused_process_ids() {
        let pid = std::process::id();
        let birth = process_start(pid).unwrap();
        assert!(!generation_gone(pid, &birth).unwrap());
        assert!(generation_gone(pid, "another-generation").is_err());
        assert!(process_start(0).is_none());
        assert!(generation_gone(0, "invalid").is_err());
    }

    #[tokio::test]
    async fn loader_failure_returns_without_an_owned_child() {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("invalid-executable");
        std::fs::write(&binary, b"#!/elastos-test-missing-loader/interpreter\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(OwnedChild::spawn(&mut Command::new(binary)).is_err());
    }

    #[tokio::test]
    async fn parent_pipe_loss_starts_graceful_shutdown() {
        let temp = tempfile::tempdir().unwrap();
        let ready = temp.path().join("ready");
        let fixture = format!(
            "{}::parent_watch_fixture",
            module_path!().split_once("::").unwrap().1
        );
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", &fixture, "--ignored", "--nocapture"])
            .env("ELASTOS_UPDATE_PARENT_WATCH_TEST_READY", &ready)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = OwnedChild::spawn(&mut command).unwrap();
        wait_for_file(&ready).await;
        child.parent_lifetime.take();
        assert!(wait_for_exit(&child).await.success());
        let pid = child.pid();
        child.stop().await.unwrap();
        assert_eq!(
            observe_exit(pid).unwrap_err().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[tokio::test]
    #[ignore = "subprocess fixture for parent_pipe_loss_starts_graceful_shutdown"]
    async fn parent_watch_fixture() {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        watch_parent().unwrap();
        let ready = std::path::PathBuf::from(
            std::env::var_os("ELASTOS_UPDATE_PARENT_WATCH_TEST_READY").unwrap(),
        );
        let helper_ready = ready.with_extension("helper");
        let mut helper_command = Command::new("sh");
        helper_command
            .args([
                "-c",
                "trap '' TERM; touch \"$1\"; while :; do :; done",
                "parent-loss-helper",
            ])
            .arg(&helper_ready);
        let mut helper = OwnedChild::spawn(&mut helper_command).unwrap();
        wait_for_file(&helper_ready).await;
        std::fs::write(&ready, b"ready").unwrap();
        terminate.recv().await;
        // Mirrors gateway shutdown: the leader drains its separate owned groups.
        helper
            .stop_with_limits(Duration::from_millis(30), Duration::from_millis(200))
            .await
            .unwrap();
    }
}
