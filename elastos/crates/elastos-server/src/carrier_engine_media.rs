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
    sync::{oneshot, watch, Mutex, Semaphore},
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
    disable_media_nagle(&target)?;
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

enum WarmPayload {
    Ready(super::BrowserCarrierStream),
    Pending {
        client: super::CarrierClient,
        send: iroh::endpoint::SendStream,
        recv: iroh::endpoint::RecvStream,
    },
}

const STREAM_WARM_DEADLINE: Duration = Duration::from_secs(5);

struct MediaWarm {
    generation: String,
    first: Option<oneshot::Receiver<Result<WarmPayload, String>>>,
    worker: Option<tokio::task::AbortHandle>,
    attach_armed: bool,
}

struct TakenWarm {
    rx: oneshot::Receiver<Result<WarmPayload, String>>,
    worker: Option<tokio::task::AbortHandle>,
}

fn abort_warm_worker(worker: Option<tokio::task::AbortHandle>) {
    if let Some(worker) = worker {
        worker.abort();
    }
}

struct AbortOnDrop(Option<tokio::task::AbortHandle>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        abort_warm_worker(self.0.take());
    }
}

async fn warm_payload_with_deadline<F>(
    prepared_abort: Option<tokio::task::AbortHandle>,
    deadline: Duration,
    work: F,
) -> Result<WarmPayload, String>
where
    F: std::future::Future<Output = Result<WarmPayload, String>>,
{
    match tokio::time::timeout(deadline, work).await {
        Ok(result) => result,
        Err(_) => {
            abort_warm_worker(prepared_abort);
            tracing::info!("browser_media_warm warm_deadline");
            Err("Engine media warm deadline".into())
        }
    }
}

static MEDIA_WARMS: OnceLock<Mutex<BTreeMap<(PathBuf, String), MediaWarm>>> = OnceLock::new();

fn clone_client_handle(client: &super::CarrierClient) -> super::CarrierClient {
    super::CarrierClient {
        conn: client.conn.clone(),
        _endpoint: client._endpoint.clone(),
        owns_endpoint: false,
    }
}

fn media_admit_request(authority: &Value) -> Value {
    json!({"op":"browser_engine_media","page_id":authority["page_id"],
        "generation":authority["generation"],"binding_hash":authority["binding_hash"]})
}

pub(crate) async fn preconnect_engine(
    endpoint: &iroh::Endpoint,
    grant: &Value,
) -> Result<super::CarrierClient> {
    warm_media_client(endpoint, grant).await
}

async fn warm_media_client(
    endpoint: &iroh::Endpoint,
    grant: &Value,
) -> Result<super::CarrierClient> {
    let peer = grant["peer_did"]
        .as_str()
        .context("Engine peer missing")?
        .parse::<iroh::PublicKey>()?;
    let addresses = super::decode_ticket_endpoints(
        grant["connect_ticket"]
            .as_str()
            .context("Engine route missing")?,
    );
    for address in addresses.into_iter().filter(|a| a.id == peer) {
        if let Ok(client) = super::CarrierClient::connect_known_endpoint(endpoint, address, 5).await
        {
            return Ok(client);
        }
    }
    anyhow::bail!("Engine media route unavailable")
}

async fn admit_opened_stream(
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    client: super::CarrierClient,
    authority: &Value,
) -> Result<super::BrowserCarrierStream> {
    super::write_json_line(&mut send, &media_admit_request(authority)).await?;
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
    Ok(super::BrowserCarrierStream {
        send,
        recv,
        _client: client,
    })
}

async fn open_media_stream(
    client: super::CarrierClient,
    authority: &Value,
) -> Result<super::BrowserCarrierStream> {
    let (send, recv) = client.conn.open_bi().await?;
    admit_opened_stream(send, recv, client, authority).await
}

async fn open_pending_stream(client: super::CarrierClient) -> Result<WarmPayload> {
    let (send, recv) = client.conn.open_bi().await?;
    Ok(WarmPayload::Pending { client, send, recv })
}

