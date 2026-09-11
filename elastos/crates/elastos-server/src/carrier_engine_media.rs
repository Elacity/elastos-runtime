//! A page's TURN TCP ingress, relayed between its two owning Runtimes. Viewer
//! sockets never select a destination; the serving Runtime retains that binding.
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    sync::{watch, Mutex, Semaphore},
};

struct MediaTarget {
    source: iroh::PublicKey,
    generation: String,
    binding_hash: String,
    address: std::net::SocketAddr,
    retiring: Arc<AtomicBool>,
    slots: Arc<Semaphore>,
}
static TARGETS: OnceLock<Mutex<BTreeMap<(PathBuf, String), MediaTarget>>> = OnceLock::new();

pub(crate) async fn bind_target(
    root: &Path,
    source: iroh::PublicKey,
    authority: &Value,
    retiring: Arc<AtomicBool>,
) -> Result<()> {
    let page = authority["page_id"]
        .as_str()
        .context("Engine media page missing")?;
    let host = authority["turn"]["listen_host"]
        .as_str()
        .context("Engine TURN host missing")?
        .parse::<std::net::IpAddr>()?;
    ensure!(
        host.is_loopback(),
        "Engine TURN target must be owned loopback"
    );
    let port = authority["turn"]["listen_port"]
        .as_u64()
        .filter(|p| *p > 0 && *p <= 65535)
        .context("Engine TURN port invalid")?;
    let mut targets = TARGETS.get_or_init(Default::default).lock().await;
    let key = (root.to_owned(), page.into());
    ensure!(
        !targets.contains_key(&key),
        "Engine media target already owned"
    );
    targets.insert(
        key,
        MediaTarget {
            source,
            generation: authority["generation"]
                .as_str()
                .context("Engine generation missing")?
                .into(),
            binding_hash: authority["binding_hash"]
                .as_str()
                .context("Engine binding missing")?
                .into(),
            address: std::net::SocketAddr::new(host, port as u16),
            retiring,
            slots: Arc::new(Semaphore::new(128)),
        },
    );
    Ok(())
}

pub(crate) async fn remove_target(root: &Path, page: &str, generation: &str) -> Result<()> {
    let mut targets = TARGETS.get_or_init(Default::default).lock().await;
    let key = (root.to_owned(), page.into());
    if let Some(target) = targets.get(&key) {
        ensure!(
            target.generation == generation,
            "Engine media cleanup owner mismatch"
        );
        target.retiring.store(true, Ordering::Release);
        targets.remove(&key);
    }
    Ok(())
}

pub(crate) async fn serve(
    root: &Path,
    source: iroh::PublicKey,
    request: &Value,
    send: &mut iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    buffered: Vec<u8>,
) -> Result<()> {
    let admitted = async {
        ensure!(
            serde_json::to_vec(request)?.len() <= 1024
                && request.as_object().is_some_and(|object| object.len() == 3
                    && object.keys().all(|key| matches!(
                        key.as_str(),
                        "page_id" | "generation" | "binding_hash"
                    ))),
            "Engine media request invalid"
        );
        let targets = TARGETS.get_or_init(Default::default).lock().await;
        let target = targets
            .get(&(
                root.to_owned(),
                request["page_id"]
                    .as_str()
                    .context("Engine media page required")?
                    .into(),
            ))
            .context("Engine media owner absent")?;
        ensure!(
            target.source == source
                && request["generation"] == target.generation
                && request["binding_hash"] == target.binding_hash
                && !target.retiring.load(Ordering::Acquire),
            "Engine media owner changed"
        );
        Ok::<_, anyhow::Error>((
            target.address,
            target.retiring.clone(),
            target.slots.clone().try_acquire_owned()?,
        ))
    }
    .await;
    let (address, retiring, _slot) = match admitted {
        Ok(binding) => binding,
        Err(_) => {
            super::send_json(
                send,
                &json!({"ok":false,"error":"Runtime Engine media is unavailable"}),
            )
            .await?;
            return Ok(());
        }
    };
    let mut target = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::TcpStream::connect(address),
    )
    .await??;
    ensure!(
        !retiring.load(Ordering::Acquire),
        "Engine media retired during connect"
    );
    super::write_json_line(send, &json!({"ok":true})).await?;
    target.write_all(&buffered).await?;
    let (mut read, mut write) = target.into_split();
    tokio::select! {
        _=async {loop {tokio::time::sleep(Duration::from_millis(100)).await;if retiring.load(Ordering::Acquire){break;}}}=>{},
        result=async {
            tokio::try_join!(async {tokio::io::copy(&mut recv,&mut write).await?;write.shutdown().await},
                async {tokio::io::copy(&mut read,&mut *send).await?;Ok::<_,std::io::Error>(())})?;
            Ok::<_,anyhow::Error>(())
        }=>{result?;}
    }
    send.finish().ok();
    Ok(())
}

