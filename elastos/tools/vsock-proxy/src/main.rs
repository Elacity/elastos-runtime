//! Guest bridge helper for crosvm VMs
//!
//! Historically this proxied TCP connections to AF_VSOCK. In the current
//! path, the same binary also exposes provider bridges on the guest-network
//! compatibility path for capsules that explicitly request a guest NIC.
//!
//! Usage: vsock-proxy <tcp-port> <vsock-cid> <vsock-port>
//! Example: vsock-proxy 3000 2 3000
//!   Listens on TCP 127.0.0.1:3000, forwards to vsock CID 2 port 3000

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::io::FromRawFd;
use std::process::{Command, Stdio};
use std::thread;

// Vsock address family (from linux/socket.h)
const AF_VSOCK: i32 = 40;
// Vsock socket type
const SOCK_STREAM: i32 = 1;
// Wildcard CID for bind/listen in guest.
const VMADDR_CID_ANY: u32 = 0xFFFFFFFF;

// Vsock sockaddr structure
#[repr(C)]
struct SockaddrVm {
    svm_family: u16,
    svm_reserved1: u16,
    svm_port: u32,
    svm_cid: u32,
    svm_zero: [u8; 4],
}

fn create_vsock_connection(cid: u32, port: u32) -> std::io::Result<std::fs::File> {
    unsafe {
        // Create vsock socket
        let fd = libc::socket(AF_VSOCK, SOCK_STREAM, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }

        // Build address
        let addr = SockaddrVm {
            svm_family: AF_VSOCK as u16,
            svm_reserved1: 0,
            svm_port: port,
            svm_cid: cid,
            svm_zero: [0; 4],
        };

        // Connect
        let result = libc::connect(
            fd,
            &addr as *const SockaddrVm as *const libc::sockaddr,
            std::mem::size_of::<SockaddrVm>() as u32,
        );

        if result < 0 {
            libc::close(fd);
            return Err(std::io::Error::last_os_error());
        }

        // Convert to File for easier handling
        Ok(std::fs::File::from_raw_fd(fd))
    }
}

fn create_vsock_listener(port: u32) -> std::io::Result<i32> {
    unsafe {
        let fd = libc::socket(AF_VSOCK, SOCK_STREAM, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }

        let addr = SockaddrVm {
            svm_family: AF_VSOCK as u16,
            svm_reserved1: 0,
            svm_port: port,
            svm_cid: VMADDR_CID_ANY,
            svm_zero: [0; 4],
        };

        let rc = libc::bind(
            fd,
            &addr as *const SockaddrVm as *const libc::sockaddr,
            std::mem::size_of::<SockaddrVm>() as u32,
        );
        if rc < 0 {
            let err = std::io::Error::last_os_error();
            libc::close(fd);
            return Err(err);
        }

        if libc::listen(fd, 8) < 0 {
            let err = std::io::Error::last_os_error();
            libc::close(fd);
            return Err(err);
        }

        Ok(fd)
    }
}

fn proxy_connection(tcp_stream: TcpStream, vsock_cid: u32, vsock_port: u32) {
    // Connect to vsock
    let vsock_stream = match create_vsock_connection(vsock_cid, vsock_port) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to connect to vsock {}:{}: {}", vsock_cid, vsock_port, e);
            return;
        }
    };

    // Clone for bidirectional proxying
    let mut tcp_read = tcp_stream.try_clone().unwrap();
    let mut tcp_write = tcp_stream;
    let mut vsock_read = vsock_stream.try_clone().unwrap();
    let mut vsock_write = vsock_stream;

    // TCP -> vsock
    let handle1 = thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match tcp_read.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if vsock_write.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // vsock -> TCP
    let handle2 = thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match vsock_read.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tcp_write.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let _ = handle1.join();
    let _ = handle2.join();
}

