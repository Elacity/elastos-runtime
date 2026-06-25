//! dkms-carrier-client — the RUNTIME-SIDE Carrier sidecar for the dKMS quorum transport.
//!
//! Exposes a loopback TCP listener that `key-provider` connects to. Each connection
//! begins with a single newline-terminated `did:key` preamble naming the target quorum
//! node; the sidecar resolves that did to a Carrier (iroh) address, dials ALPN
//! `elastos/dkms-authority/1`, and relays raw bytes both ways for the rest of the
//! session.
//!
//! This is UNTRUSTED transport: the PQ-hybrid encrypted, mutually-authenticated channel
//! is established end-to-end between `key-provider` and the node, so this sidecar only
//! ever sees ciphertext. Keeping iroh here (not in `key-provider`) preserves the small
//! audited crypto binary and forces the mandatory-encrypted-channel (network=true) path.
//!
//! Performance (warm connection reuse): a cold Carrier dial (relay discovery + hole-punch +
//! first RTT) costs several seconds cross-continent. Since the quorum-authority is spawned PER
//! OPEN, doing that cold dial on every recover made an open take ~30s. This sidecar is launched
//! ONCE for the gateway's lifetime, so it keeps a WARM, persistent Carrier connection per node
//! and serves each recover as a fresh bi-stream on the already-established connection (QUIC
//! multiplexes streams cheaply). Connections are pre-warmed at startup and kept alive via QUIC
//! keep-alive, so even the first open lands on a hot path. This is what makes opens fast AND
//! scalable — one warm mesh serves unlimited opens.
//!
//! Config (env):
//!   DKMS_CARRIER_CLIENT_LISTEN  loopback listen addr. default 127.0.0.1:9444
//!   DKMS_CARRIER_WARM_DIDS      space/comma-separated node did:keys to pre-dial + keep hot
//!   ELASTOS_RELAY_URL           optional custom relay (default: iroh public relays)

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use iroh::endpoint::Connection;
use iroh::{Endpoint, SecretKey};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpSocket, TcpStream};
use tokio::sync::Mutex;

/// The Carrier ALPN this transport multiplexes. Must match `dkms-carrier-node`.
const DKMS_AUTHORITY_ALPN: &[u8] = b"elastos/dkms-authority/1";
const DEFAULT_LISTEN: &str = "127.0.0.1:9444";
/// Cap the preamble so a stray/hostile loopback client cannot make us buffer forever.
const PREAMBLE_MAX: usize = 512;
/// Bound the Carrier dial (relay/hole-punch + first RTT can be a couple seconds
/// cross-continent); fail closed past this rather than hang a recover.
const CONNECT_TIMEOUT_SECS: u64 = 15;
const ONLINE_TIMEOUT_SECS: u64 = 15;
/// QUIC keep-alive so warm connections survive between opens (must be < idle timeout).
const KEEP_ALIVE_SECS: u64 = 10;
const MAX_IDLE_SECS: u64 = 120;
/// How often to re-dial any node whose warm connection has dropped, keeping the mesh hot.
const REWARM_INTERVAL_SECS: u64 = 30;

/// Live Carrier connections keyed by the node's 32-byte public key. Reused across recovers
/// (one connection, many bi-streams) instead of dialing cold per request.
type ConnCache = Arc<Mutex<HashMap<[u8; 32], Connection>>>;

