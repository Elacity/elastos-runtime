//! Linux model-provider socket boundary. The provider and its descendants may
//! create Unix sockets for the private local engine path, but cannot create
//! Internet sockets. Runtime closes all other inherited descriptors on exec.

use std::io;
use tokio::process::Command;

const LD_W_ABS: u16 = 0x20;
const JEQ_K: u16 = 0x15;
const JGE_K: u16 = 0x35;
const RET_K: u16 = 0x06;
const RET_ALLOW: u32 = 0x7fff_0000;
const RET_KILL_PROCESS: u32 = 0x8000_0000;
const RET_ERRNO: u32 = 0x0005_0000 | libc::EPERM as u32;
const CLOSE_RANGE_CLOEXEC: u32 = 4;

#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: u32 = 0xc000_003e;
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: u32 = 0xc000_00b7;

fn stmt(code: u16, k: u32) -> libc::sock_filter {
    libc::sock_filter {
        code,
        jt: 0,
        jf: 0,
        k,
    }
}

fn jump(code: u16, k: u32, jt: u8, jf: u8) -> libc::sock_filter {
    libc::sock_filter { code, jt, jf, k }
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub(super) fn install_on_command(command: &mut Command) -> io::Result<()> {
    let mut code = vec![
        stmt(LD_W_ABS, 4), // seccomp_data.arch
        jump(JEQ_K, AUDIT_ARCH, 1, 0),
        stmt(RET_K, RET_KILL_PROCESS),
        stmt(LD_W_ABS, 0), // seccomp_data.nr
        jump(JGE_K, 0x4000_0000, 0, 1),
        stmt(RET_K, RET_KILL_PROCESS),
    ];
    for syscall in [libc::SYS_socket, libc::SYS_socketpair] {
        code.extend([
            jump(JEQ_K, syscall as u32, 0, 4),
            stmt(LD_W_ABS, 16), // seccomp_data.args[0], low 32 bits
            jump(JEQ_K, libc::AF_UNIX as u32, 1, 0),
            stmt(RET_K, RET_ERRNO),
            stmt(RET_K, RET_ALLOW),
        ]);
    }
    // These syscalls could create sockets or receive an external descriptor
    // without the socket() gate. Ordinary local HTTP uses read/write instead.
    for syscall in [
        libc::SYS_io_uring_setup,
        libc::SYS_pidfd_getfd,
        libc::SYS_recvmsg,
        libc::SYS_recvmmsg,
        libc::SYS_sendmsg,
        libc::SYS_sendmmsg,
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
    ] {
        code.extend([jump(JEQ_K, syscall as u32, 0, 1), stmt(RET_K, RET_ERRNO)]);
    }
    code.push(stmt(RET_K, RET_ALLOW));
    let length =
        u16::try_from(code.len()).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    // All allocation occurs in Runtime before fork. The pre-exec closure only
    // makes raw syscalls and returns an errno if confinement cannot attach.
    unsafe {
        use std::os::unix::process::CommandExt as _;
        command.as_std_mut().pre_exec(move || {
            if libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, CLOSE_RANGE_CLOEXEC) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            let program = libc::sock_fprog {
                len: length,
                filter: code.as_mut_ptr(),
            };
            if libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub(super) fn install_on_command(_: &mut Command) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Linux model socket confinement is unavailable on this architecture",
    ))
}

#[cfg(all(test, any(target_arch = "x86_64", target_arch = "aarch64")))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn child_and_descendant_have_no_inet_sockets() {
        let name =
            "provider::linux_model_seccomp::tests::child_and_descendant_have_no_inet_sockets";
        let executable = std::env::current_exe().unwrap();
        if let Ok(stage) = std::env::var("ELASTOS_LINUX_MODEL_SECCOMP_PROBE") {
            assert_eq!(
                unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) },
                1
            );
            assert_eq!(unsafe { libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) }, 2);
            for domain in [libc::AF_INET, libc::AF_INET6] {
                let fd = unsafe { libc::socket(domain, libc::SOCK_STREAM, 0) };
                assert_eq!(fd, -1);
                assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
            }
            let unix = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
            assert!(unix >= 0);
            unsafe { libc::close(unix) };
            if stage == "child" {
                let output = std::process::Command::new(&executable)
                    .arg("--exact")
                    .arg(name)
                    .arg("--nocapture")
                    .env("ELASTOS_LINUX_MODEL_SECCOMP_PROBE", "descendant")
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(
                    String::from_utf8_lossy(&output.stdout).contains("descendant-probe-ok"),
                    "{}",
                    String::from_utf8_lossy(&output.stdout)
                );
            }
            println!("{stage}-probe-ok");
            return;
        }
        let mut command = Command::new(executable);
        command
            .arg("--exact")
            .arg(name)
            .arg("--nocapture")
            .env("ELASTOS_LINUX_MODEL_SECCOMP_PROBE", "child")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        install_on_command(&mut command).unwrap();
        let output = command.output().await.unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("child-probe-ok"),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
