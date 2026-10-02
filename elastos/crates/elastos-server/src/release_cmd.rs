use std::sync::Arc;

use elastos_server::update;

pub async fn run_publish_release(
    options: crate::publish::PublishReleaseOptions,
) -> anyhow::Result<()> {
    crate::publish::run_publish_release(options).await
}

pub fn run_source(cmd: crate::sources::SourceCommand) -> anyhow::Result<()> {
    crate::sources::run_source_command(cmd, crate::publish::source_discovery_uri)
}

pub fn run_version(current_version: &str) {
    println!("ElastOS Runtime v{}", current_version);
}

pub async fn run_update_command(
    check: bool,
    head_cid: Option<String>,
    no_p2p: bool,
    gateways: Vec<String>,
    yes: bool,
    rollback_to: Option<String>,
    current_version: &'static str,
) -> anyhow::Result<()> {
    let data_dir = crate::sources::default_data_dir();
    run_update_command_for_data_dir(
        &data_dir,
        check,
        head_cid,
        no_p2p,
        gateways,
        yes,
        rollback_to,
        current_version,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_update_command_for_data_dir(
    data_dir: &std::path::Path,
    check: bool,
    head_cid: Option<String>,
    no_p2p: bool,
    gateways: Vec<String>,
    yes: bool,
    rollback_to: Option<String>,
    current_version: &'static str,
) -> anyhow::Result<()> {
    let sources = elastos_server::sources::load_trusted_sources(data_dir)?;
    let source_config = sources
        .default_source()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("No trusted source configured"))?;
    let carrier_client = if !no_p2p {
        match elastos_server::carrier::CarrierClient::connect_trusted_source(&source_config, 10)
            .await
        {
            Ok(c) => Some(Arc::new(c)),
            Err(e) => {
                anyhow::bail!("Carrier connection failed: {:#}", e);
            }
        }
    } else {
        None
    };

    let gateway_only_fetch = no_p2p;
    let carrier_for_fetch = carrier_client.clone();
    let fetch_fn: update::FetchFn = Box::new(move |cid, gateways| {
        let client = carrier_for_fetch.clone();
        Box::pin(async move {
            let mut carrier_error: Option<anyhow::Error> = None;

            if !gateway_only_fetch {
                if let Some(client) = client.as_ref() {
                    match client.fetch_content(&cid, None).await {
                        Ok(bytes) => return Ok(bytes),
                        Err(err) => carrier_error = Some(err),
                    }
                } else {
                    carrier_error = Some(anyhow::anyhow!("No Carrier connection"));
                }
            }

            if !gateways.is_empty() {
                return update::fetch_cid_via_gateways(&cid, &gateways).await;
            }

            Err(carrier_error.unwrap_or_else(|| {
                anyhow::anyhow!("No Carrier connection and no gateway configured")
            }))
        })
    });

    let carrier_for_discovery = carrier_client.clone();
    let try_p2p: update::TryP2pFn = Box::new(move |source, _publisher_did| {
        let client = carrier_for_discovery.clone();
        Box::pin(async move {
            match client {
                Some(client) => update::discover_carrier_release_head(&client, &source)
                    .await
                    .map(Some),
                None => Ok(None),
            }
        })
    });

    let effective_head_cid = rollback_to.clone().or(head_cid);
    let force = rollback_to.is_some();
    let result = update::run_update_for_data_dir(
        data_dir,
        &fetch_fn,
        Some(&try_p2p),
        check,
        effective_head_cid,
        no_p2p,
        gateways,
        current_version,
        yes,
        force,
    )
    .await;
    if let Some(client) = carrier_client {
        client.close().await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_server::sources::{save_trusted_sources, TrustedSource, TrustedSourcesConfig};
    use sha2::{Digest, Sha256};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, BufReader};

    #[derive(Clone, Copy, Debug)]
    enum Reply {
        SameVersion,
        WrongSignature,
        ContentFailure,
        SilentDiscovery,
    }

    fn signed_envelope(payload: serde_json::Value, domain: &str) -> Vec<u8> {
        let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let (signature, signer_did) = elastos_server::crypto::domain_separated_sign(
            &key,
            domain,
            &serde_json::to_vec(&payload).unwrap(),
        );
        serde_json::to_vec(&serde_json::json!({
            "payload": payload, "signature": signature, "signer_did": signer_did
        }))
        .unwrap()
    }

    fn raw_cid(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        let hash = cid::multihash::Multihash::<64>::wrap(0x12, digest.as_slice()).unwrap();
        cid::Cid::new_v1(0x55, hash).to_string()
    }

    async fn update_cleanup_case(reply: Reply) {
        let release = signed_envelope(
            serde_json::json!({
                "schema": "elastos.release/v1", "version": "0.7.1", "channel": "stable"
            }),
            "elastos.release.v1",
        );
        let release_cid = raw_cid(&release);
        let mut head = signed_envelope(
            serde_json::json!({
                "schema": "elastos.release.head/v1", "version": "0.7.1", "channel": "stable",
                "latest_release_cid": release_cid, "release_sha256": hex::encode(Sha256::digest(&release))
            }),
            "elastos.release.head.v1",
        );
        if matches!(reply, Reply::WrongSignature) {
            let mut envelope: serde_json::Value = serde_json::from_slice(&head).unwrap();
            envelope["signature"] = serde_json::json!("00".repeat(64));
            head = serde_json::to_vec(&envelope).unwrap();
        }
        let head_cid = raw_cid(&head);
        let server = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .alpns(vec![b"elastos/carrier/1".to_vec()])
            .bind()
            .await
            .unwrap();
        let port = server
            .bound_sockets()
            .into_iter()
            .find(|address| address.is_ipv4())
            .unwrap()
            .port();
        let address = iroh::EndpointAddr::from(server.id())
            .with_addrs([iroh::TransportAddr::Ip(([127, 0, 0, 1], port).into())]);
        let mut ticket = data_encoding::BASE32_NOPAD.encode(
            &serde_json::to_vec(&serde_json::json!({"topic": null, "endpoints": [address]}))
                .unwrap(),
        );
        ticket.make_ascii_lowercase();
        let fixture = tempfile::tempdir().unwrap();
        let signer = elastos_server::crypto::encode_signing_key_did(
            &ed25519_dalek::SigningKey::from_bytes(&[7; 32]),
        );
        let source: TrustedSource = serde_json::from_value(serde_json::json!({
            "name": "fixture", "publisher_dids": [signer], "channel": "stable",
            "installed_version": "0.7.1", "publisher_node_id": server.id().to_string(),
            "connect_ticket": ticket
        }))
        .unwrap();
        let mut sources = TrustedSourcesConfig::empty();
        sources.upsert_source(source);
        save_trusted_sources(fixture.path(), &sources).unwrap();
        let sources_before = std::fs::read(fixture.path().join("sources.json")).unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let accepted_by_server = accepted.clone();
        let requests_by_server = requests.clone();
        let server_endpoint = server.clone();
        let mut serving = tokio::spawn(async move {
            let connection = server_endpoint.accept().await.unwrap().await.unwrap();
            accepted_by_server.fetch_add(1, Ordering::SeqCst);
            let mut silent_streams = Vec::new();
            loop {
                tokio::select! {
                    biased;
                    closed = connection.closed() => break closed,
                    incoming = server_endpoint.accept() => {
                        let _extra = incoming.unwrap().await.unwrap();
                        accepted_by_server.fetch_add(1, Ordering::SeqCst);
                        panic!("update opened a second Carrier connection");
                    }
                    stream = connection.accept_bi() => {
                        let (mut send, recv) = match stream {
                            Ok(stream) => stream,
                            Err(_) => break connection.closed().await,
                        };
                        let mut reader = BufReader::new(recv);
                        let mut line = String::new();
                        reader.read_line(&mut line).await.unwrap();
                        let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                        let op = request["op"].as_str().unwrap();
                        requests_by_server.lock().unwrap().push(op.to_string());
                        match op {
                            "release_head" if matches!(reply, Reply::SilentDiscovery) => {
                                silent_streams.push(send);
                                continue;
                            }
                            "release_head" => {
                                let mut response = serde_json::to_vec(&serde_json::json!({
                                    "ok": true, "release": {"head_cid": head_cid, "release_cid": release_cid}
                                })).unwrap();
                                response.push(b'\n');
                                send.write_all(&response).await.unwrap();
                            }
                            "content_fetch" => {
                                let cid = request["cid"].as_str().unwrap();
                                let bytes = if cid == head_cid {
                                    &head
                                } else {
                                    assert_eq!(cid, release_cid);
                                    if matches!(reply, Reply::ContentFailure) {
                                        send.write_all(b"{\"ok\":false,\"error\":\"fixture content unavailable\"}\n").await.unwrap();
                                        send.finish().unwrap();
                                        continue;
                                    }
                                    &release
                                };
                                send.write_all(&(bytes.len() as u64).to_be_bytes()).await.unwrap();
                                send.write_all(bytes).await.unwrap();
                            }
                            _ => panic!("unexpected update request: {op}"),
                        }
                        send.finish().unwrap();
                    }
                }
            }
        });
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            run_update_command_for_data_dir(
                fixture.path(),
                true,
                None,
                false,
                Vec::new(),
                true,
                None,
                "0.7.1",
            ),
        )
        .await;
        let remote_close = tokio::time::timeout(Duration::from_secs(5), &mut serving).await;
        server.close().await;
        if remote_close.is_err() {
            serving.abort();
            let _ = serving.await;
        }
        let result = result.expect("update wrapper exceeded its bounded fixture budget");
        let remote_close = remote_close
            .expect("update returned without closing its Carrier connection")
            .unwrap();
        assert!(matches!(
            remote_close,
            iroh::endpoint::ConnectionError::ApplicationClosed(_)
        ));
        assert_eq!(accepted.load(Ordering::SeqCst), 1);
        assert_eq!(
            std::fs::read(fixture.path().join("sources.json")).unwrap(),
            sources_before
        );
        let expected_requests = match reply {
            Reply::SameVersion => {
                result.unwrap();
                vec!["release_head", "content_fetch", "content_fetch"]
            }
            Reply::WrongSignature => {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("signature verification failed"));
                vec!["release_head", "content_fetch"]
            }
            Reply::ContentFailure => {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("fixture content unavailable"));
                vec!["release_head", "content_fetch", "content_fetch"]
            }
            Reply::SilentDiscovery => {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("Could not discover updates"));
                vec!["release_head"]
            }
        };
        assert_eq!(*requests.lock().unwrap(), expected_requests);
    }

    #[tokio::test]
    async fn update_same_version_check_closes_its_only_carrier_connection() {
        update_cleanup_case(Reply::SameVersion).await;
    }

    #[tokio::test]
    async fn update_wrong_signature_closes_its_only_carrier_connection() {
        update_cleanup_case(Reply::WrongSignature).await;
    }

    #[tokio::test]
    async fn update_content_failure_closes_its_only_carrier_connection() {
        update_cleanup_case(Reply::ContentFailure).await;
    }

    #[tokio::test]
    async fn update_discovery_timeout_closes_its_only_carrier_connection() {
        update_cleanup_case(Reply::SilentDiscovery).await;
    }
}