#[tokio::main]
async fn main() -> Result<()> {
    let listen = std::env::var("DKMS_CARRIER_CLIENT_LISTEN").unwrap_or_else(|_| DEFAULT_LISTEN.to_string());

    // Ephemeral iroh identity: the dKMS caller identity is the PQ caller seed at the
    // app layer, NOT this transport key, so a fresh per-process key is fine.
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| anyhow!("OS entropy unavailable: {e}"))?;
    let secret_key = SecretKey::from_bytes(&seed);

    // Keep-alive so a warm connection is held open between opens rather than idling out.
    let transport = iroh::endpoint::QuicTransportConfig::builder()
        .default_path_keep_alive_interval(Duration::from_secs(KEEP_ALIVE_SECS))
        .default_path_max_idle_timeout(Duration::from_secs(MAX_IDLE_SECS))
        .build();
    let mut builder = Endpoint::builder()
        .secret_key(secret_key)
        .transport_config(transport);
    if let Ok(relay_url) = std::env::var("ELASTOS_RELAY_URL") {
        if let Ok(url) = relay_url.parse::<url::Url>() {
            let config = iroh::RelayConfig {
                url: url.into(),
                quic: Some(Default::default()),
            };
            builder =
                builder.relay_mode(iroh::RelayMode::Custom(iroh::RelayMap::from_iter([config])));
        }
    }
    let endpoint = builder
        .bind()
        .await
        .context("dkms-carrier-client: failed to bind iroh endpoint")?;
    let _ = tokio::time::timeout(Duration::from_secs(ONLINE_TIMEOUT_SECS), endpoint.online()).await;

    // Shared warm-connection cache; pre-dial the quorum nodes so the FIRST open is hot, and
    // keep re-dialing any that drop so the mesh stays warm for unlimited subsequent opens.
    let cache: ConnCache = Arc::new(Mutex::new(HashMap::new()));
    let warm = warm_dids();
    if !warm.is_empty() {
        let endpoint = endpoint.clone();
        let cache = cache.clone();
        tokio::spawn(async move {
            loop {
                for did in &warm {
                    match get_or_dial(&endpoint, &cache, did).await {
                        Ok(_) => eprintln!("dkms-carrier-client: warm connection ready -> {did}"),
                        Err(err) => eprintln!("dkms-carrier-client: warm dial {did} failed: {err:#}"),
                    }
                }
                tokio::time::sleep(Duration::from_secs(REWARM_INTERVAL_SECS)).await;
            }
        });
    }

    // Bind with SO_REUSEADDR so a quick restart doesn't hit EADDRINUSE from sockets left in
    // TIME_WAIT by the previous run's connections.
    let bind_addr: std::net::SocketAddr = listen
        .parse()
        .with_context(|| format!("dkms-carrier-client: listen addr '{listen}' is not a socket address"))?;
    let socket = if bind_addr.is_ipv4() {
        TcpSocket::new_v4()
    } else {
        TcpSocket::new_v6()
    }
    .context("dkms-carrier-client: cannot create loopback socket")?;
    socket
        .set_reuseaddr(true)
        .context("dkms-carrier-client: cannot set SO_REUSEADDR")?;
    socket
        .bind(bind_addr)
        .with_context(|| format!("dkms-carrier-client: cannot bind loopback listener at {listen}"))?;
    let listener = socket
        .listen(1024)
        .with_context(|| format!("dkms-carrier-client: cannot listen at {listen}"))?;
    eprintln!(
        "dkms-carrier-client: loopback {listen} -> Carrier ALPN '{}' (this client {})",
        String::from_utf8_lossy(DKMS_AUTHORITY_ALPN),
        endpoint.id()
    );
    // stdout signals readiness for tooling/launch scripts.
    println!("ready {listen}");

    loop {
        let (sock, _peer) = listener
            .accept()
            .await
            .context("dkms-carrier-client: loopback accept failed")?;
        let endpoint = endpoint.clone();
        let cache = cache.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_loopback(sock, endpoint, cache).await {
                eprintln!("dkms-carrier-client: session ended with error: {err:#}");
            }
        });
    }
}