/// Forward lines from a vsock connection to the persistent provider process.
///
/// Reads one JSON line from the vsock client, forwards to provider stdin,
/// reads one JSON line response from provider stdout, sends back to client.
/// Loops until the client closes the connection.
fn handle_provider_session(
    client_fd: i32,
    child_stdin: &mut impl Write,
    child_stdout: &mut impl BufRead,
) -> std::io::Result<()> {
    // SAFETY: accepted fd is uniquely owned by this function.
    let socket = unsafe { std::fs::File::from_raw_fd(client_fd) };
    let mut socket_reader = BufReader::new(socket.try_clone()?);
    let mut socket_writer = socket;

    loop {
        let mut request = String::new();
        let n = socket_reader.read_line(&mut request)?;
        if n == 0 {
            break; // client closed connection
        }

        // Forward request to provider
        child_stdin.write_all(request.as_bytes())?;
        child_stdin.flush()?;

        // Read response from provider
        let mut response = String::new();
        let rn = child_stdout.read_line(&mut response)?;
        if rn == 0 {
            eprintln!("provider process closed stdout unexpectedly");
            break;
        }

        // Send response back to client
        socket_writer.write_all(response.as_bytes())?;
    }
    Ok(())
}

fn handle_provider_stream(
    mut socket_reader: impl BufRead,
    mut socket_writer: impl Write,
    child_stdin: &mut impl Write,
    child_stdout: &mut impl BufRead,
) -> std::io::Result<()> {
    loop {
        let mut request = String::new();
        let n = socket_reader.read_line(&mut request)?;
        if n == 0 {
            break;
        }

        child_stdin.write_all(request.as_bytes())?;
        child_stdin.flush()?;

        let mut response = String::new();
        let rn = child_stdout.read_line(&mut response)?;
        if rn == 0 {
            eprintln!("provider process closed stdout unexpectedly");
            break;
        }

        socket_writer.write_all(response.as_bytes())?;
    }
    Ok(())
}

/// Provider mode: spawn ONE persistent provider process, forward all vsock
/// connections through it using line-delimited JSON (one request → one response).
fn run_provider_mode(listen_port: u32, provider_cmd: &str, provider_args: &[String]) -> std::io::Result<()> {
    let listener_fd = create_vsock_listener(listen_port)?;

    let mut child = Command::new(provider_cmd)
        .args(provider_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    let mut child_stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("provider child stdin unavailable"))?;
    let mut child_stdout = BufReader::new(
        child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("provider child stdout unavailable"))?,
    );

    println!(
        "carrier-bridge(provider): listening on vsock any:{} -> {} {}",
        listen_port,
        provider_cmd,
        provider_args.join(" ")
    );

    loop {
        // SAFETY: accept on valid listener fd.
        let client_fd = unsafe { libc::accept(listener_fd, std::ptr::null_mut(), std::ptr::null_mut()) };
        if client_fd < 0 {
            eprintln!(
                "provider accept error on port {}: {}",
                listen_port,
                std::io::Error::last_os_error()
            );
            continue;
        }
        if let Err(e) = handle_provider_session(client_fd, &mut child_stdin, &mut child_stdout) {
            eprintln!("provider session error: {}", e);
        }
    }
}

/// Provider mode over TCP: same JSON protocol as the vsock provider bridge,
/// but exposed on the guest-network compatibility path instead of AF_VSOCK.
fn run_provider_tcp_mode(
    listen_port: u16,
    provider_cmd: &str,
    provider_args: &[String],
) -> std::io::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", listen_port))?;

    let mut child = Command::new(provider_cmd)
        .args(provider_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    let mut child_stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("provider child stdin unavailable"))?;
    let mut child_stdout = BufReader::new(
        child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("provider child stdout unavailable"))?,
    );

    println!(
        "carrier-bridge(provider): listening on tcp 0.0.0.0:{} -> {} {}",
        listen_port,
        provider_cmd,
        provider_args.join(" ")
    );

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let reader = BufReader::new(stream.try_clone()?);
                if let Err(e) = handle_provider_stream(
                    reader,
                    stream,
                    &mut child_stdin,
                    &mut child_stdout,
                ) {
                    eprintln!("provider tcp session error: {}", e);
                }
            }
            Err(e) => {
                eprintln!("provider tcp accept error on port {}: {}", listen_port, e);
            }
        }
    }

    Ok(())
}