async fn connect(
    endpoint: &iroh::Endpoint,
    grant: &Value,
    authority: &Value,
) -> Result<super::BrowserCarrierStream> {
    let peer = grant["peer_did"]
        .as_str()
        .context("Engine peer missing")?
        .parse::<iroh::PublicKey>()?;
    let addresses = super::decode_ticket_endpoints(
        grant["connect_ticket"]
            .as_str()
            .context("Engine route missing")?,
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        for address in addresses.into_iter().filter(|a| a.id == peer) {
            let Ok(client) =
                super::CarrierClient::connect_known_endpoint(endpoint, address, 5).await
            else {
                continue;
            };
            let (mut send, mut recv) = client.conn.open_bi().await?;
            super::write_json_line(
                &mut send,
                &json!({"op":"browser_engine_media","page_id":authority["page_id"],
                "generation":authority["generation"],"binding_hash":authority["binding_hash"]}),
            )
            .await?;
            let mut response = Vec::new();
            loop {
                let mut b = [0; 1];
                recv.read_exact(&mut b).await?;
                ensure!(
                    response.len() < 1024,
                    "Engine media acknowledgement too large"
                );
                if b[0] == b'\n' {
                    break;
                }
                response.push(b[0]);
            }
            ensure!(
                serde_json::from_slice::<Value>(&response)?["ok"] == true,
                "Engine media was not admitted"
            );
            return Ok(super::BrowserCarrierStream {
                send,
                recv,
                _client: client,
            });
        }
        anyhow::bail!("Engine media route unavailable")
    })
    .await
    .context("Engine media connection deadline")?
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct IngressConfig {
    schema: String,
    listen_host: std::net::IpAddr,
    advertised_host: String,
    port_start: u16,
    port_end: u16,
}
struct Ingress {
    generation: String,
    descriptor: Value,
    cancel: watch::Sender<bool>,
    closed: watch::Receiver<bool>,
}
static INGRESS: OnceLock<Mutex<BTreeMap<(PathBuf, String), Ingress>>> = OnceLock::new();

fn read_config(root: &Path) -> Result<IngressConfig> {
    use std::io::Read;
    let path = root.join("config/browser-viewer-ingress.json");
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options
        .open(path)
        .context("Runtime viewer ingress needs an approved reachable route")?;
    ensure!(
        file.metadata()?.is_file() && file.metadata()?.len() <= 4096,
        "Runtime viewer ingress config invalid"
    );
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    let config: IngressConfig = serde_json::from_slice(&bytes)?;
    ensure!(
        config.schema == "elastos.browser.viewer-ingress-config/v1"
            && config.port_start > 0
            && config.port_end >= config.port_start
            && config.port_end - config.port_start < 64
            && valid_host(&config.advertised_host),
        "Runtime viewer ingress config invalid"
    );
    Ok(config)
}
fn valid_host(host: &str) -> bool {
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return !ip.is_unspecified() && !ip.is_multicast();
    }
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && !part.starts_with('-')
                && !part.ends_with('-')
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}
pub(crate) fn configuration_ready(root: &Path) -> Result<()> {
    read_config(root).map(|_| ())
}

