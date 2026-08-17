//! dkms-carrier-node — the NODE-SIDE Carrier bridge for the dKMS quorum transport.
//!
//! Bridges a Carrier (iroh) ALPN endpoint to the node's EXISTING `dkms-authority`
//! TCP listener. The runtime's `key-provider` dials this bridge by its stable
//! `did:key` (NAT-traversed over relays/hole-punch, zero VPN, zero enrollment), and
//! the bridge relays raw bytes to/from `dkms-authority` on the local mesh socket.
//!
//! Trust boundary: this relay is UNTRUSTED transport. The PQ-hybrid encrypted,
//! mutually-authenticated channel is established end-to-end between `key-provider`
//! and `dkms-authority` (descriptor-pinned identity), so the relay only ever sees
//! ciphertext. It performs NO authorization — fail-closed authorization stays in
//! `dkms-authority` (wallet-signed `AccessGrantV1` + live on-chain check).
//!
//! Config (env or argv):
//!   DKMS_CARRIER_NODE_TARGET  (argv[1]) the local dkms-authority address. default 10.66.66.1:9443
//!   DKMS_CARRIER_NODE_SEED    (argv[2]) 32-byte iroh identity seed file. default /var/lib/elastos/dkms/carrier.seed
//!   DKMS_CARRIER_NODE_BIND               optional fixed UDP bind addr (e.g. 0.0.0.0:11204)
//!   ELASTOS_RELAY_URL                    optional custom relay (default: iroh public relays)
//!
//! It prints its `did:key` to stdout (for descriptor/tooling capture) and serves forever.

use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use futures_lite::future::Boxed;
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, SecretKey};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

/// The Carrier ALPN this transport multiplexes. Must match `dkms-carrier-client`.
const DKMS_AUTHORITY_ALPN: &[u8] = b"elastos/dkms-authority/1";

/// Leveled logging is provided by the shared `elastos-logger` crate, configured once at the
/// top of `main()` from `DKMS_CARRIER_NODE_LOG_LEVEL` (falling back to `ELASTOS_LOG`, then
/// `info`). At `trace` (the crate's alias for legacy `debug`) the bridge traces every inbound
/// Carrier connection (the dialer's endpoint id), every relayed stream, and the byte counts each
/// stream moved. Secret-free by construction: the relay only ever sees ciphertext, and it logs
/// only peer ids, the target address, and byte tallies — never payload bytes.
const DEFAULT_TARGET: &str = "10.66.66.1:9443";
const DEFAULT_SEED_PATH: &str = "/var/lib/elastos/dkms/carrier.seed";
/// Bound how long we wait for a relay/online before we begin serving (best-effort:
/// discovery keeps working after this, but publishing our address sooner helps).
const ONLINE_TIMEOUT_SECS: u64 = 15;

#[derive(Clone, Debug)]
struct BridgeHandler {
    /// The local `dkms-authority` address we relay every stream to.
    target: Arc<String>,
}

#[allow(refining_impl_trait)]
impl ProtocolHandler for BridgeHandler {
    fn accept(&self, conn: iroh::endpoint::Connection) -> Boxed<std::result::Result<(), AcceptError>> {
        let target = self.target.clone();
        Box::pin(async move {
            handle_connection(conn, target)
                .await
                .map_err(|e| AcceptError::from(std::io::Error::other(e.to_string())))
        })
    }
}

async fn handle_connection(conn: iroh::endpoint::Connection, target: Arc<String>) -> Result<()> {
    // The dialer's Carrier endpoint id (its public key) — the "who is knocking" handle for the
    // trace. This authenticates the TRANSPORT peer only; end-to-end identity/authorization stays
    // between key-provider and dkms-authority.
    let remote = conn.remote_id().to_string();
    elastos_logger::log_trace!("inbound carrier connection from {remote}");
    // `key-provider` opens one bi-stream per session and runs init/hello/recover/shutdown
    // over it. A connection may carry more than one stream; relay each independently.
    let mut streams = 0u64;
    loop {
        let (send, recv) = match conn.accept_bi().await {
            Ok(pair) => pair,
            Err(_) => break, // peer closed the connection: stop accepting on it
        };
        streams += 1;
        elastos_logger::log_trace!(
            "[{remote}] stream {streams} opened — relaying to dkms-authority at {target}"
        );
        let target = target.clone();
        let remote = remote.clone();
        tokio::spawn(async move {
            match relay_stream(send, recv, &target).await {
                Ok((to_node, to_client)) => elastos_logger::log_trace!(
                    "[{remote}] stream closed ({to_node} B client->node, {to_client} B node->client)"
                ),
                Err(err) => {
                    elastos_logger::log_warn!(
                        "[{remote}] stream relay ended with error: {err:#}"
                    );
                }
            }
        });
    }
    elastos_logger::log_trace!(
        "carrier connection from {remote} closed ({streams} stream(s) served)"
    );
    Ok(())
}

