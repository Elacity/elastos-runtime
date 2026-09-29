//! Bounded file transfer over the existing Carrier u64-length + bytes wire.

use super::{CarrierClient, TrustedSource};
use anyhow::{ensure, Context, Result};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};

const CHUNK_SIZE: usize = 64 * 1024;
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const ERROR_LIMIT: usize = 16 * 1024;

struct IncomingFile(iroh::endpoint::RecvStream);

impl Drop for IncomingFile {
    fn drop(&mut self) {
        // Also runs when a caller drops the download future while it is waiting.
        let _ = self.0.stop(0u32.into());
    }
}

pub(super) async fn send_file(send: &mut iroh::endpoint::SendStream, path: &Path) -> Result<u64> {
    let mut file = tokio::fs::File::open(path).await?;
    let metadata = file.metadata().await?;
    ensure!(
        metadata.is_file(),
        "Carrier artifact must be a regular file"
    );
    let len = metadata.len();
    send_body(&mut file, send, len, IDLE_TIMEOUT).await?;
    send.finish()?;
    tokio::time::timeout(IDLE_TIMEOUT, send.stopped())
        .await
        .context("Carrier file receiver acknowledgement deadline")??;
    Ok(len)
}

async fn send_body<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
    len: u64,
    idle: Duration,
) -> Result<()> {
    tokio::time::timeout(idle, writer.write_all(&len.to_be_bytes()))
        .await
        .context("Carrier file header write deadline")??;
    let mut remaining = len;
    let mut chunk = [0u8; CHUNK_SIZE];
    while remaining > 0 {
        let wanted = remaining.min(CHUNK_SIZE as u64) as usize;
        let count = tokio::time::timeout(idle, reader.read(&mut chunk[..wanted]))
            .await
            .context("Carrier file read deadline")??;
        ensure!(
            count > 0,
            "Carrier source file was truncated during transfer"
        );
        tokio::time::timeout(idle, writer.write_all(&chunk[..count]))
            .await
            .context("Carrier file write deadline")??;
        remaining -= count as u64;
    }
    Ok(())
}

impl CarrierClient {
    pub(crate) async fn fetch_file_to<W: AsyncWrite + Unpin>(
        &self,
        path: &str,
        writer: &mut W,
        expected_size: u64,
        progress: &mut (impl FnMut(u64, u64) -> Result<()> + Send),
    ) -> Result<()> {
        let (mut send, recv) = self.conn.open_bi().await?;
        let mut recv = IncomingFile(recv);
        let mut request = serde_json::to_vec(&serde_json::json!({"op":"file","path":path}))?;
        request.push(b'\n');
        tokio::time::timeout(IDLE_TIMEOUT, send.write_all(&request))
            .await
            .context("Carrier file request deadline")??;
        send.finish()?;
        let result = receive_body(&mut recv.0, writer, expected_size, IDLE_TIMEOUT, progress).await;
        result.with_context(|| format!("trusted source file fetch for {path}"))
    }
}

