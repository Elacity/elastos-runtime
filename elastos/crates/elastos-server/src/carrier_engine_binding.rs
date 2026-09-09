//! Private Runtime-to-Runtime Engine ownership, carried on authenticated Carrier.
//! Capsule requests continue to use the existing Browser Engine protocol.

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(crate) const EXECUTION_SCOPE: &str = "principal_scoped_browser_engine_execution_grant";
pub(crate) const PROBE_SCOPE: &str = "principal_scoped_browser_engine_probe_grant";
pub(crate) const EXECUTION_OPERATIONS: &[&str] = &[
    "status",
    "readiness",
    "prepare_launch",
    "launch",
    "attach_stream",
    "page_status",
    "diagnostics",
    "inspect",
    "input",
    "webrtc_signal",
    "close_page",
];

pub(crate) fn operations(scope: Option<&str>) -> Value {
    if scope == Some(EXECUTION_SCOPE) {
        json!(EXECUTION_OPERATIONS)
    } else {
        json!(["status", "readiness"])
    }
}

pub(crate) fn allows(scope: Option<&str>, operation: &str) -> bool {
    matches!(operation, "status" | "readiness")
        || scope == Some(EXECUTION_SCOPE) && EXECUTION_OPERATIONS.contains(&operation)
}

/// The consumer's random lifecycle generation stays intact in native receipts.
/// The serving Runtime additionally binds it to the authenticated endpoint and
/// issued grant; the generation alone never confers access.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteEngineOwner {
    pub requester_endpoint: String,
    pub requester_principal_id: String,
    pub grant_id: String,
    pub grant_revision: u64,
    pub generation: String,
    pub page_id: String,
    pub vm_id: String,
    pub profile_key: String,
    pub adapter_id: String,
}

impl RemoteEngineOwner {
    pub(crate) fn from_launch(
        grant: &super::browser_engine::BrowserEngineGrant,
        request: &Value,
    ) -> Result<Self> {
        ensure!(
            grant.execution_allowed,
            "Engine grant permits observations only"
        );
        let generation = request["lifecycle_generation"]
            .as_str()
            .context("Engine lifecycle generation required")?;
        ensure!(
            generation.strip_prefix("sha256:").is_some_and(hex64),
            "Engine lifecycle generation invalid"
        );
        let page_id = format!(
            "page:vz-{}",
            hex::encode(Sha256::digest(format!("{generation}\npage")))
        );
        let vm_id = format!(
            "browser-vm-{}",
            hex::encode(Sha256::digest(format!("{generation}\nvm")))
        );
        ensure!(
            request["page_id"] == page_id && request["vm_id"] == vm_id,
            "Engine page does not bind the lifecycle generation"
        );
        let profile_key = request["profile_key"]
            .as_str()
            .context("Engine profile identity required")?;
        ensure!(
            profile_key.strip_prefix("profile-").is_some_and(hex64),
            "Engine profile identity invalid"
        );
        ensure!(
            elastos_common::browser_profile_key_from_value(&grant.requester_principal_id)
                .as_deref()
                == Some(profile_key),
            "Engine profile identity does not belong to the approved requester"
        );
        let adapter_id = request["adapter_id"]
            .as_str()
            .context("Engine adapter required")?;
        ensure!(
            !adapter_id.is_empty()
                && adapter_id.len() <= 128
                && adapter_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':')),
            "Engine adapter invalid"
        );
        Ok(Self {
            requester_endpoint: grant.requester_endpoint.to_string(),
            requester_principal_id: grant.requester_principal_id.clone(),
            grant_id: grant.grant_id.clone(),
            grant_revision: grant.revision,
            generation: generation.into(),
            page_id,
            vm_id,
            profile_key: profile_key.into(),
            adapter_id: adapter_id.into(),
        })
    }

    pub(crate) fn matches_request(&self, source: &iroh::PublicKey, request: &Value) -> bool {
        self.requester_endpoint == source.to_string()
            && request["principal_id"] == self.requester_principal_id
            && request["grant_id"] == self.grant_id
            && request["lifecycle_generation"] == self.generation
            && request["page_id"] == self.page_id
    }

    /// Provider storage is scoped to both identities. A peer cannot name a host
    /// directory or borrow another peer's profile by supplying its principal.
    pub(crate) fn storage_principal(&self) -> String {
        format!(
            "remote-engine-{}",
            hex::encode(Sha256::digest(format!(
                "{}\n{}",
                self.requester_endpoint, self.requester_principal_id
            )))
        )
    }
}