/// Pump bytes between one Carrier bi-stream (the runtime client) and a fresh TCP
/// connection to the local `dkms-authority`. Fully transparent: the length-prefixed
/// framed protocol and the PQ channel inside it are opaque to this relay. Returns the
/// byte counts moved in each direction (client->node, node->client) for the trace.
async fn relay_stream(
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    target: &str,
) -> Result<(u64, u64)> {
    let tcp = TcpStream::connect(target)
        .await
        .with_context(|| format!("dkms-carrier-node: local dkms-authority at {target} is unreachable"))?;
    tcp.set_nodelay(true).ok();
    let (mut tcp_read, mut tcp_write) = tcp.into_split();

    // client -> node: relay request bytes, then half-close so the node sees EOF.
    let client_to_node = async {
        let copied = tokio::io::copy(&mut recv, &mut tcp_write).await.unwrap_or(0);
        let _ = tcp_write.shutdown().await;
        copied
    };
    // node -> client: relay response bytes, then finish the Carrier send stream (FIN).
    let node_to_client = async {
        let copied = tokio::io::copy(&mut tcp_read, &mut send).await.unwrap_or(0);
        let _ = send.finish();
        copied
    };
    let (to_node, to_client) = tokio::join!(client_to_node, node_to_client);
    Ok((to_node, to_client))
}

#[tokio::main]
async fn main() -> Result<()> {
    {
        use elastos_logger::{resolve_level, Level, LoggerConfig};
        // Legacy DKMS_CARRIER_NODE_LOG_LEVEL still works (docker-compose sets it); shared
        // ELASTOS_LOG is a fallback.
        let level = resolve_level(None, &["DKMS_CARRIER_NODE_LOG_LEVEL", "ELASTOS_LOG"], Level::Info);
        elastos_logger::init(LoggerConfig::new("dkms-carrier-node", level).build());
    }

    let mut args = std::env::args().skip(1);
    let target = args
        .next()
        .or_else(|| std::env::var("DKMS_CARRIER_NODE_TARGET").ok())
        .unwrap_or_else(|| DEFAULT_TARGET.to_string());
    let seed_path = args
        .next()
        .or_else(|| std::env::var("DKMS_CARRIER_NODE_SEED").ok())
        .unwrap_or_else(|| DEFAULT_SEED_PATH.to_string());

    // Validate the target resolves before we advertise an address we cannot serve.
    target
        .to_socket_addrs()
        .with_context(|| format!("dkms-carrier-node: target '{target}' does not resolve"))?
        .next()
        .ok_or_else(|| anyhow::anyhow!("dkms-carrier-node: target '{target}' resolves to nothing"))?;

    let seed = load_or_create_seed(&seed_path)?;
    let secret_key = SecretKey::from_bytes(&seed);
    let did = encode_did_key(&secret_key.public());

    let mut builder = Endpoint::builder().secret_key(secret_key);
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

    let endpoint = match std::env::var("DKMS_CARRIER_NODE_BIND") {
        Ok(bind) => {
            let addr = bind
                .parse::<std::net::SocketAddr>()
                .with_context(|| format!("dkms-carrier-node: DKMS_CARRIER_NODE_BIND '{bind}' is not a socket address"))?;
            builder
                .bind_addr(addr)
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .bind()
                .await
                .context("dkms-carrier-node: failed to bind iroh endpoint")?
        }
        Err(_) => builder
            .bind()
            .await
            .context("dkms-carrier-node: failed to bind iroh endpoint")?,
    };

    let handler = BridgeHandler {
        target: Arc::new(target.clone()),
    };
    let _router = Router::builder(endpoint.clone())
        .accept(DKMS_AUTHORITY_ALPN, handler)
        .spawn();

    // Best-effort: wait for a relay so our address publishes to the DHT promptly.
    let _ = tokio::time::timeout(Duration::from_secs(ONLINE_TIMEOUT_SECS), endpoint.online()).await;

    elastos_logger::log_info!(
        "bridging Carrier ALPN '{}' -> dkms-authority at {}",
        String::from_utf8_lossy(DKMS_AUTHORITY_ALPN),
        target
    );
    elastos_logger::log_info!("node DID {did}");
    // stdout carries ONLY the did so tooling can capture it for the descriptor.
    println!("{did}");

    // Serve forever.
    futures_lite::future::pending::<()>().await;
    Ok(())
}

/// Load a persisted 32-byte iroh identity seed, or create one (0600) so the node's
/// `did:key` is STABLE across restarts. This identity is purely for Carrier addressing;
/// it is independent of the node's PQ quorum identity (master.seed) which never moves.
fn load_or_create_seed(path: &str) -> Result<[u8; 32]> {
    let p = PathBuf::from(path);
    if p.exists() {
        let bytes = std::fs::read(&p)
            .with_context(|| format!("dkms-carrier-node: cannot read seed at {path}"))?;
        if bytes.len() != 32 {
            bail!("dkms-carrier-node: seed at {path} must be 32 bytes, got {}", bytes.len());
        }
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&bytes);
        Ok(seed)
    } else {
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed)
            .map_err(|e| anyhow::anyhow!("dkms-carrier-node: OS entropy unavailable: {e}"))?;
        if let Some(parent) = p.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }
        std::fs::write(&p, seed)
            .with_context(|| format!("dkms-carrier-node: cannot persist seed at {path}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
        }
        Ok(seed)
    }
}

/// Encode an iroh Ed25519 public key as a `did:key:z6Mk...` (multicodec 0xed01 + base58),
/// matching `elastos_identity::encode_did_key` so `did_to_public_key` round-trips it.
fn encode_did_key(public_key: &iroh::PublicKey) -> String {
    let mut bytes = Vec::with_capacity(34);
    bytes.extend_from_slice(&[0xed, 0x01]);
    bytes.extend_from_slice(public_key.as_bytes());
    format!("did:key:z{}", bs58::encode(&bytes).into_string())
}