async fn receive_body<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
    expected_size: u64,
    idle: Duration,
    progress: &mut (impl FnMut(u64, u64) -> Result<()> + Send),
) -> Result<()> {
    let mut prefix = [0u8; 8];
    tokio::time::timeout(idle, reader.read_exact(&mut prefix))
        .await
        .context("Carrier file header deadline")??;
    // Existing senders return a short JSON line for missing/invalid paths.
    if prefix[0] == b'{' {
        let mut error = prefix.to_vec();
        tokio::time::timeout(
            idle,
            reader
                .take((ERROR_LIMIT - prefix.len()) as u64)
                .read_to_end(&mut error),
        )
        .await
        .context("Carrier file error reply deadline")??;
        let value: serde_json::Value = serde_json::from_slice(&error)
            .context("Carrier file returned an invalid error reply")?;
        anyhow::bail!(
            "Carrier file error: {}",
            value["error"].as_str().unwrap_or("unknown error")
        );
    }
    let declared = u64::from_be_bytes(prefix);
    ensure!(
        declared == expected_size,
        "Carrier file size mismatch: expected {expected_size}, declared {declared}"
    );
    progress(0, declared)?;
    let mut received = 0u64;
    let mut chunk = [0u8; CHUNK_SIZE];
    while received < declared {
        let wanted = (declared - received).min(CHUNK_SIZE as u64) as usize;
        let count = tokio::time::timeout(idle, reader.read(&mut chunk[..wanted]))
            .await
            .context("Carrier file download idle deadline")??;
        ensure!(
            count > 0,
            "Carrier file download is incomplete ({received}/{declared} bytes)"
        );
        tokio::time::timeout(idle, writer.write_all(&chunk[..count]))
            .await
            .context("Carrier file destination write deadline")??;
        received += count as u64;
        progress(received, declared)?;
    }
    let mut extra = [0u8; 1];
    let count = tokio::time::timeout(idle, reader.read(&mut extra))
        .await
        .context("Carrier file completion deadline")??;
    ensure!(count == 0, "Carrier file exceeds its declared size");
    tokio::time::timeout(idle, writer.flush())
        .await
        .context("Carrier file destination flush deadline")??;
    Ok(())
}