/// Return a LIVE Carrier connection to `target_did`, reusing the warm cached one when possible
/// (just a new bi-stream — no relay/hole-punch) and dialing cold only when there is none or the
/// cached one has closed. This is the optimization that turns a per-open cold dial into a hot
/// stream open.
async fn get_or_dial(endpoint: &Endpoint, cache: &ConnCache, target_did: &str) -> Result<Connection> {
    let public_key = did_to_public_key(target_did)
        .ok_or_else(|| anyhow!("dkms-carrier-client: invalid target did: {target_did}"))?;
    let key_bytes = *public_key.as_bytes();

    // Fast path: a warm, still-open connection — reuse it.
    {
        let cache = cache.lock().await;
        if let Some(conn) = cache.get(&key_bytes) {
            if conn.close_reason().is_none() {
                return Ok(conn.clone());
            }
        }
    }

    // Slow path: dial once (bounded) and cache for every subsequent recover.
    let addr = iroh::EndpointAddr::from(public_key);
    let conn = tokio::time::timeout(
        Duration::from_secs(CONNECT_TIMEOUT_SECS),
        endpoint.connect(addr, DKMS_AUTHORITY_ALPN),
    )
    .await
    .map_err(|_| anyhow!("dkms-carrier-client: Carrier connect to {target_did} timed out"))?
    .with_context(|| format!("dkms-carrier-client: Carrier connect to {target_did} failed"))?;

    let mut cache = cache.lock().await;
    // A concurrent task may have dialed the same node meanwhile — prefer the live existing one.
    if let Some(existing) = cache.get(&key_bytes) {
        if existing.close_reason().is_none() {
            return Ok(existing.clone());
        }
    }
    cache.insert(key_bytes, conn.clone());
    Ok(conn)
}

/// Node did:keys to pre-warm + keep hot (`DKMS_CARRIER_WARM_DIDS`, space/comma separated).
fn warm_dids() -> Vec<String> {
    std::env::var("DKMS_CARRIER_WARM_DIDS")
        .ok()
        .map(|s| {
            s.split([',', ' ', '\n', '\t'])
                .map(|t| t.strip_prefix("carrier:").unwrap_or(t).trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

async fn handle_loopback(mut sock: TcpStream, endpoint: Endpoint, cache: ConnCache) -> Result<()> {
    sock.set_nodelay(true).ok();
    let target_did = read_preamble_line(&mut sock).await?;
    let target_did = target_did
        .strip_prefix("carrier:")
        .unwrap_or(&target_did)
        .trim()
        .to_string();

    let conn = get_or_dial(&endpoint, &cache, &target_did).await?;
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .context("dkms-carrier-client: failed to open bi-stream to node")?;
    let (mut sock_read, mut sock_write) = sock.into_split();

    // loopback -> node: relay request bytes, then finish the Carrier send stream (FIN).
    let loopback_to_node = async {
        let _ = tokio::io::copy(&mut sock_read, &mut send).await;
        let _ = send.finish();
    };
    // node -> loopback: relay response bytes, then half-close the loopback socket.
    let node_to_loopback = async {
        let _ = tokio::io::copy(&mut recv, &mut sock_write).await;
        let _ = sock_write.shutdown().await;
    };
    tokio::join!(loopback_to_node, node_to_loopback);
    Ok(())
}

/// Read the newline-terminated `did:key` preamble byte-by-byte (so no request bytes
/// that follow are swallowed into a buffer we would then drop).
async fn read_preamble_line(sock: &mut TcpStream) -> Result<String> {
    let mut buf = Vec::with_capacity(96);
    let mut byte = [0u8; 1];
    loop {
        let n = sock
            .read(&mut byte)
            .await
            .context("dkms-carrier-client: error reading preamble")?;
        if n == 0 {
            bail!("dkms-carrier-client: loopback closed before preamble");
        }
        if byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0]);
        if buf.len() > PREAMBLE_MAX {
            bail!("dkms-carrier-client: preamble exceeded {PREAMBLE_MAX} bytes");
        }
    }
    String::from_utf8(buf).map_err(|e| anyhow!("dkms-carrier-client: preamble is not UTF-8: {e}"))
}

/// Parse a `did:key:z6Mk...` into an iroh PublicKey (multicodec 0xed01 + base58).
/// Mirrors `elastos_server::carrier::did_to_public_key`.
fn did_to_public_key(did: &str) -> Option<iroh::PublicKey> {
    let multibase = did.strip_prefix("did:key:z")?;
    let bytes = bs58::decode(multibase).into_vec().ok()?;
    if bytes.len() != 34 || bytes[0] != 0xed || bytes[1] != 0x01 {
        return None;
    }
    let key_bytes: [u8; 32] = bytes[2..34].try_into().ok()?;
    iroh::PublicKey::from_bytes(&key_bytes).ok()
}
