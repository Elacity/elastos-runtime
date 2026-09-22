//! The macOS Runtime owns the only socket reachable by the confined model provider.
//! It forwards only local llama routes to an engine descended from that provider.

use std::io;
use std::mem::{size_of, MaybeUninit};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;
use tokio::task::{JoinHandle, JoinSet};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_REQUEST_BODY_BYTES: usize = 4 * 1024 * 1024;
// Offer policy admits up to 64 concurrent runs. Leave room for model health
// checks and close-out requests while bounding idle or hostile connections.
const MAX_CONNECTIONS: usize = 68;
const HEADER_TIMEOUT: Duration = Duration::from_secs(2);
// Model-provider admits runs up to one hour; allow a small close-out margin.
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(3605);

pub(super) fn start(
    broker_path: &str,
    engine_path: PathBuf,
    provider_pid: Arc<AtomicU32>,
    provider_birth: Arc<AtomicU64>,
) -> io::Result<JoinHandle<()>> {
    let listener = UnixListener::bind(broker_path)?;
    Ok(tokio::spawn(async move {
        let mut connections = JoinSet::new();
        let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        let mut monitor = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let Ok((client, _)) = accepted else { break };
                    let Ok(permit) = permits.clone().try_acquire_owned() else { continue };
                    let engine_path = engine_path.clone();
                    let provider_pid = provider_pid.clone();
                    let provider_birth = provider_birth.clone();
                    connections.spawn(async move {
                        let _permit = permit;
                        let _ = tokio::time::timeout(
                            CONNECTION_TIMEOUT,
                            forward(client, &engine_path, &provider_pid, &provider_birth),
                        ).await;
                    });
                }
                Some(_) = connections.join_next(), if !connections.is_empty() => {}
                _ = monitor.tick() => {
                    let pid = provider_pid.load(Ordering::Acquire);
                    if pid != 0 && process_birth(pid) != Some(provider_birth.load(Ordering::Acquire)) {
                        break;
                    }
                }
            }
        }
    }))
}

async fn forward(
    mut client: UnixStream,
    engine_path: &PathBuf,
    provider_pid: &AtomicU32,
    provider_birth: &AtomicU64,
) -> io::Result<()> {
    let expected = provider_pid.load(Ordering::Acquire);
    if expected == 0
        || peer_pid(&client)? != expected
        || process_birth(expected) != Some(provider_birth.load(Ordering::Acquire))
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "wrong model client",
        ));
    }
    let mut request = Vec::with_capacity(1024);
    tokio::time::timeout(HEADER_TIMEOUT, async {
        let mut byte = [0u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            if request.len() >= MAX_HEADER_BYTES || client.read_exact(&mut byte).await.is_err() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid model request",
                ));
            }
            request.push(byte[0]);
        }
        Ok(())
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "model request timeout"))??;
    let line_end = request
        .windows(2)
        .position(|window| window == b"\r\n")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing model route"))?;
    let route = &request[..line_end];
    if !matches!(
        route,
        b"GET /v1/models HTTP/1.1" | b"POST /v1/chat/completions HTTP/1.1"
    ) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "model route denied",
        ));
    }
    let body_length = request_body_length(&request, route)?;
    let mut engine = UnixStream::connect(engine_path).await?;
    if !is_descendant(peer_pid(&engine)?, expected) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "wrong model engine",
        ));
    }
    // Make the engine close after this response so the stream has an exact
    // boundary, while keeping the request side open for SSE generation.
    engine.write_all(&request[..request.len() - 2]).await?;
    engine.write_all(b"Connection: close\r\n\r\n").await?;
    let mut remaining = body_length;
    let mut body = [0u8; 8192];
    while remaining > 0 {
        let chunk_size = remaining.min(body.len());
        let count = client.read(&mut body[..chunk_size]).await?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "model body truncated",
            ));
        }
        engine.write_all(&body[..count]).await?;
        remaining -= count;
    }
    // A pipelined second request remains on the provider connection.
    tokio::io::copy(&mut engine, &mut client).await?;
    Ok(())
}

fn request_body_length(headers: &[u8], route: &[u8]) -> io::Result<usize> {
    let header = std::str::from_utf8(headers)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid model headers"))?;
    let mut length = None;
    for line in header.split("\r\n").skip(1).filter(|line| !line.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid model header"))?;
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "chunked model request denied",
            ));
        }
        if name.eq_ignore_ascii_case("connection") {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "model connection header denied",
            ));
        }
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "duplicate model length",
                ));
            }
            length =
                Some(value.trim().parse::<usize>().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid model length")
                })?);
        }
    }
    let length = length.unwrap_or(0);
    if length > MAX_REQUEST_BODY_BYTES
        || (route == b"GET /v1/models HTTP/1.1" && length != 0)
        || (route == b"POST /v1/chat/completions HTTP/1.1" && length == 0)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "model body length denied",
        ));
    }
    Ok(length)
}