fn hex64(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// Only the consumer Runtime can install these callbacks, after it has obtained
// a Net/Exit stream. The peer never supplies a socket path to this map.
#[cfg(unix)]
mod egress {
    use super::*;
    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
        sync::{Arc, OnceLock},
    };
    use tokio::{
        io::AsyncWriteExt,
        sync::{watch, Mutex, Semaphore},
    };

    struct Binding {
        peer: iroh::PublicKey,
        generation: String,
        stream_id: String,
        path: PathBuf,
        cancel: watch::Sender<bool>,
        slots: Arc<Semaphore>,
    }
    static BINDINGS: OnceLock<Mutex<BTreeMap<(PathBuf, String), Binding>>> = OnceLock::new();

    #[cfg(test)]
    pub(super) async fn slots_for_test(data_dir: &Path, page: &str) -> Arc<Semaphore> {
        BINDINGS.get_or_init(Default::default).lock().await[&(data_dir.to_owned(), page.to_owned())]
            .slots
            .clone()
    }

    pub(crate) async fn bind(
        data_dir: &Path,
        peer: iroh::PublicKey,
        page: &str,
        generation: &str,
        stream_id: &str,
        path: &Path,
    ) -> Result<()> {
        use std::os::unix::fs::FileTypeExt;
        ensure!(
            std::fs::symlink_metadata(path)?.file_type().is_socket(),
            "Engine egress socket unavailable"
        );
        let mut bindings = BINDINGS.get_or_init(Default::default).lock().await;
        let key = (data_dir.to_owned(), page.to_owned());
        if let Some(existing) = bindings.get(&key) {
            ensure!(
                existing.peer == peer && existing.generation == generation,
                "Engine egress owner changed"
            );
            ensure!(
                existing.stream_id == stream_id
                    && existing.path == path
                    && !*existing.cancel.borrow(),
                "Engine egress is fixed for this launch generation"
            );
            return Ok(());
        }
        let (cancel, _) = watch::channel(false);
        bindings.insert(
            key,
            Binding {
                peer,
                generation: generation.into(),
                stream_id: stream_id.into(),
                path: path.into(),
                cancel,
                slots: Arc::new(Semaphore::new(128)),
            },
        );
        Ok(())
    }

    pub(crate) async fn remove(data_dir: &Path, page: &str, generation: &str) -> Result<()> {
        let mut bindings = BINDINGS.get_or_init(Default::default).lock().await;
        let key = (data_dir.to_owned(), page.to_owned());
        if let Some(binding) = bindings.get(&key) {
            ensure!(
                binding.generation == generation,
                "Engine egress cleanup generation changed"
            );
            binding.cancel.send_replace(true);
            bindings.remove(&key);
        }
        Ok(())
    }

    pub(crate) async fn serve(
        data_dir: &Path,
        peer: iroh::PublicKey,
        request: &Value,
        send: &mut iroh::endpoint::SendStream,
        mut recv: iroh::endpoint::RecvStream,
        buffered: Vec<u8>,
    ) -> Result<()> {
        let admitted = async {
            ensure!(
                serde_json::to_vec(request)?.len() <= 1024,
                "Engine egress request too large"
            );
            let page = request["page_id"]
                .as_str()
                .context("Engine egress page required")?;
            let bindings = BINDINGS.get_or_init(Default::default).lock().await;
            let binding = bindings
                .get(&(data_dir.to_owned(), page.into()))
                .context("Engine egress owner absent")?;
            ensure!(
                binding.peer == peer
                    && request["generation"] == binding.generation
                    && request["stream_id"] == binding.stream_id
                    && !*binding.cancel.borrow(),
                "Engine egress owner mismatch"
            );
            let slot = binding
                .slots
                .clone()
                .try_acquire_owned()
                .context("Engine egress capacity unavailable")?;
            Ok::<_, anyhow::Error>((binding.path.clone(), binding.cancel.subscribe(), slot))
        }
        .await;
        let (path, mut cancel, _slot) = match admitted {
            Ok(binding) => binding,
            Err(_) => {
                super::super::send_json(
                    send,
                    &json!({"ok":false,"error":"Runtime Engine egress is unavailable"}),
                )
                .await?;
                return Ok(());
            }
        };
        let mut socket = tokio::net::UnixStream::connect(path).await?;
        super::super::write_json_line(send, &json!({"ok":true})).await?;
        socket.write_all(&buffered).await?;
        let (mut input, mut output) = socket.into_split();
        tokio::select! {
            _ = cancel.changed() => {},
            result = async {
                tokio::try_join!(
                    async {tokio::io::copy(&mut recv, &mut output).await?; output.shutdown().await},
                    async {
                        tokio::io::copy(&mut input, &mut *send).await?;
                        // Exit EOF must reach the Engine while its request half stays open.
                        send.finish().ok();
                        Ok::<_, std::io::Error>(())
                    },
                )?;
                Ok::<_, anyhow::Error>(())
            } => {result?;}
        }
        send.finish().ok();
        Ok(())
    }

    pub(crate) async fn connect(
        endpoint: &iroh::Endpoint,
        owner: &RemoteEngineOwner,
        stream_id: &str,
    ) -> Result<super::super::BrowserCarrierStream> {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let client = super::super::CarrierClient::connect_resolved_peer(
                endpoint,
                owner.requester_endpoint.parse()?,
                5,
            )
            .await?;
            let (mut send, mut recv) = client.conn.open_bi().await?;
            super::super::write_json_line(
                &mut send,
                &json!({"op":"browser_engine_egress",
                "page_id":owner.page_id,"generation":owner.generation,"stream_id":stream_id}),
            )
            .await?;
            let mut bytes = Vec::new();
            loop {
                let mut byte = [0u8; 1];
                recv.read_exact(&mut byte).await?;
                ensure!(
                    bytes.len() < 1024,
                    "Engine egress acknowledgement too large"
                );
                if byte[0] == b'\n' {
                    break;
                }
                bytes.push(byte[0]);
            }
            ensure!(
                serde_json::from_slice::<Value>(&bytes)?["ok"] == true,
                "Engine egress was not admitted"
            );
            Ok(super::super::BrowserCarrierStream {
                send,
                recv,
                _client: client,
            })
        })
        .await
        .context("Engine egress deadline")?
    }
}
#[cfg(unix)]
pub(crate) use egress::{
    bind as bind_egress, connect as connect_egress, remove as remove_egress, serve as serve_egress,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    async fn wait_for_egress_slots(slots: &tokio::sync::Semaphore) -> Result<()> {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while slots.available_permits() != 128 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .context("Engine egress slot was not released")
    }

    #[test]
    fn legacy_approval_does_not_acquire_execution_on_upgrade() {
        for scope in [None, Some(PROBE_SCOPE), Some("unknown")] {
            assert!(allows(scope, "status"));
            assert!(!allows(scope, "launch"));
            assert!(!allows(scope, "prepare_launch"));
            assert_eq!(operations(scope), json!(["status", "readiness"]));
        }
        assert!(allows(Some(EXECUTION_SCOPE), "launch"));
        assert!(!allows(Some(EXECUTION_SCOPE), "shutdown"));
        assert!(!allows(Some(EXECUTION_SCOPE), "init"));
    }

    fn owner_fixture() -> (super::super::BrowserEngineGrant, Value) {
        let grant = super::super::BrowserEngineGrant {
            provider_principal_id: "provider".into(),
            requester_principal_id: "requester".into(),
            requester_endpoint: iroh::SecretKey::from_bytes(&[22; 32]).public(),
            grant_id: "grant:engine".into(),
            revision: 100,
            expires_at: 3700,
            execution_allowed: true,
        };
        let generation = format!("sha256:{}", "a".repeat(64));
        let request = json!({"op":"prepare_launch","principal_id":grant.requester_principal_id,"grant_id":grant.grant_id,
            "lifecycle_generation":generation,
            "page_id":format!("page:vz-{}", hex::encode(Sha256::digest(format!("{generation}\npage")))),
            "vm_id":format!("browser-vm-{}", hex::encode(Sha256::digest(format!("{generation}\nvm")))),
            "profile_key":elastos_common::browser_profile_key_from_value(&grant.requester_principal_id),"adapter_id":"macos-vz"});
        (grant, request)
    }

    #[test]
    fn exact_runtime_owner_binds_generation_page_profile_and_authenticated_peer() {
        let (grant, request) = owner_fixture();
        grant
            .validate(&grant.requester_endpoint, &request, 100)
            .unwrap();
        let owner = RemoteEngineOwner::from_launch(&grant, &request).unwrap();
        assert!(owner.matches_request(&grant.requester_endpoint, &request));
        for (key, value) in [
            ("page_id", "page:vz-foreign"),
            ("vm_id", "vm:foreign"),
            ("profile_key", "profile-foreign"),
            ("adapter_id", "/private/host"),
            ("lifecycle_generation", "generation:legacy"),
        ] {
            let mut invalid = request.clone();
            invalid[key] = json!(value);
            assert!(
                RemoteEngineOwner::from_launch(&grant, &invalid).is_err(),
                "{key}"
            );
        }
        for key in [
            "principal_id",
            "grant_id",
            "page_id",
            "lifecycle_generation",
        ] {
            let mut invalid = request.clone();
            invalid[key] = json!("foreign");
            assert!(
                !owner.matches_request(&grant.requester_endpoint, &invalid),
                "{key}"
            );
        }
        let other = iroh::SecretKey::from_bytes(&[23; 32]).public();
        assert!(!owner.matches_request(&other, &request));
        let mut second = owner.clone();
        second.requester_endpoint = other.to_string();
        assert_ne!(owner.storage_principal(), second.storage_principal());
        second = owner.clone();
        second.requester_principal_id = "another".into();
        assert_ne!(owner.storage_principal(), second.storage_principal());
        assert!(!owner
            .storage_principal()
            .contains(&grant.requester_principal_id));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn egress_uses_actual_endpoint_and_retained_socket_then_cancels_on_close() {
        use iroh::Watcher as _;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let consumer_root = tempfile::tempdir().unwrap();
        let engine_root = tempfile::tempdir().unwrap();
        let consumer_key = ed25519_dalek::SigningKey::from_bytes(&[24; 32]);
        let engine_key = ed25519_dalek::SigningKey::from_bytes(&[22; 32]);
        let consumer_did =
            super::super::public_key_to_did(&iroh::SecretKey::from_bytes(&[24; 32]).public())
                .unwrap();
        let engine_did =
            super::super::public_key_to_did(&iroh::SecretKey::from_bytes(&[22; 32]).public())
                .unwrap();
        let consumer = super::super::start_isolated_carrier_node_with_registry(
            &consumer_key,
            &consumer_did,
            consumer_root.path().into(),
            None,
        )
        .await
        .unwrap();
        let engine = super::super::start_isolated_carrier_node_with_registry(
            &engine_key,
            &engine_did,
            engine_root.path().into(),
            None,
        )
        .await
        .unwrap();
        engine
            .memory_lookup
            .add_endpoint_info(consumer.endpoint.watch_addr().get());
        let path = consumer_root.path().join("exit.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let (grant, request) = owner_fixture();
        let mut owner = RemoteEngineOwner::from_launch(&grant, &request).unwrap();
        owner.requester_endpoint = consumer.endpoint.id().to_string();
        bind_egress(
            consumer_root.path(),
            engine.endpoint.id(),
            &owner.page_id,
            &owner.generation,
            "stream:owned",
            &path,
        )
        .await
        .unwrap();
        let slots = egress::slots_for_test(consumer_root.path(), &owner.page_id).await;
        let echo = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4];
            socket.read_exact(&mut bytes).await.unwrap();
            assert_eq!(&bytes, b"test");
            socket.write_all(b"pass").await.unwrap();
            let mut byte = [0; 1];
            assert_eq!(socket.read(&mut byte).await.unwrap(), 0);
        });
        let mut stream = connect_egress(&engine.endpoint, &owner, "stream:owned")
            .await
            .unwrap();
        stream.send.write_all(b"test").await.unwrap();
        let mut reply = [0; 4];
        stream.recv.read_exact(&mut reply).await.unwrap();
        assert_eq!(&reply, b"pass");
        let anonymous = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .bind()
            .await
            .unwrap();
        // Seed only routing information; it confers no sender authority.
        let client = super::super::CarrierClient::connect_known_endpoint(
            &anonymous,
            consumer.endpoint.watch_addr().get(),
            2,
        )
        .await
        .unwrap();
        let (mut send, mut recv) = client.conn.open_bi().await.unwrap();
        super::super::write_json_line(
            &mut send,
            &json!({"op":"browser_engine_egress","page_id":owner.page_id,
            "generation":owner.generation,"stream_id":"stream:owned","path":"/private/foreign"}),
        )
        .await
        .unwrap();
        send.finish().unwrap();
        let response = recv.read_to_end(1024).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&response).unwrap()["ok"],
            false
        );
        assert!(connect_egress(&engine.endpoint, &owner, "stream:other")
            .await
            .is_err());
        remove_egress(consumer_root.path(), &owner.page_id, &owner.generation)
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), echo)
            .await
            .unwrap()
            .unwrap();
        assert!(connect_egress(&engine.endpoint, &owner, "stream:owned")
            .await
            .is_err());
        wait_for_egress_slots(&slots).await.unwrap();
        anonymous.close().await;
        engine.endpoint.close().await;
        consumer.endpoint.close().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn egress_propagates_each_eof_without_waiting_for_reverse_stream() {
        use iroh::Watcher as _;
        use std::{sync::Arc, time::Duration};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let consumer_root = tempfile::tempdir().unwrap();
        let engine_root = tempfile::tempdir().unwrap();
        let consumer_key = ed25519_dalek::SigningKey::from_bytes(&[26; 32]);
        let engine_key = ed25519_dalek::SigningKey::from_bytes(&[27; 32]);
        let consumer_did =
            super::super::public_key_to_did(&iroh::SecretKey::from_bytes(&[26; 32]).public())
                .unwrap();
        let engine_did =
            super::super::public_key_to_did(&iroh::SecretKey::from_bytes(&[27; 32]).public())
                .unwrap();
        let consumer = super::super::start_isolated_carrier_node_with_registry(
            &consumer_key,
            &consumer_did,
            consumer_root.path().into(),
            None,
        )
        .await
        .unwrap();
        let engine = super::super::start_isolated_carrier_node_with_registry(
            &engine_key,
            &engine_did,
            engine_root.path().into(),
            None,
        )
        .await
        .unwrap();
        engine
            .memory_lookup
            .add_endpoint_info(consumer.endpoint.watch_addr().get());
        let path = consumer_root.path().join("exit.sock");
        let listener = Arc::new(tokio::net::UnixListener::bind(&path).unwrap());
        let (grant, request) = owner_fixture();
        let mut owner = RemoteEngineOwner::from_launch(&grant, &request).unwrap();
        owner.requester_endpoint = consumer.endpoint.id().to_string();
        bind_egress(
            consumer_root.path(),
            engine.endpoint.id(),
            &owner.page_id,
            &owner.generation,
            "stream:eof",
            &path,
        )
        .await
        .unwrap();
        let slots = egress::slots_for_test(consumer_root.path(), &owner.page_id).await;
        let exercise = async {
            for exit_first in [true, false] {
                let listener = listener.clone();
                let mut exit = tokio::spawn(async move {
                    let (mut socket, _) = listener.accept().await?;
                    let mut request = [0; 4];
                    socket.read_exact(&mut request).await?;
                    ensure!(&request == b"test", "wrong Engine request");
                    if exit_first {
                        socket.write_all(b"pass").await?;
                        socket.shutdown().await?;
                        // Sending FIN must leave the request direction writable.
                        let mut tail = [0; 4];
                        socket
                            .read_exact(&mut tail)
                            .await
                            .context("Exit lost reverse tail after sending FIN")?;
                        ensure!(&tail == b"tail", "Engine reverse traffic was lost");
                    }
                    let mut byte = [0; 1];
                    ensure!(socket.read(&mut byte).await? == 0, "Engine EOF missing");
                    if !exit_first {
                        // The response starts only after the Engine request EOF.
                        socket.write_all(b"pass").await?;
                        socket.shutdown().await?;
                    }
                    Ok::<_, anyhow::Error>(())
                });
                let transfer = tokio::time::timeout(Duration::from_secs(8), async {
                    let mut stream = connect_egress(&engine.endpoint, &owner, "stream:eof").await?;
                    stream.send.write_all(b"test").await?;
                    if !exit_first {
                        stream.send.finish()?;
                    }
                    let mut reply = [0; 4];
                    stream
                        .recv
                        .read_exact(&mut reply)
                        .await
                        .context("Engine lost Exit response")?;
                    ensure!(&reply == b"pass", "wrong Exit response");
                    let suffix =
                        tokio::time::timeout(Duration::from_secs(2), stream.recv.read_to_end(1024))
                            .await
                            .context("Exit EOF withheld while Engine writer remains open")??;
                    ensure!(suffix.is_empty(), "unexpected Exit response suffix");
                    if exit_first {
                        ensure!(
                            slots.available_permits() == 127,
                            "Exit EOF released a still-active reverse stream"
                        );
                        stream.send.write_all(b"tail").await?;
                        stream.send.finish()?;
                    }
                    // Retain the connection while the Exit drains the reverse bytes.
                    Ok::<_, anyhow::Error>(stream)
                })
                .await
                .context("Engine EOF transfer deadline")
                .and_then(|result| result);
                // Capture failure before cleanup: even the expected old-code timeout
                // must leave no Exit task, endpoint or retained binding behind.
                let peer_result = if transfer.is_ok() {
                    match tokio::time::timeout(Duration::from_secs(2), &mut exit).await {
                        Ok(result) => result
                            .context("Exit task panicked")
                            .and_then(|result| result),
                        Err(_) => {
                            exit.abort();
                            let _ = exit.await;
                            Err(anyhow::anyhow!("Exit task deadline"))
                        }
                    }
                } else {
                    exit.abort();
                    let _ = exit.await;
                    Ok(())
                };
                let stream = transfer?;
                peer_result?;
                let stopped = tokio::time::timeout(Duration::from_secs(2), stream.send.stopped())
                    .await
                    .context("Engine send acknowledgement deadline")??;
                ensure!(
                    stopped.is_none(),
                    "Exit stopped the Engine stream before delivery"
                );
                wait_for_egress_slots(&slots).await?;
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let removed = remove_egress(consumer_root.path(), &owner.page_id, &owner.generation).await;
        let released = wait_for_egress_slots(&slots).await;
        engine.shutdown().await;
        consumer.shutdown().await;
        removed.unwrap();
        released.unwrap();
        exercise.unwrap();
    }
}