/// Retry the existing ticket, relay and direct routes into the same temporary
/// file. Each attempt starts at byte zero and stays bound to one trusted peer.
pub(crate) async fn fetch_file_from_trusted_source_to(
    source: &TrustedSource,
    path: &str,
    file: &mut tokio::fs::File,
    expected_size: u64,
    progress: &mut (impl FnMut(u64, u64) -> Result<()> + Send),
) -> Result<()> {
    let peer = super::source_transport_endpoint_id(source)?
        .context("trusted source has no usable Carrier node id")?;
    let mut candidates = super::decode_ticket_endpoints(&source.connect_ticket);
    candidates.extend(super::relay_only_ticket_endpoints(source));
    ensure!(
        candidates
            .iter()
            .all(|address| address.id.to_string() == peer),
        "trusted source file routes do not match the configured Carrier identity"
    );
    let mut errors = Vec::new();
    for candidate in candidates
        .into_iter()
        .map(Some)
        .chain(std::iter::once(None))
    {
        file.set_len(0).await?;
        file.rewind().await?;
        progress(0, expected_size)?;
        let connected = match candidate {
            Some(address) => CarrierClient::connect_endpoint_addr(address, 15).await,
            None => CarrierClient::connect(&peer, &super::source_carrier_addrs(source), 15).await,
        };
        let client = match connected {
            Ok(client) => client,
            Err(error) => {
                errors.push(format!("connect: {error:#}"));
                continue;
            }
        };
        match client
            .fetch_file_to(path, file, expected_size, progress)
            .await
        {
            Ok(()) => return Ok(()),
            Err(error) => errors.push(format!("fetch: {error:#}")),
        }
    }
    anyhow::bail!(
        "trusted source Carrier file download failed: {}",
        errors.join(" | ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::task::{Context as TaskContext, Poll};

    #[derive(Default)]
    struct CountingWriter {
        bytes: u64,
        largest: usize,
    }

    impl AsyncWrite for CountingWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _: &mut TaskContext<'_>,
            bytes: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            assert!(
                bytes.len() <= CHUNK_SIZE,
                "transfer tried to buffer a whole artifact"
            );
            assert!(bytes.iter().all(|byte| *byte == 0x5a));
            self.bytes += bytes.len() as u64;
            self.largest = self.largest.max(bytes.len());
            Poll::Ready(Ok(bytes.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: Pin<&mut Self>,
            _: &mut TaskContext<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn carrier_file_streams_beyond_old_limit_with_bounded_chunks_and_progress() {
        // Synthetic stream to a counting sink: no image, large Vec or disk file.
        let size = 200 * 1024 * 1024 + CHUNK_SIZE as u64 + 17;
        let (mut send, mut recv) = tokio::io::duplex(CHUNK_SIZE);
        let sender = async {
            let mut source = tokio::io::repeat(0x5a).take(size);
            send_body(&mut source, &mut send, size, IDLE_TIMEOUT)
                .await
                .unwrap();
            send.shutdown().await.unwrap();
        };
        let receiver = async {
            let mut writer = CountingWriter::default();
            let mut last = 0;
            receive_body(
                &mut recv,
                &mut writer,
                size,
                IDLE_TIMEOUT,
                &mut |received, total| {
                    assert_eq!(total, size);
                    assert!(received >= last && received - last <= CHUNK_SIZE as u64);
                    last = received;
                    Ok(())
                },
            )
            .await
            .unwrap();
            assert_eq!(last, size);
            assert_eq!(writer.bytes, size);
            assert!(writer.largest > 0 && writer.largest <= CHUNK_SIZE);
        };
        tokio::join!(sender, receiver);
    }

    #[tokio::test]
    async fn carrier_file_rejects_wrong_length_truncation_extra_bytes_and_json_errors() {
        for (declared, content, expected, error) in [
            (u64::MAX, vec![], 1, "size mismatch"),
            (2, vec![0x5a], 2, "incomplete"),
            (1, vec![0x5a, 0x5a], 1, "exceeds"),
        ] {
            let mut wire = declared.to_be_bytes().to_vec();
            wire.extend(content);
            let mut reader = std::io::Cursor::new(wire);
            let failure = receive_body(
                &mut reader,
                &mut CountingWriter::default(),
                expected,
                IDLE_TIMEOUT,
                &mut |_, _| Ok(()),
            )
            .await
            .unwrap_err();
            assert!(failure.to_string().contains(error), "{failure:#}");
        }
        let mut reader = &b"{\"ok\":false,\"error\":\"not found\"}\n"[..];
        let failure = receive_body(
            &mut reader,
            &mut tokio::io::sink(),
            1,
            IDLE_TIMEOUT,
            &mut |_, _| Ok(()),
        )
        .await
        .unwrap_err();
        assert!(failure.to_string().contains("not found"));
    }

    #[tokio::test(start_paused = true)]
    async fn carrier_file_idle_deadline_and_cancellation_stop_partial_transfer() {
        let (mut send, mut recv) = tokio::io::duplex(64);
        send.write_all(&100u64.to_be_bytes()).await.unwrap();
        let failure = receive_body(
            &mut recv,
            &mut tokio::io::sink(),
            100,
            Duration::from_millis(20),
            &mut |_, _| Ok(()),
        )
        .await
        .unwrap_err();
        assert!(failure.to_string().contains("idle deadline"));
        drop(send);
        let size = 3 * CHUNK_SIZE;
        let mut wire = (size as u64).to_be_bytes().to_vec();
        wire.extend(vec![0x5a; size]);
        let mut reader = std::io::Cursor::new(wire);
        let failure = receive_body(
            &mut reader,
            &mut tokio::io::sink(),
            size as u64,
            IDLE_TIMEOUT,
            &mut |received, _| {
                ensure!(received == 0, "cancelled by receiver");
                Ok(())
            },
        )
        .await
        .unwrap_err();
        assert!(failure.to_string().contains("cancelled"));
        assert_eq!(reader.position(), 8 + CHUNK_SIZE as u64);
    }

    #[tokio::test(start_paused = true)]
    async fn carrier_file_progress_keeps_a_transfer_alive_beyond_thirty_seconds() {
        let (mut send, mut recv) = tokio::io::duplex(64);
        let start = tokio::time::Instant::now();
        let sender = async {
            send.write_all(&3u64.to_be_bytes()).await.unwrap();
            for _ in 0..3 {
                tokio::time::sleep(Duration::from_secs(20)).await;
                send.write_all(&[0x5a]).await.unwrap();
            }
            send.shutdown().await.unwrap();
        };
        let receiver = async {
            receive_body(
                &mut recv,
                &mut CountingWriter::default(),
                3,
                IDLE_TIMEOUT,
                &mut |_, _| Ok(()),
            )
            .await
            .unwrap();
        };
        tokio::join!(sender, receiver);
        assert_eq!(start.elapsed(), Duration::from_secs(60));
    }

    #[tokio::test]
    async fn carrier_file_source_truncation_and_destination_failure_stop_copying() {
        let error = send_body(&mut &b"x"[..], &mut tokio::io::sink(), 2, IDLE_TIMEOUT)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("truncated"));
        // A closed destination stands in for a failed disk write.
        let (mut writer, destination) = tokio::io::duplex(1);
        drop(destination);
        let mut wire = 2u64.to_be_bytes().to_vec();
        wire.extend([0x5a; 2]);
        assert!(receive_body(
            &mut &wire[..],
            &mut writer,
            2,
            IDLE_TIMEOUT,
            &mut |_, _| Ok(())
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn carrier_file_real_sender_keeps_legacy_wire_and_exact_peer_for_disk_download() {
        use iroh::Watcher;
        let dir = tempfile::tempdir().unwrap();
        let artifacts = super::super::publisher_artifacts_path(dir.path());
        std::fs::create_dir_all(&artifacts).unwrap();
        let bytes = vec![0x5a; 2 * CHUNK_SIZE + 9];
        std::fs::write(artifacts.join("test.bin"), &bytes).unwrap();
        let (key, did) = elastos_identity::derive_did(&[139; 32]);
        let node = super::super::start_isolated_carrier_node_with_registry(
            &key,
            &did,
            dir.path().to_owned(),
            None,
        )
        .await
        .unwrap();
        let address = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let address = node.endpoint.watch_addr().get();
                if !address.addrs.is_empty() {
                    break address;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .bind()
            .await
            .unwrap();
        let client = CarrierClient::connect_known_endpoint(&endpoint, address, 3)
            .await
            .unwrap();
        assert_eq!(client.conn.remote_id(), node.endpoint.id());
        assert_eq!(client.fetch_file("test.bin").await.unwrap(), bytes);
        let mut file = tokio::fs::File::from_std(tempfile::tempfile_in(dir.path()).unwrap());
        client
            .fetch_file_to(
                "test.bin",
                &mut file,
                bytes.len() as u64,
                &mut |_, _| Ok(()),
            )
            .await
            .unwrap();
        file.rewind().await.unwrap();
        let mut actual = Vec::new();
        file.read_to_end(&mut actual).await.unwrap();
        assert_eq!(actual, bytes);
        file.set_len(0).await.unwrap();
        file.rewind().await.unwrap();
        let cancelled = client
            .fetch_file_to(
                "test.bin",
                &mut file,
                bytes.len() as u64,
                &mut |received, _| {
                    ensure!(received == 0, "cancelled by receiver");
                    Ok(())
                },
            )
            .await
            .unwrap_err();
        assert!(format!("{cancelled:#}").contains("cancelled"));
        // Stopping one file stream leaves the authenticated connection usable.
        assert_eq!(client.fetch_file("test.bin").await.unwrap(), bytes);
        let error = client
            .fetch_file_to("absent", &mut file, 1, &mut |_, _| Ok(()))
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("not found"));
        let source: TrustedSource = serde_json::from_value(serde_json::json!({
            "name":"test", "publisher_node_id":iroh::SecretKey::from_bytes(&[140; 32]).public().to_string(),
            "connect_ticket":super::super::carrier_connect_ticket(&node.endpoint)
        })).unwrap();
        let error = fetch_file_from_trusted_source_to(
            &source,
            "test.bin",
            &mut file,
            bytes.len() as u64,
            &mut |_, _| Ok(()),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("identity"));
        endpoint.close().await;
        super::super::CarrierRuntimeService::new(node)
            .shutdown()
            .await
            .unwrap();
    }
}