fn peer_pid(stream: &UnixStream) -> io::Result<u32> {
    let mut pid: libc::pid_t = 0;
    let mut size = size_of::<libc::pid_t>() as libc::socklen_t;
    let status = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut size,
        )
    };
    if status != 0 || size as usize != size_of::<libc::pid_t>() || pid <= 1 {
        return Err(io::Error::last_os_error());
    }
    Ok(pid as u32)
}

fn is_descendant(mut pid: u32, provider_pid: u32) -> bool {
    // llama-server is the guard's child. A small bound avoids following an
    // unrelated or corrupted process chain indefinitely.
    for _ in 0..4 {
        if pid == provider_pid || pid <= 1 {
            return false;
        }
        let Some(info) = process_info(pid) else {
            return false;
        };
        if info.pbi_ppid == provider_pid {
            return true;
        }
        pid = info.pbi_ppid;
    }
    false
}

pub(super) fn process_birth(pid: u32) -> Option<u64> {
    let info = process_info(pid)?;
    Some((info.pbi_start_tvsec << 20) | info.pbi_start_tvusec)
}

fn process_info(pid: u32) -> Option<libc::proc_bsdinfo> {
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = size_of::<libc::proc_bsdinfo>();
    let result = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size as libc::c_int,
        )
    };
    if result != size as libc::c_int {
        return None;
    }
    let info = unsafe { info.assume_init() };
    (info.pbi_pid == pid && info.pbi_status != libc::SZOMB).then_some(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    #[tokio::test]
    async fn only_owned_engine_and_exact_route_are_forwarded() {
        let dir = tempfile::tempdir().unwrap();
        let broker = dir.path().join("broker.sock");
        let engine = dir.path().join("engine.sock");
        let mut child = tokio::process::Command::new("/usr/bin/python3")
            .arg("-c")
            .arg(r"import socket,sys; s=socket.socket(socket.AF_UNIX); s.bind(sys.argv[1]); s.listen(1); c,_=s.accept(); c.recv(4096); c.sendall(b'HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok'); c.close()")
            .arg(&engine)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while !engine.exists() {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let task = start(
            broker.to_str().unwrap(),
            engine,
            Arc::new(AtomicU32::new(std::process::id())),
            Arc::new(AtomicU64::new(process_birth(std::process::id()).unwrap())),
        )
        .unwrap();
        let mut denied = UnixStream::connect(&broker).await.unwrap();
        denied
            .write_all(b"POST /elsewhere HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        denied.shutdown().await.unwrap();
        let mut refused = Vec::new();
        denied.read_to_end(&mut refused).await.unwrap();
        assert!(refused.is_empty());

        let mut allowed = UnixStream::connect(&broker).await.unwrap();
        allowed
            .write_all(b"GET /v1/models HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        allowed.shutdown().await.unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), allowed.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(response.ends_with(b"\r\n\r\nok"));
        assert!(child.wait().await.unwrap().success());
        task.abort();
    }

    #[test]
    fn unrelated_sibling_process_is_not_an_engine() {
        let mut provider = std::process::Command::new("/bin/sleep")
            .arg("2")
            .spawn()
            .unwrap();
        let mut sibling = std::process::Command::new("/bin/sleep")
            .arg("2")
            .spawn()
            .unwrap();
        assert!(!is_descendant(sibling.id(), provider.id()));
        assert!(!is_descendant(std::process::id(), provider.id()));
        let _ = provider.kill();
        let _ = sibling.kill();
        let _ = provider.wait();
        let _ = sibling.wait();
    }

    #[test]
    fn one_request_body_is_bounded_and_chunked_or_duplicate_lengths_are_refused() {
        let route = b"POST /v1/chat/completions HTTP/1.1";
        assert_eq!(
            request_body_length(
                b"POST /v1/chat/completions HTTP/1.1\r\nContent-Length: 2\r\n\r\n",
                route
            )
            .unwrap(),
            2
        );
        for header in [
            b"POST /v1/chat/completions HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n".as_slice(),
            b"POST /v1/chat/completions HTTP/1.1\r\nContent-Length: 2\r\nContent-Length: 3\r\n\r\n",
            b"POST /v1/chat/completions HTTP/1.1\r\nContent-Length: 4194305\r\n\r\n",
        ] {
            assert!(request_body_length(header, route).is_err());
        }
    }

    #[tokio::test]
    async fn excess_provider_connections_are_closed_without_new_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let broker = dir.path().join("broker.sock");
        let task = start(
            broker.to_str().unwrap(),
            dir.path().join("engine.sock"),
            Arc::new(AtomicU32::new(std::process::id())),
            Arc::new(AtomicU64::new(process_birth(std::process::id()).unwrap())),
        )
        .unwrap();
        let mut held = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            held.push(UnixStream::connect(&broker).await.unwrap());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut excess = UnixStream::connect(&broker).await.unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), excess.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(response.is_empty());
        task.abort();
    }
}