async fn take_warm_stream(root: &Path, authority: &Value) -> Option<TakenWarm> {
    let page = authority["page_id"].as_str()?;
    let generation = authority["generation"].as_str()?;
    let mut warms = MEDIA_WARMS.get_or_init(Default::default).lock().await;
    let warm = warms.get_mut(&(root.to_owned(), page.into()))?;
    if warm.generation != generation || !warm.attach_armed {
        return None;
    }
    warm.attach_armed = false;
    Some(TakenWarm {
        rx: warm.first.take()?,
        worker: warm.worker.take(),
    })
}

fn spawn_stream_warm(
    tx: oneshot::Sender<Result<WarmPayload, String>>,
    prepared: Option<tokio::task::JoinHandle<Result<super::CarrierClient>>>,
    endpoint: iroh::Endpoint,
    grant: Value,
    authority: Value,
    admit: bool,
) -> tokio::task::AbortHandle {
    let prepared_abort = prepared.as_ref().map(|task| task.abort_handle());
    tokio::spawn(async move {
        let _cancel_prepared = AbortOnDrop(prepared_abort.clone());
        let result = warm_payload_with_deadline(prepared_abort, STREAM_WARM_DEADLINE, async {
            let client = match prepared {
                Some(task) => match task.await {
                    Ok(Ok(client)) => Ok(client),
                    Ok(Err(err)) => Err(err.to_string()),
                    Err(err) => Err(err.to_string()),
                },
                None => warm_media_client(&endpoint, &grant)
                    .await
                    .map_err(|err| err.to_string()),
            };
            match client {
                Ok(client) if admit => open_media_stream(client, &authority)
                    .await
                    .map(WarmPayload::Ready)
                    .map_err(|err| err.to_string()),
                Ok(client) => open_pending_stream(client)
                    .await
                    .map_err(|err| err.to_string()),
                Err(err) => Err(err),
            }
        })
        .await;
        let _ = tx.send(result);
    })
    .abort_handle()
}

async fn replace_media_warm(key: (PathBuf, String), warm: MediaWarm) {
    let previous = MEDIA_WARMS
        .get_or_init(Default::default)
        .lock()
        .await
        .insert(key, warm);
    if let Some(previous) = previous {
        abort_warm_worker(previous.worker);
    }
}

#[cfg(test)]
struct PreadmitCommitBarrier {
    prepared: Arc<tokio::sync::Barrier>,
    release: watch::Sender<bool>,
}

#[cfg(test)]
static PREADMIT_COMMIT_BARRIER: OnceLock<std::sync::Mutex<Option<Arc<PreadmitCommitBarrier>>>> =
    OnceLock::new();

#[cfg(test)]
async fn await_preadmit_commit_barrier() {
    let barrier = PREADMIT_COMMIT_BARRIER
        .get_or_init(Default::default)
        .lock()
        .ok()
        .and_then(|slot| slot.clone());
    let Some(barrier) = barrier else {
        return;
    };
    let mut released = barrier.release.subscribe();
    barrier.prepared.wait().await;
    while !*released.borrow() {
        if released.changed().await.is_err() {
            break;
        }
    }
}

async fn commit_prepared_media<T>(
    key: (PathBuf, String),
    generation: String,
    armed: bool,
    prepared: T,
    into_payload: impl FnOnce(T) -> Result<WarmPayload, String>,
) -> bool {
    #[cfg(test)]
    await_preadmit_commit_barrier().await;

    let ingresses = INGRESS.get_or_init(Default::default).lock().await;
    if ingresses.get(&key).is_none() {
        return false;
    }
    let (tx, rx) = oneshot::channel();
    let previous = {
        let mut warms = MEDIA_WARMS.get_or_init(Default::default).lock().await;
        warms.insert(
            key,
            MediaWarm {
                generation,
                first: Some(rx),
                worker: None,
                attach_armed: armed,
            },
        )
    };
    drop(ingresses);
    if let Some(previous) = previous {
        abort_warm_worker(previous.worker);
    }
    let _ = tx.send(into_payload(prepared));
    true
}

