//! Parent lifetime is an inherited pipe. Its sole writer belongs to the owner.
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::Command;

pub const PARENT_PIPE_ENV: &str = "ELASTOS_UPDATE_PARENT_PIPE";

/// Keeps helpers owned across short-lived launch commands and parent crashes.
pub struct ParentLifetime {
    reader: OwnedFd,
    _writer: OwnedFd,
}

impl ParentLifetime {
    pub fn new() -> io::Result<Self> {
        let (reader, writer) = parent_pipe()?;
        Ok(Self {
            reader,
            _writer: writer,
        })
    }

    /// Forward the Runtime's reader through a transient provider or launcher.
    pub fn configure_inherited(command: &mut Command) -> io::Result<bool> {
        let Ok(marker) = std::env::var(PARENT_PIPE_ENV) else {
            return Ok(false);
        };
        let fields: Vec<_> = marker.split(':').collect();
        if fields.len() != 3 {
            return Err(io::Error::other("invalid parent pipe marker"));
        }
        let fd: libc::c_int = fields[0].parse().map_err(io::Error::other)?;
        let device: u64 = fields[1].parse().map_err(io::Error::other)?;
        let inode: u64 = fields[2].parse().map_err(io::Error::other)?;
        if fd < 3 || pipe_identity(fd)? != (device, inode) {
            return Err(io::Error::other("parent pipe identity changed"));
        }
        command.env(PARENT_PIPE_ENV, marker);
        unsafe {
            command.pre_exec(move || set_cloexec(fd, false));
        }
        Ok(true)
    }

    pub fn configure(&self, command: &mut Command) -> io::Result<()> {
        let reader = self.reader.as_raw_fd();
        let writer = self._writer.as_raw_fd();
        let (device, inode) = pipe_identity(reader)?;
        command.env(PARENT_PIPE_ENV, format!("{reader}:{device}:{inode}"));
        unsafe {
            command.pre_exec(move || {
                libc::close(writer);
                set_cloexec(reader, false)
            });
        }
        Ok(())
    }
}

pub fn set_cloexec(fd: libc::c_int, enabled: bool) -> io::Result<()> {
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

pub fn parent_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
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

pub fn pipe_identity(fd: libc::c_int) -> io::Result<(u64, u64)> {
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