/// Proxy TCP connections to a serial device (e.g., /dev/ttyS1).
/// Used as the Carrier bridge when vsock is not available.
fn proxy_tcp_to_serial(tcp_stream: TcpStream, serial_path: &str) {
    let serial = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(serial_path)
    {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Failed to open serial device {}: {}", serial_path, e);
            return;
        }
    };

    let mut tcp_read = tcp_stream.try_clone().unwrap();
    let mut tcp_write = tcp_stream;
    let mut serial_read = serial.try_clone().unwrap();
    let mut serial_write = serial;

    let handle1 = thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match tcp_read.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if serial_write.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let handle2 = thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match serial_read.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tcp_write.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let _ = handle1.join();
    let _ = handle2.join();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Serial bridge mode: vsock-proxy serial <tcp-port> <serial-device>
    // Example: vsock-proxy serial 3000 /dev/ttyS1
    if args.len() >= 2 && args[1] == "serial" {
        if args.len() < 4 {
            eprintln!("Usage: {} serial <tcp-port> <serial-device>", args[0]);
            std::process::exit(1);
        }
        let tcp_port: u16 = args[2].parse().expect("Invalid TCP port");
        let serial_path = &args[3];

        let listener = TcpListener::bind(format!("127.0.0.1:{}", tcp_port))
            .expect("Failed to bind TCP listener");

        println!("carrier-bridge(serial): 127.0.0.1:{} -> {}", tcp_port, serial_path);

        for stream in listener.incoming() {
            match stream {
                Ok(tcp_stream) => {
                    let path = serial_path.to_string();
                    thread::spawn(move || {
                        proxy_tcp_to_serial(tcp_stream, &path);
                    });
                }
                Err(e) => eprintln!("Accept error: {}", e),
            }
        }
        return;
    }

    if args.len() >= 2 && args[1] == "provider-tcp" {
        if args.len() < 4 {
            eprintln!(
                "Usage: {} provider-tcp <listen-tcp-port> <provider-cmd> [provider-args...]",
                args[0]
            );
            std::process::exit(1);
        }

        let listen_port: u16 = args[2].parse().expect("Invalid listen TCP port");
        let provider_cmd = &args[3];
        let provider_args: Vec<String> = if args.len() > 4 {
            args[4..].to_vec()
        } else {
            vec![]
        };

        if let Err(e) = run_provider_tcp_mode(listen_port, provider_cmd, &provider_args) {
            eprintln!("provider-tcp mode failed: {}", e);
            std::process::exit(1);
        }
        return;
    }

    if args.len() >= 2 && args[1] == "provider" {
        if args.len() < 4 {
            eprintln!(
                "Usage: {} provider <listen-vsock-port> <provider-cmd> [provider-args...]",
                args[0]
            );
            std::process::exit(1);
        }

        let listen_port: u32 = args[2].parse().expect("Invalid listen vsock port");
        let provider_cmd = &args[3];
        let provider_args: Vec<String> = if args.len() > 4 {
            args[4..].to_vec()
        } else {
            Vec::new()
        };

        if let Err(e) = run_provider_mode(listen_port, provider_cmd, &provider_args) {
            eprintln!("provider mode failed: {}", e);
            std::process::exit(1);
        }
        return;
    }

    if args.len() != 4 {
        eprintln!("Usage: {} <tcp-port> <vsock-cid> <vsock-port>", args[0]);
        eprintln!("Example: {} 3000 2 3000", args[0]);
        eprintln!("Or:      {} provider <port> <provider-cmd> [args...]", args[0]);
        std::process::exit(1);
    }

    let tcp_port: u16 = args[1].parse().expect("Invalid TCP port");
    let vsock_cid: u32 = args[2].parse().expect("Invalid vsock CID");
    let vsock_port: u32 = args[3].parse().expect("Invalid vsock port");

    let listener = TcpListener::bind(format!("127.0.0.1:{}", tcp_port))
        .expect("Failed to bind TCP listener");

    println!("vsock-proxy: 127.0.0.1:{} -> vsock {}:{}", tcp_port, vsock_cid, vsock_port);

    for stream in listener.incoming() {
        match stream {
            Ok(tcp_stream) => {
                let cid = vsock_cid;
                let port = vsock_port;
                thread::spawn(move || {
                    proxy_connection(tcp_stream, cid, port);
                });
            }
            Err(e) => {
                eprintln!("Accept error: {}", e);
            }
        }
    }
}