struct StreamWarmSource {
    endpoint: iroh::Endpoint,
    grant: Value,
    authority: Value,
}

async fn install_stream_warm(
    root: &Path,
    page: &str,
    generation: &str,
    prepared: Option<tokio::task::JoinHandle<Result<super::CarrierClient>>>,
    source: StreamWarmSource,
    attach_armed: bool,
    admit: bool,
) {
    let (tx, rx) = oneshot::channel();
    let worker = spawn_stream_warm(
        tx,
        prepared,
        source.endpoint,
        source.grant,
        source.authority,
        admit,
    );
    replace_media_warm(
        (root.to_owned(), page.into()),
        MediaWarm {
            generation: generation.into(),
            first: Some(rx),
            worker: Some(worker),
            attach_armed,
        },
    )
    .await;
}

async fn install_pending_spare(
    root: &Path,
    page: &str,
    generation: &str,
    client: &super::CarrierClient,
) {
    {
        let warms = MEDIA_WARMS.get_or_init(Default::default).lock().await;
        if let Some(warm) = warms.get(&(root.to_owned(), page.into())) {
            if warm.generation == generation && warm.first.is_some() {
                tracing::info!(page, "browser_media_warm spare_keep");
                return;
            }
        }
    }
    match open_pending_stream(clone_client_handle(client)).await {
        Ok(payload) => {
            let (tx, rx) = oneshot::channel();
            replace_media_warm(
                (root.to_owned(), page.into()),
                MediaWarm {
                    generation: generation.into(),
                    first: Some(rx),
                    worker: None,
                    attach_armed: false,
                },
            )
            .await;
            let _ = tx.send(Ok(payload));
            tracing::info!(page, "browser_media_warm spare_pending");
        }
        Err(error) => tracing::info!(page, error = %error, "browser_media_warm spare_failed"),
    }
}

pub(crate) async fn preadmit_media_stream(root: &Path, authority: &Value) {
    let Some(page) = authority["page_id"].as_str() else {
        return;
    };
    let Some(generation) = authority["generation"].as_str() else {
        return;
    };
    let key = (root.to_owned(), page.into());
    let mut warms = MEDIA_WARMS.get_or_init(Default::default).lock().await;
    let Some(warm) = warms.get_mut(&key) else {
        return;
    };
    if warm.generation != generation {
        return;
    }
    let Some(rx) = warm.first.take() else {
        return;
    };
    let armed = warm.attach_armed;
    let worker = warm.worker.take();
    drop(warms);
    let payload = match tokio::time::timeout(Duration::from_secs(2), rx).await {
        Ok(Ok(Ok(payload))) => payload,
        _ => {
            abort_warm_worker(worker);
            tracing::info!(page, "browser_media_warm preadmit_miss");
            return;
        }
    };
    let ready = match payload {
        WarmPayload::Ready(stream) => {
            tracing::info!(page, "browser_media_warm preadmit_already_ready");
            stream
        }
        WarmPayload::Pending { client, send, recv } => match tokio::time::timeout(
            STREAM_WARM_DEADLINE,
            admit_opened_stream(send, recv, client, authority),
        )
        .await
        {
            Ok(Ok(stream)) => {
                tracing::info!(page, "browser_media_warm preadmit_ready");
                stream
            }
            Ok(Err(error)) => {
                tracing::info!(page, error = %error, "browser_media_warm preadmit_failed");
                return;
            }
            Err(_) => {
                tracing::info!(page, "browser_media_warm preadmit_deadline");
                return;
            }
        },
    };
    if !commit_prepared_media(key, generation.into(), armed, ready, |stream| {
        Ok(WarmPayload::Ready(stream))
    })
    .await
    {
        tracing::info!(page, "browser_media_warm preadmit_retired");
    }
}