pub(crate) async fn start_ingress(
    root: &Path,
    endpoint: &iroh::Endpoint,
    grant: &Value,
    authority: &Value,
) -> Result<Value> {
    let config = read_config(root)?;
    let page = authority["page_id"]
        .as_str()
        .context("Viewer page required")?;
    let generation = authority["generation"]
        .as_str()
        .context("Viewer generation required")?;
    let mut ingresses = INGRESS.get_or_init(Default::default).lock().await;
    let key = (root.to_owned(), page.into());
    if let Some(ingress) = ingresses.get(&key) {
        ensure!(
            ingress.generation == generation
                && ingress.descriptor["engine_binding_hash"] == authority["binding_hash"]
                && !*ingress.cancel.borrow()
                && !*ingress.closed.borrow()
                && ingress.descriptor["owner_runtime_hash"]
                    == runtime_hash(&endpoint.id().to_string())
                && ingress.descriptor["engine_runtime_hash"]
                    == runtime_hash(
                        grant["peer_did"]
                            .as_str()
                            .context("Engine identity missing")?
                    ),
            "Viewer ingress owner changed"
        );
        return Ok(ingress.descriptor.clone());
    }
    let mut listener = None;
    for port in config.port_start..=config.port_end {
        if let Ok(bound) = tokio::net::TcpListener::bind((config.listen_host, port)).await {
            listener = Some(bound);
            break;
        }
    }
    let listener = listener.context("Runtime viewer ingress capacity unavailable")?;
    let host = if config.advertised_host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{}]", config.advertised_host)
    } else {
        config.advertised_host
    };
    let descriptor = json!({"schema":"elastos.browser.viewer-ingress/v1","page_id":page,"generation":generation,
        "engine_binding_hash":authority["binding_hash"],"owner_runtime_hash":runtime_hash(&endpoint.id().to_string()),"engine_runtime_hash":runtime_hash(grant["peer_did"].as_str().context("Engine identity missing")?),
        "turn_url":format!("turn:{host}:{}?transport=tcp",listener.local_addr()?.port())});
    let (cancel, mut cancelled) = watch::channel(false);
    let (done, closed) = watch::channel(false);
    ingresses.insert(
        key,
        Ingress {
            generation: generation.into(),
            descriptor: descriptor.clone(),
            cancel,
            closed,
        },
    );
    drop(ingresses);
    let endpoint = endpoint.clone();
    let grant = grant.clone();
    let authority = authority.clone();
    tokio::spawn(async move {
        let mut children = tokio::task::JoinSet::new();
        let slots = Arc::new(Semaphore::new(128));
        loop {
            tokio::select! {
                _=cancelled.changed()=>break,
                result=listener.accept()=>match result {
                    Ok((socket,_))=>{
                        let Ok(slot)=slots.clone().try_acquire_owned() else {drop(socket);continue;};
                        let endpoint=endpoint.clone();let grant=grant.clone();let authority=authority.clone();
                        children.spawn(async move {
                            let _slot=slot;
                            let mut remote=connect(&endpoint,&grant,&authority).await?;
                            let (mut read,mut write)=socket.into_split();
                            tokio::try_join!(async {tokio::io::copy(&mut read,&mut remote.send).await?;remote.send.finish()?;Ok::<_,anyhow::Error>(())},
                                async {tokio::io::copy(&mut remote.recv,&mut write).await?;write.shutdown().await?;Ok::<_,anyhow::Error>(())})?;
                            Ok::<_,anyhow::Error>(())
                        });
                    },Err(_)=>break,
                },
                _=children.join_next(),if !children.is_empty()=>{},
            }
        }
        children.abort_all();
        while children.join_next().await.is_some() {}
        drop(listener);
        done.send_replace(true);
    });
    Ok(descriptor)
}