pub(crate) async fn rearm_media_stream(
    root: &Path,
    endpoint: &iroh::Endpoint,
    grant: &Value,
    authority: &Value,
) {
    let Some(page) = authority["page_id"].as_str() else {
        return;
    };
    let Some(generation) = authority["generation"].as_str() else {
        return;
    };
    let key = (root.to_owned(), page.into());
    let ingresses = INGRESS.get_or_init(Default::default).lock().await;
    let Some(ingress) = ingresses.get(&key) else {
        return;
    };
    if ingress.generation != generation || *ingress.cancel.borrow() || *ingress.closed.borrow() {
        return;
    }
    drop(ingresses);
    let mut warms = MEDIA_WARMS.get_or_init(Default::default).lock().await;
    if let Some(warm) = warms.get_mut(&key) {
        if warm.generation == generation && warm.first.is_some() {
            warm.attach_armed = true;
            tracing::info!(page, "browser_media_warm rearm_keep");
            return;
        }
    }
    drop(warms);
    tracing::info!(page, "browser_media_warm rearm_pending");
    install_stream_warm(
        root,
        page,
        generation,
        None,
        StreamWarmSource {
            endpoint: endpoint.clone(),
            grant: grant.clone(),
            authority: authority.clone(),
        },
        true,
        false,
    )
    .await;
}