pub(crate) async fn close_ingress(root: &Path, page: &str, generation: &str) -> Result<()> {
    let mut ingresses = INGRESS.get_or_init(Default::default).lock().await;
    let key = (root.to_owned(), page.into());
    let Some(ingress) = ingresses.get(&key) else {
        return Ok(());
    };
    ensure!(
        ingress.generation == generation,
        "Viewer ingress cleanup owner changed"
    );
    ingress.cancel.send_replace(true);
    let mut closed = ingress.closed.clone();
    drop(ingresses);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !*closed.borrow() {
            closed.changed().await?;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    ingresses = INGRESS.get_or_init(Default::default).lock().await;
    ingresses.remove(&key);
    Ok(())
}

fn runtime_hash(identity: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(identity)))
}

pub(crate) fn validate_ingress(authority: &Value, ingress: &Value, turn_url: &str) -> Result<()> {
    let object = ingress
        .as_object()
        .context("Viewer ingress binding missing")?;
    ensure!(
        object.len() == 7
            && ingress["schema"] == "elastos.browser.viewer-ingress/v1"
            && ingress["page_id"] == authority["page_id"]
            && ingress["generation"] == authority["generation"]
            && ingress["engine_binding_hash"] == authority["binding_hash"]
            && ingress["turn_url"] == turn_url,
        "Viewer ingress binding changed"
    );
    for key in ["owner_runtime_hash", "engine_runtime_hash"] {
        ensure!(
            ingress[key]
                .as_str()
                .and_then(|value| value.strip_prefix("sha256:"))
                .is_some_and(|hash| hash.len() == 64
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))),
            "Viewer Runtime identity invalid"
        );
    }
    let rest = turn_url
        .strip_prefix("turn:")
        .and_then(|s| s.strip_suffix("?transport=tcp"))
        .context("Viewer TURN transport invalid")?;
    let (host, port) = rest
        .rsplit_once(':')
        .context("Viewer TURN endpoint invalid")?;
    ensure!(
        valid_host(host.trim_start_matches('[').trim_end_matches(']'))
            && port.parse::<u16>().is_ok_and(|p| p > 0),
        "Viewer TURN endpoint invalid"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[test]
    fn remote_viewer_capability_keeps_native_owner_and_explicit_route() {
        let root = tempfile::tempdir().unwrap();
        assert!(configuration_ready(root.path()).is_err());
        for host in ["0.0.0.0", "::", "224.0.0.1", "user@host", "host/path"] {
            assert!(!valid_host(host), "{host}");
        }
        for host in ["127.0.0.1", "::1", "viewer.example.test"] {
            assert!(valid_host(host), "{host}");
        }
        let authority = json!({"page_id":"page:owned","generation":runtime_hash("generation"),"binding_hash":runtime_hash("binding")});
        let ingress = json!({"schema":"elastos.browser.viewer-ingress/v1","page_id":authority["page_id"],
            "generation":authority["generation"],"engine_binding_hash":authority["binding_hash"],
            "owner_runtime_hash":runtime_hash("consumer"),"engine_runtime_hash":runtime_hash("engine"),
            "turn_url":"turn:viewer.example.test:49100?transport=tcp"});
        validate_ingress(&authority, &ingress, ingress["turn_url"].as_str().unwrap()).unwrap();
        for key in [
            "page_id",
            "generation",
            "engine_binding_hash",
            "owner_runtime_hash",
            "engine_runtime_hash",
            "turn_url",
        ] {
            let mut invalid = ingress.clone();
            invalid[key] = json!("foreign");
            assert!(
                validate_ingress(&authority, &invalid, ingress["turn_url"].as_str().unwrap())
                    .is_err(),
                "{key}"
            );
        }
    }

    #[tokio::test]
    async fn viewer_tcp_bytes_cross_runtime_carrier_and_close_exact_owner() {
        tokio::time::timeout(Duration::from_secs(15),async {
            let consumer_root=tempfile::tempdir().unwrap();let engine_root=tempfile::tempdir().unwrap();
            let consumer_key=ed25519_dalek::SigningKey::from_bytes(&[64;32]);
            let engine_key=ed25519_dalek::SigningKey::from_bytes(&[65;32]);
            let consumer_did=super::super::public_key_to_did(&iroh::SecretKey::from_bytes(&[64;32]).public()).unwrap();
            let engine_did=super::super::public_key_to_did(&iroh::SecretKey::from_bytes(&[65;32]).public()).unwrap();
            let consumer=super::super::start_isolated_carrier_node_with_registry(&consumer_key,&consumer_did,consumer_root.path().into(),None).await.unwrap();
            let engine=super::super::start_isolated_carrier_node_with_registry(&engine_key,&engine_did,engine_root.path().into(),None).await.unwrap();
            // Only this Runtime-installed TCP target can receive media bytes.
            let target=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let authority=json!({"page_id":"page:media-test","generation":runtime_hash("generation"),"binding_hash":runtime_hash("binding"),
                "turn":{"listen_host":"127.0.0.1","listen_port":target.local_addr().unwrap().port()}});
            let retiring=Arc::new(AtomicBool::new(false));
            bind_target(engine_root.path(),consumer.endpoint.id(),&authority,retiring.clone()).await.unwrap();
            let grant=json!({"peer_did":engine.endpoint.id().to_string(),"connect_ticket":super::super::carrier_connect_ticket(&engine.endpoint)});
            let unused=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let port=unused.local_addr().unwrap().port();drop(unused);
            std::fs::create_dir_all(consumer_root.path().join("config")).unwrap();
            std::fs::write(consumer_root.path().join("config/browser-viewer-ingress.json"),serde_json::to_vec(&json!({
                "schema":"elastos.browser.viewer-ingress-config/v1","listen_host":"127.0.0.1","advertised_host":"127.0.0.1",
                "port_start":port,"port_end":port})).unwrap()).unwrap();
            let ingress=start_ingress(consumer_root.path(),&consumer.endpoint,&grant,&authority).await.unwrap();
            validate_ingress(&authority,&ingress,ingress["turn_url"].as_str().unwrap()).unwrap();
            assert_eq!(start_ingress(consumer_root.path(),&consumer.endpoint,&grant,&authority).await.unwrap(),ingress);
            assert!(!ingress.to_string().contains(&engine.endpoint.id().to_string()));
            let echo=tokio::spawn(async move {
                let (mut socket,_)=target.accept().await.unwrap();let mut bytes=[0;8];
                socket.read_exact(&mut bytes).await.unwrap();assert_eq!(&bytes,b"turndata");
                socket.write_all(b"received").await.unwrap();
                let mut byte=[0;1];assert_eq!(socket.read(&mut byte).await.unwrap(),0);
            });
            let mut viewer=tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST,port)).await.unwrap();
            viewer.write_all(b"turndata").await.unwrap();let mut bytes=[0;8];viewer.read_exact(&mut bytes).await.unwrap();assert_eq!(&bytes,b"received");
            let anonymous=iroh::Endpoint::builder(iroh::endpoint::presets::Minimal).bind().await.unwrap();
            assert!(connect(&anonymous,&grant,&authority).await.is_err(),"routing information cannot confer Runtime authority");
            let mut stale=authority.clone();stale["generation"]=json!(runtime_hash("stale"));
            assert!(connect(&consumer.endpoint,&grant,&stale).await.is_err());
            assert!(close_ingress(consumer_root.path(),"page:media-test","foreign").await.is_err());
            remove_target(engine_root.path(),"page:media-test",authority["generation"].as_str().unwrap()).await.unwrap();
            echo.await.unwrap();
            close_ingress(consumer_root.path(),"page:media-test",authority["generation"].as_str().unwrap()).await.unwrap();
            assert!(tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST,port)).await.is_err());
            assert!(connect(&consumer.endpoint,&grant,&authority).await.is_err());
            anonymous.close().await;consumer.endpoint.close().await;engine.endpoint.close().await;
        }).await.unwrap();
    }
}