async fn connect(
    root: &Path,
    endpoint: &iroh::Endpoint,
    grant: &Value,
    authority: &Value,
) -> Result<super::BrowserCarrierStream> {
    let stream = if let Some(taken) = take_warm_stream(root, authority).await {
        let TakenWarm { rx, worker } = taken;
        if let Ok(Ok(Ok(payload))) = tokio::time::timeout(Duration::from_secs(5), rx).await {
            match payload {
                WarmPayload::Ready(stream) => {
                    tracing::info!("browser_media_warm take_ready");
                    stream
                }
                WarmPayload::Pending { client, send, recv } => {
                    tracing::info!("browser_media_warm take_pending");
                    admit_opened_stream(send, recv, client, authority).await?
                }
            }
        } else {
            abort_warm_worker(worker);
            tracing::info!("browser_media_warm take_failed");
            tokio::time::timeout(Duration::from_secs(5), async {
                open_media_stream(warm_media_client(endpoint, grant).await?, authority).await
            })
            .await
            .context("Engine media connection deadline")??
        }
    } else {
        tracing::info!("browser_media_warm take_miss");
        tokio::time::timeout(Duration::from_secs(5), async {
            open_media_stream(warm_media_client(endpoint, grant).await?, authority).await
        })
        .await
        .context("Engine media connection deadline")??
    };
    if let (Some(page), Some(generation)) = (
        authority["page_id"].as_str(),
        authority["generation"].as_str(),
    ) {
        install_pending_spare(root, page, generation, &stream._client).await;
    }
    Ok(stream)
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

fn disable_media_nagle(stream: &tokio::net::TcpStream) -> Result<()> {
    stream
        .set_nodelay(true)
        .context("Runtime media TCP requires TCP_NODELAY")?;
    Ok(())
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
    prepared: Option<tokio::task::JoinHandle<Result<super::CarrierClient>>>,
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
        let descriptor = ingress.descriptor.clone();
        drop(ingresses);
        if let Some(task) = prepared {
            task.abort();
        }
        return Ok(descriptor);
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
    install_stream_warm(
        root,
        page,
        generation,
        prepared,
        StreamWarmSource {
            endpoint: endpoint.clone(),
            grant: grant.clone(),
            authority: authority.clone(),
        },
        true,
        true,
    )
    .await;
    ingresses.insert(
        key.clone(),
        Ingress {
            generation: generation.into(),
            descriptor: descriptor.clone(),
            cancel,
            closed,
        },
    );
    drop(ingresses);
    let root = root.to_owned();
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
                        let root=root.clone();let endpoint=endpoint.clone();let grant=grant.clone();let authority=authority.clone();
                        children.spawn(async move {
                            let _slot=slot;
                            disable_media_nagle(&socket)?;
                            let mut remote=connect(&root,&endpoint,&grant,&authority).await?;
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
    let previous = MEDIA_WARMS
        .get_or_init(Default::default)
        .lock()
        .await
        .remove(&key);
    if let Some(previous) = previous {
        abort_warm_worker(previous.worker);
    }
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
            let ingress=start_ingress(consumer_root.path(),&consumer.endpoint,&grant,&authority,None).await.unwrap();
            validate_ingress(&authority,&ingress,ingress["turn_url"].as_str().unwrap()).unwrap();
            assert_eq!(start_ingress(consumer_root.path(),&consumer.endpoint,&grant,&authority,None).await.unwrap(),ingress);
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
            assert!(connect(consumer_root.path(),&anonymous,&grant,&authority).await.is_err(),"routing information cannot confer Runtime authority");
            let mut stale=authority.clone();stale["generation"]=json!(runtime_hash("stale"));
            assert!(connect(consumer_root.path(),&consumer.endpoint,&grant,&stale).await.is_err());
            assert!(close_ingress(consumer_root.path(),"page:media-test","foreign").await.is_err());
            remove_target(engine_root.path(),"page:media-test",authority["generation"].as_str().unwrap()).await.unwrap();
            echo.await.unwrap();
            close_ingress(consumer_root.path(),"page:media-test",authority["generation"].as_str().unwrap()).await.unwrap();
            assert!(tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST,port)).await.is_err());
            assert!(connect(consumer_root.path(),&consumer.endpoint,&grant,&authority).await.is_err());
            anonymous.close().await;consumer.endpoint.close().await;engine.endpoint.close().await;
        }).await.unwrap();
    }

    #[tokio::test]
    async fn stream_warm_deadline_aborts_hung_prepare() {
        let prepared = tokio::spawn(std::future::pending::<()>());
        let abort = prepared.abort_handle();
        let started = tokio::time::Instant::now();
        let result = warm_payload_with_deadline(
            Some(abort.clone()),
            Duration::from_millis(50),
            std::future::pending(),
        )
        .await;
        match result {
            Err(error) => assert_eq!(error, "Engine media warm deadline"),
            Ok(_) => panic!("hung prepare must miss the warm deadline"),
        }
        assert!(started.elapsed() < Duration::from_millis(500));
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(abort.is_finished());
        drop(prepared);
    }

    #[tokio::test]
    async fn take_warm_timeout_aborts_owned_worker() {
        let root = tempfile::tempdir().unwrap();
        let authority = json!({"page_id":"page:warm-timeout","generation":"gen"});
        let (_tx, rx) = oneshot::channel::<Result<WarmPayload, String>>();
        let worker = tokio::spawn(std::future::pending::<()>());
        let abort = worker.abort_handle();
        MEDIA_WARMS
            .get_or_init(Default::default)
            .lock()
            .await
            .insert(
                (root.path().to_owned(), "page:warm-timeout".into()),
                MediaWarm {
                    generation: "gen".into(),
                    first: Some(rx),
                    worker: Some(abort.clone()),
                    attach_armed: true,
                },
            );
        let TakenWarm { rx, worker } = take_warm_stream(root.path(), &authority).await.unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(20), rx)
            .await
            .is_err());
        abort_warm_worker(worker);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(abort.is_finished());
    }

    #[tokio::test]
    async fn close_ingress_aborts_prepared_warm_worker() {
        let root = tempfile::tempdir().unwrap();
        let unused = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = unused.local_addr().unwrap().port();
        drop(unused);
        std::fs::create_dir_all(root.path().join("config")).unwrap();
        std::fs::write(
            root.path().join("config/browser-viewer-ingress.json"),
            serde_json::to_vec(&json!({
                "schema":"elastos.browser.viewer-ingress-config/v1",
                "listen_host":"127.0.0.1",
                "advertised_host":"127.0.0.1",
                "port_start":port,
                "port_end":port
            }))
            .unwrap(),
        )
        .unwrap();
        let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .bind()
            .await
            .unwrap();
        let grant = json!({"peer_did":endpoint.id().to_string(),"connect_ticket":"ticket"});
        let authority = json!({
            "page_id":"page:warm-close",
            "generation":"gen",
            "binding_hash":runtime_hash("binding")
        });
        let prepared = tokio::spawn(std::future::pending::<Result<super::super::CarrierClient>>());
        let abort = prepared.abort_handle();
        start_ingress(root.path(), &endpoint, &grant, &authority, Some(prepared))
            .await
            .unwrap();
        close_ingress(root.path(), "page:warm-close", "gen")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(abort.is_finished());
        endpoint.close().await;
    }

    struct ReleasedPreparedStream(Arc<AtomicBool>);

    impl Drop for ReleasedPreparedStream {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    struct PreadmitBarrierGuard(Arc<PreadmitCommitBarrier>);

    impl Drop for PreadmitBarrierGuard {
        fn drop(&mut self) {
            if let Ok(mut slot) = PREADMIT_COMMIT_BARRIER.get_or_init(Default::default).lock() {
                *slot = None;
            }
            let _ = self.0.release.send(true);
        }
    }

    fn install_preadmit_commit_barrier() -> PreadmitBarrierGuard {
        let (release, _) = watch::channel(false);
        let barrier = Arc::new(PreadmitCommitBarrier {
            prepared: Arc::new(tokio::sync::Barrier::new(2)),
            release,
        });
        *PREADMIT_COMMIT_BARRIER
            .get_or_init(Default::default)
            .lock()
            .expect("preadmit barrier") = Some(barrier.clone());
        PreadmitBarrierGuard(barrier)
    }

    async fn insert_fast_close_ingress(root: &Path, page: &str, generation: &str) {
        let (cancel, _) = watch::channel(false);
        let (_done, closed) = watch::channel(true);
        INGRESS.get_or_init(Default::default).lock().await.insert(
            (root.to_owned(), page.into()),
            Ingress {
                generation: generation.into(),
                descriptor: json!({}),
                cancel,
                closed,
            },
        );
    }

    #[tokio::test]
    async fn late_preadmit_after_close_releases_prepared_stream() {
        let root = tempfile::tempdir().unwrap();
        let page = "page:late-preadmit";
        let key = (root.path().to_owned(), page.into());
        insert_fast_close_ingress(root.path(), page, "gen").await;
        let barrier = install_preadmit_commit_barrier();
        let released = Arc::new(AtomicBool::new(false));
        let pending = tokio::spawn({
            let key = key.clone();
            let released = released.clone();
            async move {
                commit_prepared_media(
                    key,
                    "gen".into(),
                    true,
                    ReleasedPreparedStream(released),
                    |_| Err("test".into()),
                )
                .await
            }
        });
        barrier.0.prepared.wait().await;
        close_ingress(root.path(), page, "gen").await.unwrap();
        let _ = barrier.0.release.send(true);
        assert!(!pending.await.unwrap());
        assert!(released.load(Ordering::SeqCst));
        assert!(MEDIA_WARMS
            .get_or_init(Default::default)
            .lock()
            .await
            .get(&key)
            .is_none());
        assert!(INGRESS
            .get_or_init(Default::default)
            .lock()
            .await
            .get(&key)
            .is_none());
    }

    #[tokio::test]
    async fn preadmit_commit_retains_stream_while_ingress_owns_the_page() {
        let root = tempfile::tempdir().unwrap();
        let page = "page:preadmit-retain";
        let key = (root.path().to_owned(), page.into());
        insert_fast_close_ingress(root.path(), page, "gen").await;
        assert!(
            commit_prepared_media(key.clone(), "gen".into(), true, (), |_| Err("kept".into()))
                .await
        );
        assert!(MEDIA_WARMS
            .get_or_init(Default::default)
            .lock()
            .await
            .get(&key)
            .is_some());
        close_ingress(root.path(), page, "gen").await.unwrap();
        assert!(MEDIA_WARMS
            .get_or_init(Default::default)
            .lock()
            .await
            .get(&key)
            .is_none());
    }
}
