//! Built-in Carrier — P2P transport and gossip messaging.
//!
//! One iroh endpoint, two protocols:
//! - `elastos/carrier/1` — file serving (updates, capsule downloads)
//! - iroh-gossip ALPN — gossip messaging (chat, peer discovery)
//!
//! Replaces the separate peer-provider process. Same wire format.
//!
//! ## Trust boundary
//!
//! Carrier is a **transport-only** layer. It delivers gossip messages without
//! authenticating sender identity or verifying message signatures. The
//! `sender_id` and `signature` fields in GossipMessage are caller-controlled
//! and NOT validated by Carrier — they are the application layer's
//! responsibility. Capsules that need authenticated messages must implement
//! their own signing and verification (the chat capsule does this via
//! `signing_payload_hex` in app.rs).
//!
//! See `docs/CARRIER_TRUST_DECISION.md` for the rationale.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use iroh::address_lookup::memory::MemoryLookup;
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, SecretKey, Watcher};
// AutoDiscoveryGossip trait extends Gossip with DHT-based peer discovery.
// RecordPublisher publishes topic records to DHT so peers can find each other.
// DHT auto-discovery imports — used by native `elastos chat` path (main.rs).
// Provider gossip_join uses deterministic subscribe_with_opts for now.
#[allow(unused_imports)]
use distributed_topic_tracker::{AutoDiscoveryGossip, RecordPublisher, TopicId};
use iroh_gossip::net::Gossip;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use tracing::{debug, info};

use elastos_common::localhost::{
    publisher_artifacts_path, publisher_install_script_path, publisher_publish_state_path,
    publisher_release_head_path, publisher_release_manifest_path,
};
use elastos_runtime::provider::{Provider, ProviderError, ResourceRequest, ResourceResponse};

use crate::sources::TrustedSource;

const CARRIER_ALPN: &[u8] = b"elastos/carrier/1";
const CHAT_DISCOVERY_TOPIC_GENERAL: &str = "__elastos_internal/chat-presence-v1/#general";

/// Well-known secret for topic discovery. Any Carrier node with this secret
/// can discover peers on the same topic via DHT.
const TOPIC_DISCOVERY_SECRET: &[u8] = b"elastos-carrier-v1";

/// Hash a topic name to 32 bytes (SHA-256). Compatible with peer-provider.
pub fn topic_hash(name: &str) -> iroh_gossip::proto::TopicId {
    let hash = Sha256::digest(name.as_bytes());
    let mut id = [0u8; 32];
    id.copy_from_slice(&hash[..32]);
    iroh_gossip::proto::TopicId::from(id)
}

pub fn chat_discovery_topic(room: &str) -> String {
    if room == "#general" {
        CHAT_DISCOVERY_TOPIC_GENERAL.to_string()
    } else {
        format!("__elastos_internal/chat-presence-v1/{}", room)
    }
}

// ── Gossip message format (compatible with peer-provider) ────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipMessage {
    pub sender_id: String,
    pub sender_nick: String,
    pub content: String,
    pub ts: u64,
    pub nonce: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_session_id: Option<String>,
}

fn requested_gossip_ts(request: &serde_json::Value) -> u64 {
    request
        .get("ts")
        .and_then(|v| v.as_u64())
        .filter(|ts| *ts > 0)
        .unwrap_or_else(now_secs)
}

fn random_gossip_nonce() -> u64 {
    let mut buf = [0u8; 8];
    getrandom::getrandom(&mut buf).expect("OS entropy source unavailable for gossip nonce");
    u64::from_le_bytes(buf)
}

fn requested_gossip_nonce(request: &serde_json::Value) -> u64 {
    request
        .get("nonce")
        .and_then(|v| v.as_u64())
        .filter(|nonce| *nonce > 0)
        .unwrap_or_else(random_gossip_nonce)
}

struct TopicBuffer {
    messages: VecDeque<GossipMessage>,
    base_index: u64,
}

const MAX_BUFFER: usize = 10_000;
const MAX_TOPICS: usize = 100;
const MAX_CURSORS: usize = 1_000;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── Carrier Node ─────────────────────────────────────────────────

/// A running Carrier node with file serving and gossip.
/// Uses DHT-based peer discovery via `distributed-topic-tracker` — Carrier
/// works like a virtual LAN. Join a topic, discover peers automatically.
pub struct CarrierNode {
    pub endpoint: Endpoint,
    pub gossip: Gossip,
    _router: Router,
    pub gossip_state: Arc<Mutex<GossipState>>,
    pub memory_lookup: MemoryLookup,
}

pub struct GossipState {
    endpoint: Endpoint,
    gossip: Gossip,
    memory_lookup: MemoryLookup,
    signing_key: Option<ed25519_dalek::SigningKey>,
    joined_topics: std::collections::HashSet<String>,
    bootstrap_peers: Vec<iroh::EndpointId>,
    senders: HashMap<String, distributed_topic_tracker::GossipSender>,
    receiver_tasks: HashMap<String, tokio::task::JoinHandle<()>>,
    buffers: Arc<Mutex<HashMap<String, TopicBuffer>>>,
    cursors: Arc<Mutex<HashMap<(String, String), u64>>>,
    peers: Arc<Mutex<Vec<String>>>,
    topic_peers: Arc<Mutex<HashMap<String, HashSet<String>>>>,
    did: Option<String>,
}

impl GossipState {
    fn new(
        endpoint: Endpoint,
        gossip: Gossip,
        memory_lookup: MemoryLookup,
        signing_key: Option<ed25519_dalek::SigningKey>,
        did: Option<String>,
    ) -> Self {
        Self {
            endpoint,
            gossip,
            memory_lookup,
            signing_key,
            joined_topics: std::collections::HashSet::new(),
            bootstrap_peers: Vec::new(),
            senders: HashMap::new(),
            receiver_tasks: HashMap::new(),
            buffers: Arc::new(Mutex::new(HashMap::new())),
            cursors: Arc::new(Mutex::new(HashMap::new())),
            peers: Arc::new(Mutex::new(Vec::new())),
            topic_peers: Arc::new(Mutex::new(HashMap::new())),
            did,
        }
    }
}

fn parse_ticket_endpoints_or_error(
    ticket_str: &str,
) -> std::result::Result<Vec<iroh::EndpointAddr>, serde_json::Value> {
    let ticket_bytes = match data_encoding::BASE32_NOPAD
        .decode(ticket_str.to_ascii_uppercase().as_bytes())
    {
        Ok(b) => b,
        Err(e) => {
            return Err(
                serde_json::json!({"status":"error","code":"invalid_ticket","message": e.to_string()}),
            )
        }
    };
    let ticket: serde_json::Value = match serde_json::from_slice(&ticket_bytes) {
        Ok(t) => t,
        Err(e) => {
            return Err(
                serde_json::json!({"status":"error","code":"invalid_ticket","message": e.to_string()}),
            )
        }
    };

    let mut endpoints = Vec::new();
    if let Some(values) = ticket["endpoints"].as_array() {
        for ep_val in values {
            if let Ok(addr) = serde_json::from_value::<iroh::EndpointAddr>(ep_val.clone()) {
                endpoints.push(addr);
            }
        }
    }
    Ok(endpoints)
}

fn add_ticket_endpoints(
    memory_lookup: &MemoryLookup,
    bootstrap_peers: &mut Vec<iroh::EndpointId>,
    endpoints: &[iroh::EndpointAddr],
    mark_bootstrap: bool,
) -> Vec<String> {
    let mut added = Vec::new();
    for addr in endpoints {
        let endpoint_id = addr.id;
        let peer_id = endpoint_id.to_string();
        memory_lookup.add_endpoint_info(addr.clone());
        if mark_bootstrap && !bootstrap_peers.contains(&endpoint_id) {
            bootstrap_peers.push(endpoint_id);
        }
        added.push(peer_id.clone());
        if mark_bootstrap {
            info!(
                "carrier: added bootstrap peer {} to address book",
                &peer_id[..12]
            );
        } else {
            info!(
                "carrier: remembered peer {} for DHT rendezvous",
                &peer_id[..12]
            );
        }
    }
    added
}

async fn connect_ticket_endpoints(
    endpoint: &Endpoint,
    gossip: &Gossip,
    peers: Arc<Mutex<Vec<String>>>,
    endpoints: &[iroh::EndpointAddr],
) -> Vec<String> {
    let mut connected = Vec::new();
    for addr in endpoints {
        match endpoint.connect(addr.clone(), iroh_gossip::ALPN).await {
            Ok(conn) => {
                gossip.handle_connection(conn).await.ok();
                let peer_id = addr.id.to_string();
                connected.push(peer_id.clone());
                let mut known = peers.lock().await;
                if !known.contains(&peer_id) {
                    known.push(peer_id);
                }
            }
            Err(e) => {
                debug!("carrier: connect to {} failed: {}", addr.id, e);
            }
        }
    }
    connected
}

/// Parse a `did:key:z6Mk...` string into an iroh PublicKey.
///
/// DID encodes Ed25519 public key bytes (multicodec 0xed01 + base58).
/// iroh PublicKey is the same Ed25519 bytes, different encoding.
pub fn did_to_public_key(did: &str) -> Option<iroh::PublicKey> {
    let multibase = did.strip_prefix("did:key:z")?;
    let bytes = bs58::decode(multibase).into_vec().ok()?;
    if bytes.len() != 34 || bytes[0] != 0xed || bytes[1] != 0x01 {
        return None;
    }
    let key_bytes: [u8; 32] = bytes[2..34].try_into().ok()?;
    iroh::PublicKey::from_bytes(&key_bytes).ok()
}

pub fn decode_ticket_endpoints(ticket: &str) -> Vec<iroh::EndpointAddr> {
    let ticket_bytes =
        match data_encoding::BASE32_NOPAD.decode(ticket.to_ascii_uppercase().as_bytes()) {
            Ok(bytes) => bytes,
            Err(_) => return Vec::new(),
        };
    let ticket_json: serde_json::Value = match serde_json::from_slice(&ticket_bytes) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    ticket_json["endpoints"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|ep| serde_json::from_value::<iroh::EndpointAddr>(ep.clone()).ok())
        .collect()
}

fn relay_only_ticket_endpoints(source: &TrustedSource) -> Vec<iroh::EndpointAddr> {
    decode_ticket_endpoints(&source.connect_ticket)
        .into_iter()
        .filter_map(|endpoint| {
            let relay_addrs: Vec<_> = endpoint
                .addrs
                .iter()
                .filter(|addr| matches!(addr, iroh::TransportAddr::Relay(_)))
                .cloned()
                .collect();
            if relay_addrs.is_empty() {
                None
            } else {
                Some(iroh::EndpointAddr::from(endpoint.id).with_addrs(relay_addrs))
            }
        })
        .collect()
}

fn is_trusted_source_runtime(data_dir: &std::path::Path) -> bool {
    // Installed clients cache release metadata under the Publisher root for
    // update checks. That does NOT make them a trusted-source runtime.
    //
    // Only runtimes that actually serve publisher content should auto-join the
    // internal discovery topic at startup.
    publisher_install_script_path(data_dir).exists()
        || publisher_publish_state_path(data_dir).exists()
        || publisher_artifacts_path(data_dir).exists()
}

fn register_topic_state(
    state: &mut GossipState,
    topic_name: &str,
    sender: distributed_topic_tracker::GossipSender,
    receiver_task: tokio::task::JoinHandle<()>,
) {
    state.joined_topics.insert(topic_name.to_string());
    state.senders.insert(topic_name.to_string(), sender);
    if let Some(existing) = state
        .receiver_tasks
        .insert(topic_name.to_string(), receiver_task)
    {
        existing.abort();
    }
}

async fn join_gossip_topic(
    state: &mut GossipState,
    topic_name: &str,
    force_direct: bool,
) -> Result<()> {
    let bootstrap_peers = state.bootstrap_peers.clone();
    if force_direct {
        let topic = topic_hash(topic_name);
        if bootstrap_peers.is_empty() {
            info!(
                "carrier: gossip_join '{}' direct mode with 0 bootstrap peer(s)",
                topic_name
            );
        } else {
            info!(
                "carrier: gossip_join '{}' with {} bootstrap peer(s)",
                topic_name,
                bootstrap_peers.len()
            );
        }
        let joined = state
            .gossip
            .subscribe_with_opts(
                topic,
                iroh_gossip::api::JoinOptions::with_bootstrap(bootstrap_peers),
            )
            .await?;
        let (iroh_sender, iroh_receiver) = joined.split();
        let dtt_sender =
            distributed_topic_tracker::GossipSender::new(iroh_sender, state.gossip.clone());
        let dtt_receiver =
            distributed_topic_tracker::GossipReceiver::new(iroh_receiver, state.gossip.clone());
        state
            .buffers
            .lock()
            .await
            .entry(topic_name.to_string())
            .or_insert_with(|| TopicBuffer {
                messages: VecDeque::new(),
                base_index: 0,
            });
        let buffers = state.buffers.clone();
        let peers = state.peers.clone();
        let topic_peers = state.topic_peers.clone();
        let topic_key = topic_name.to_string();
        let receiver_task = tokio::spawn(async move {
            recv_loop(dtt_receiver, buffers, peers, topic_peers, topic_key).await;
        });
        register_topic_state(state, topic_name, dtt_sender, receiver_task);
        return Ok(());
    }

    let sk_bytes = match &state.signing_key {
        Some(k) => k.to_bytes(),
        None => anyhow::bail!("no signing key"),
    };
    let dtt_sk = ed25519_dalek3::SigningKey::from_bytes(&sk_bytes);
    let topic_id = TopicId::new(topic_name.to_string());
    let record_publisher = RecordPublisher::new(
        topic_id,
        dtt_sk.verifying_key(),
        dtt_sk,
        None,
        TOPIC_DISCOVERY_SECRET.to_vec(),
    );
    info!(
        "carrier: gossip_join '{}' via DHT auto-discovery ({} connected bootstrap peer(s))",
        topic_name,
        bootstrap_peers.len()
    );
    let topic = state
        .gossip
        .subscribe_and_join_with_auto_discovery_no_wait(record_publisher)
        .await?;
    let (sender, receiver) = topic.split().await?;
    state
        .buffers
        .lock()
        .await
        .entry(topic_name.to_string())
        .or_insert_with(|| TopicBuffer {
            messages: VecDeque::new(),
            base_index: 0,
        });
    let buffers = state.buffers.clone();
    let peers = state.peers.clone();
    let topic_peers = state.topic_peers.clone();
    let topic_key = topic_name.to_string();
    let receiver_task = tokio::spawn(async move {
        recv_loop(receiver, buffers, peers, topic_peers, topic_key).await;
    });
    register_topic_state(state, topic_name, sender, receiver_task);
    Ok(())
}

pub fn source_carrier_addrs(source: &TrustedSource) -> Vec<String> {
    let mut addrs = Vec::new();

    for endpoint in decode_ticket_endpoints(&source.connect_ticket) {
        for transport in &endpoint.addrs {
            if let iroh::TransportAddr::Ip(addr) = transport {
                let addr = addr.to_string();
                if !addrs.contains(&addr) {
                    addrs.push(addr);
                }
            }
        }
    }

    addrs
}

fn source_node_id(source: &TrustedSource) -> Option<String> {
    if !source.publisher_node_id.is_empty() {
        if source.publisher_node_id.starts_with("did:key:") {
            return did_to_public_key(&source.publisher_node_id).map(|pk| pk.to_string());
        }
        return Some(source.publisher_node_id.clone());
    }

    decode_ticket_endpoints(&source.connect_ticket)
        .into_iter()
        .next()
        .map(|endpoint| endpoint.id.to_string())
}

/// Start the Carrier node (endpoint + gossip + file serving).
///
/// Accepts an Ed25519 `SigningKey` (from DID derivation). The iroh `SecretKey`
/// is derived directly from the signing key bytes — so the node ID IS the DID.
pub async fn start_carrier_node(
    signing_key: &ed25519_dalek::SigningKey,
    did: &str,
    data_dir: PathBuf,
) -> Result<CarrierNode> {
    let secret_key = SecretKey::from_bytes(&signing_key.to_bytes());

    // Build endpoint. Uses iroh default relays unless ELASTOS_RELAY_URL is set.
    // Don't override address_lookup — the default includes pkarr for DHT discovery.
    let mut builder = Endpoint::builder().secret_key(secret_key.clone());
    if let Ok(relay_url) = std::env::var("ELASTOS_RELAY_URL") {
        if let Ok(url) = relay_url.parse::<url::Url>() {
            let config = iroh::RelayConfig {
                url: url.into(),
                quic: Some(Default::default()),
            };
            builder =
                builder.relay_mode(iroh::RelayMode::Custom(iroh::RelayMap::from_iter([config])));
            info!("carrier: using custom relay {}", relay_url);
        }
    }
    let endpoint = match builder
        .bind_addr("0.0.0.0:4433".parse::<std::net::SocketAddr>().unwrap())
        .map_err(|e| anyhow::anyhow!("{}", e))
    {
        Ok(builder) => match builder.bind().await {
            Ok(ep) => ep,
            Err(_) => Endpoint::builder()
                .secret_key(secret_key)
                .bind()
                .await
                .context("Failed to bind Carrier endpoint")?,
        },
        Err(_) => Endpoint::builder()
            .secret_key(secret_key)
            .bind()
            .await
            .context("Failed to bind Carrier endpoint")?,
    };

    // Add mDNS for LAN discovery (supplements the default pkarr/DNS)
    if let Ok(mdns) = iroh::address_lookup::MdnsAddressLookup::builder().build(endpoint.id()) {
        endpoint.address_lookup().add(mdns);
    }

    // Add MemoryLookup for explicit peer addresses (--connect tickets)
    let memory_lookup = MemoryLookup::new();
    endpoint.address_lookup().add(memory_lookup.clone());

    let gossip = Gossip::builder().spawn(endpoint.clone());

    let file_handler = FileHandler {
        data_dir: data_dir.clone(),
    };
    let router = Router::builder(endpoint.clone())
        .accept(CARRIER_ALPN, file_handler)
        .accept(iroh_gossip::ALPN, gossip.clone())
        .spawn();

    let gossip_state = Arc::new(Mutex::new(GossipState::new(
        endpoint.clone(),
        gossip.clone(),
        memory_lookup.clone(),
        Some(signing_key.clone()),
        Some(did.to_string()),
    )));

    let bound_port = endpoint
        .bound_sockets()
        .first()
        .map(|s| s.port())
        .unwrap_or(0);

    // Wait for relay connection (NAT traversal requires relay)
    match tokio::time::timeout(Duration::from_secs(10), endpoint.online()).await {
        Ok(()) => {
            let mut watcher = endpoint.watch_addr();
            let addr = watcher.get();
            let relay_count = addr
                .addrs
                .iter()
                .filter(|a| matches!(a, iroh::TransportAddr::Relay(_)))
                .count();
            let ip_count = addr
                .addrs
                .iter()
                .filter(|a| matches!(a, iroh::TransportAddr::Ip(_)))
                .count();
            info!(
                "carrier: online {} (port {}, {} relay, {} direct)",
                did, bound_port, relay_count, ip_count
            );
        }
        Err(_) => {
            info!("carrier: online {} (port {}, no relay)", did, bound_port);
        }
    }

    if is_trusted_source_runtime(&data_dir) {
        let mut state = gossip_state.lock().await;
        match join_gossip_topic(&mut state, CHAT_DISCOVERY_TOPIC_GENERAL, true).await {
            Ok(()) => {
                info!(
                    "carrier: trusted source discovery topic '{}' ready",
                    CHAT_DISCOVERY_TOPIC_GENERAL
                );
            }
            Err(err) => {
                tracing::warn!(
                    "carrier: failed to join trusted source discovery topic '{}': {}",
                    CHAT_DISCOVERY_TOPIC_GENERAL,
                    err
                );
            }
        }
    }

    Ok(CarrierNode {
        endpoint,
        gossip,
        _router: router,
        gossip_state,
        memory_lookup,
    })
}

// ── File serving protocol handler ────────────────────────────────

#[derive(Debug, Clone)]
struct FileHandler {
    data_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CarrierMessage {
    op: String,
    #[serde(default)]
    path: String,
    #[serde(flatten)]
    data: serde_json::Value,
}

#[allow(refining_impl_trait)]
impl ProtocolHandler for FileHandler {
    fn accept(
        &self,
        conn: iroh::endpoint::Connection,
    ) -> futures_lite::future::Boxed<std::result::Result<(), AcceptError>> {
        let data_dir = self.data_dir.clone();
        Box::pin(async move {
            handle_file_connection(conn, &data_dir)
                .await
                .map_err(|e| AcceptError::from(std::io::Error::other(e.to_string())))
        })
    }
}

async fn handle_file_connection(
    conn: iroh::endpoint::Connection,
    data_dir: &std::path::Path,
) -> Result<()> {
    loop {
        let (mut send, recv) = match conn.accept_bi().await {
            Ok(streams) => streams,
            Err(_) => break,
        };
        let data_dir = data_dir.to_path_buf();
        tokio::spawn(async move {
            if let Err(e) = handle_file_stream(&mut send, recv, &data_dir).await {
                debug!("carrier file stream error: {:#}", e);
            }
        });
    }
    Ok(())
}

async fn handle_file_stream(
    send: &mut iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
    data_dir: &std::path::Path,
) -> Result<()> {
    let mut reader = BufReader::new(recv);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let msg: CarrierMessage = serde_json::from_str(line.trim())?;

    match msg.op.as_str() {
        "release_head" => {
            // Serve release announcement from release-head.json + publish state.
            let head_path = publisher_release_head_path(data_dir);
            let state_path = publisher_publish_state_path(data_dir);
            if head_path.is_file() {
                let content = tokio::fs::read(&head_path).await?;
                let head: serde_json::Value = serde_json::from_slice(&content)?;
                // Extract fields the client expects in flat format
                let head_cid = head["payload"]["latest_release_cid"]
                    .as_str()
                    .unwrap_or_default();
                let version = head["payload"]["version"].as_str().unwrap_or_default();
                let channel = head["payload"]["channel"].as_str().unwrap_or("stable");
                let signer_did = head["signer_did"].as_str().unwrap_or_default();
                // Read release_cid from publish state if available
                let release_cid = if let Ok(state_bytes) = tokio::fs::read(&state_path).await {
                    serde_json::from_slice::<serde_json::Value>(&state_bytes)
                        .ok()
                        .and_then(|s| s["last_release_cid"].as_str().map(|s| s.to_string()))
                        .unwrap_or_default()
                } else {
                    String::new()
                };

                let response = serde_json::json!({
                    "ok": true,
                    "release": {
                        "head_cid": head_cid,
                        "release_cid": release_cid,
                        "version": version,
                        "channel": channel,
                        "signer_did": signer_did,
                    }
                });
                send_json(send, &response).await?;
            } else {
                send_json(
                    send,
                    &serde_json::json!({ "ok": false, "error": "no release published" }),
                )
                .await?;
            }
            info!("carrier: served release_head");
        }
        "file" => {
            let path = &msg.path;
            if path.is_empty() || path.contains("..") || path.starts_with('/') {
                send_json(
                    send,
                    &serde_json::json!({ "ok": false, "error": "invalid path" }),
                )
                .await?;
                return Ok(());
            }
            let file_path = if path == "release.json" || path == "release-head.json" {
                if path == "release.json" {
                    publisher_release_manifest_path(data_dir)
                } else {
                    publisher_release_head_path(data_dir)
                }
            } else {
                publisher_artifacts_path(data_dir).join(path)
            };
            if !file_path.is_file() {
                send_json(
                    send,
                    &serde_json::json!({ "ok": false, "error": "not found" }),
                )
                .await?;
                return Ok(());
            }
            let content = tokio::fs::read(&file_path).await?;
            let len = content.len() as u64;
            send.write_all(&len.to_be_bytes()).await?;
            send.write_all(&content).await?;
            send.finish()?;
            send.stopped().await.ok();
            info!("carrier: served file {} ({} bytes)", path, len);
        }
        _ => {
            send_json(
                send,
                &serde_json::json!({ "ok": false, "error": "unknown op" }),
            )
            .await?;
        }
    }
    Ok(())
}

async fn send_json(send: &mut iroh::endpoint::SendStream, value: &serde_json::Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    send.write_all(&bytes).await?;
    send.finish()?;
    send.stopped().await.ok();
    Ok(())
}

// ── Gossip Provider (implements Provider trait) ──────────────────

/// In-process gossip provider for `elastos://peer/*`.
/// Replaces the separate peer-provider subprocess.
pub struct CarrierGossipProvider {
    state: Arc<Mutex<GossipState>>,
}

impl CarrierGossipProvider {
    pub fn new(state: Arc<Mutex<GossipState>>) -> Self {
        Self { state }
    }
}

impl std::fmt::Debug for CarrierGossipProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CarrierGossipProvider").finish()
    }
}

#[async_trait::async_trait]
impl Provider for CarrierGossipProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "use send_raw for peer operations".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["peer"]
    }
    fn name(&self) -> &'static str {
        "carrier-gossip"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let op = request.get("op").and_then(|v| v.as_str()).unwrap_or("");
        let mut state = self.state.lock().await;

        match op {
            "init" => {
                let id = state
                    .did
                    .clone()
                    .unwrap_or_else(|| state.endpoint.id().to_string());
                Ok(serde_json::json!({"status": "ok", "data": {"node_id": id}}))
            }

            "gossip_join" => {
                let topic_name = request["topic"].as_str().unwrap_or_default();
                let force_direct = request
                    .get("mode")
                    .and_then(|value| value.as_str())
                    .map(|mode| mode.eq_ignore_ascii_case("direct"))
                    .unwrap_or(false);
                if topic_name.is_empty() {
                    return Ok(
                        serde_json::json!({"status":"error","code":"missing_topic","message":"topic required"}),
                    );
                }
                if state.joined_topics.contains(topic_name) {
                    return Ok(
                        serde_json::json!({"status":"error","code":"already_joined","message":"already joined"}),
                    );
                }
                if state.joined_topics.len() >= MAX_TOPICS {
                    return Ok(
                        serde_json::json!({"status":"error","code":"too_many_topics","message":"topic limit reached"}),
                    );
                }

                match join_gossip_topic(&mut state, topic_name, force_direct).await {
                    Ok(()) => Ok(serde_json::json!({"status":"ok","data":{"topic": topic_name}})),
                    Err(err) => Ok(
                        serde_json::json!({"status":"error","code":"join_failed","message": err.to_string()}),
                    ),
                }
            }

            "gossip_leave" => {
                let topic_name = request["topic"].as_str().unwrap_or_default();
                if topic_name.is_empty() {
                    return Ok(
                        serde_json::json!({"status":"error","code":"missing_topic","message":"topic required"}),
                    );
                }

                let removed_sender = state.senders.remove(topic_name);
                let removed_task = state.receiver_tasks.remove(topic_name);
                let was_joined = state.joined_topics.remove(topic_name);
                if removed_sender.is_none() && removed_task.is_none() && !was_joined {
                    return Ok(
                        serde_json::json!({"status":"error","code":"not_joined","message":"not joined"}),
                    );
                }
                if let Some(task) = removed_task {
                    task.abort();
                }

                state
                    .cursors
                    .lock()
                    .await
                    .retain(|(topic, _), _| topic != topic_name);
                state.buffers.lock().await.remove(topic_name);
                state.topic_peers.lock().await.remove(topic_name);

                Ok(serde_json::json!({"status":"ok","data":{"topic": topic_name}}))
            }

            "gossip_send" => {
                let topic_name = request["topic"].as_str().unwrap_or_default();
                let message = request["message"].as_str().unwrap_or_default();
                let sender_nick = request["sender"].as_str().unwrap_or("unknown");

                let sender = match state.senders.get(topic_name) {
                    Some(s) => s,
                    None => {
                        return Ok(
                            serde_json::json!({"status":"error","code":"not_joined","message":"not joined"}),
                        )
                    }
                };

                let default_id = state
                    .did
                    .clone()
                    .unwrap_or_else(|| state.endpoint.id().to_string());
                let msg = GossipMessage {
                    sender_id: request
                        .get("sender_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&default_id)
                        .to_string(),
                    sender_nick: sender_nick.to_string(),
                    content: message.to_string(),
                    ts: requested_gossip_ts(request),
                    nonce: requested_gossip_nonce(request),
                    signature: request
                        .get("signature")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    sender_session_id: request
                        .get("sender_session_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                };

                // Insert into local buffer so other local clients (native chat,
                // WASM bridge, microVM bridge) on the same runtime see the message.
                {
                    let mut bufs = state.buffers.lock().await;
                    if let Some(buf) = bufs.get_mut(topic_name) {
                        if buf.messages.len() >= MAX_BUFFER {
                            buf.messages.pop_front();
                            buf.base_index += 1;
                        }
                        buf.messages.push_back(msg.clone());
                    }
                }

                let bytes = serde_json::to_vec(&msg).unwrap_or_default();
                match sender.broadcast(bytes).await {
                    Ok(_) => Ok(serde_json::json!({"status":"ok"})),
                    Err(e) => {
                        // Broadcast may fail with 0 peers — message is still in
                        // the local buffer for same-runtime clients, but remote
                        // peers did NOT receive it. Report honestly.
                        tracing::debug!("gossip broadcast to external peers failed: {}", e);
                        Ok(serde_json::json!({"status":"ok","broadcast":"local_only"}))
                    }
                }
            }

            "gossip_recv" => {
                let topic_name = request["topic"].as_str().unwrap_or_default();
                let limit = request["limit"].as_u64().unwrap_or(50) as usize;
                let consumer_id = request
                    .get("consumer_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default")
                    .to_string();
                // Skip messages from this sender (prevents local loopback echo)
                let skip_sender_id = request
                    .get("skip_sender_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                let buffers = state.buffers.lock().await;
                let buf = match buffers.get(topic_name) {
                    Some(b) => b,
                    None => return Ok(serde_json::json!({"status":"ok","data":{"messages":[]}})),
                };

                let mut cursors = state.cursors.lock().await;
                // Evict cursors under memory pressure. HashMap iteration order
                // is arbitrary, so this is not LRU — it just prevents unbounded
                // growth. Active consumers will recreate their cursor on the
                // next gossip_recv call.
                if cursors.len() >= MAX_CURSORS {
                    let to_remove: Vec<_> =
                        cursors.keys().take(MAX_CURSORS / 10).cloned().collect();
                    for k in to_remove {
                        cursors.remove(&k);
                    }
                }
                let cursor_key = (topic_name.to_string(), consumer_id);
                let cursor = cursors.entry(cursor_key).or_insert(buf.base_index);

                let start = if *cursor >= buf.base_index {
                    (*cursor - buf.base_index) as usize
                } else {
                    0
                };

                let all: Vec<&GossipMessage> =
                    buf.messages.iter().skip(start).take(limit).collect();
                let count = all.len();
                let messages: Vec<&GossipMessage> = if skip_sender_id.is_empty() {
                    all
                } else {
                    all.into_iter()
                        .filter(|m| m.sender_id != skip_sender_id)
                        .collect()
                };

                *cursor = buf.base_index + start as u64 + count as u64;

                Ok(serde_json::json!({"status":"ok","data":{"messages": messages}}))
            }

            "get_ticket" => {
                // Use watch_addr() to include relay URLs (NAT traversal)
                let mut watcher = state.endpoint.watch_addr();
                let addr = watcher.get();
                let ticket_json = serde_json::json!({
                    "topic": null,
                    "endpoints": [addr],
                });
                let ticket_bytes = serde_json::to_vec(&ticket_json).unwrap_or_default();
                let mut ticket_str = data_encoding::BASE32_NOPAD.encode(&ticket_bytes);
                ticket_str.make_ascii_lowercase();

                Ok(serde_json::json!({"status":"ok","data":{
                    "ticket": ticket_str,
                    "node_id": state.endpoint.id().to_string(),
                }}))
            }

            "connect" => {
                let memory_lookup = state.memory_lookup.clone();
                let endpoints = match parse_ticket_endpoints_or_error(
                    request["ticket"].as_str().unwrap_or_default(),
                ) {
                    Ok(endpoints) => endpoints,
                    Err(err) => return Ok(err),
                };
                let added = add_ticket_endpoints(
                    &memory_lookup,
                    &mut state.bootstrap_peers,
                    &endpoints,
                    true,
                );
                let connected = connect_ticket_endpoints(
                    &state.endpoint,
                    &state.gossip,
                    state.peers.clone(),
                    &endpoints,
                )
                .await;
                Ok(
                    serde_json::json!({"status":"ok","data":{"added": added, "connected": connected}}),
                )
            }

            "remember_peer" => {
                let memory_lookup = state.memory_lookup.clone();
                let endpoints = match parse_ticket_endpoints_or_error(
                    request["ticket"].as_str().unwrap_or_default(),
                ) {
                    Ok(endpoints) => endpoints,
                    Err(err) => return Ok(err),
                };
                let added = add_ticket_endpoints(
                    &memory_lookup,
                    &mut state.bootstrap_peers,
                    &endpoints,
                    false,
                );
                Ok(serde_json::json!({"status":"ok","data":{"added": added}}))
            }

            "get_node_id" => {
                let id = state
                    .did
                    .clone()
                    .unwrap_or_else(|| state.endpoint.id().to_string());
                Ok(serde_json::json!({"status":"ok","data":{"node_id": id}}))
            }

            "list_peers" => {
                let peers = state.peers.lock().await.clone();
                Ok(serde_json::json!({"status":"ok","data":{"peers": peers}}))
            }

            "list_topics" => {
                let topics: Vec<&String> = state
                    .joined_topics
                    .iter()
                    .filter(|topic| !topic.starts_with("__elastos_internal/"))
                    .collect();
                Ok(serde_json::json!({"status":"ok","data":{"topics": topics}}))
            }

            "list_topic_peers" => {
                let topic_name = request["topic"].as_str().unwrap_or_default();
                if topic_name.is_empty() {
                    return Ok(
                        serde_json::json!({"status":"error","code":"missing_topic","message":"topic required"}),
                    );
                }
                let mut peers: Vec<String> = state
                    .topic_peers
                    .lock()
                    .await
                    .get(topic_name)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                peers.sort();
                Ok(serde_json::json!({"status":"ok","data":{"topic": topic_name, "peers": peers}}))
            }

            "gossip_join_peers" => {
                let topic_name = request["topic"].as_str().unwrap_or_default();
                if topic_name.is_empty() {
                    return Ok(
                        serde_json::json!({"status":"error","code":"missing_topic","message":"topic required"}),
                    );
                }
                let sender = match state.senders.get(topic_name) {
                    Some(s) => s,
                    None => {
                        return Ok(
                            serde_json::json!({"status":"error","code":"not_joined","message":"not joined"}),
                        )
                    }
                };
                let peer_ids: Vec<iroh::EndpointId> = request
                    .get("peers")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str())
                    .filter_map(|peer| peer.parse::<iroh::EndpointId>().ok())
                    .collect();
                if peer_ids.is_empty() {
                    return Ok(
                        serde_json::json!({"status":"error","code":"missing_peers","message":"peers required"}),
                    );
                }
                match sender.join_peers(peer_ids, None).await {
                    Ok(_) => Ok(serde_json::json!({"status":"ok","data":{"topic": topic_name}})),
                    Err(err) => Ok(
                        serde_json::json!({"status":"error","code":"join_failed","message": err.to_string()}),
                    ),
                }
            }

            _ => Ok(
                serde_json::json!({"status":"error","code":"unknown_op","message": format!("unknown: {}", op)}),
            ),
        }
    }
}

/// Background task: receive gossip messages and buffer them.
async fn handle_gossip_event(
    event: iroh_gossip::api::Event,
    buffers: &Arc<Mutex<HashMap<String, TopicBuffer>>>,
    peers: &Arc<Mutex<Vec<String>>>,
    topic_peers: &Arc<Mutex<HashMap<String, HashSet<String>>>>,
    topic: &str,
) {
    match event {
        iroh_gossip::api::Event::Received(msg) => {
            if let Ok(gossip_msg) = serde_json::from_slice::<GossipMessage>(&msg.content) {
                let mut bufs = buffers.lock().await;
                if let Some(buf) = bufs.get_mut(topic) {
                    if buf.messages.len() >= MAX_BUFFER {
                        buf.messages.pop_front();
                        buf.base_index += 1;
                    }
                    buf.messages.push_back(gossip_msg);
                }
            }
        }
        iroh_gossip::api::Event::NeighborUp(peer) => {
            let mut p = peers.lock().await;
            let peer_str = peer.to_string();
            if !p.contains(&peer_str) {
                p.push(peer_str.clone());
            }
            drop(p);
            topic_peers
                .lock()
                .await
                .entry(topic.to_string())
                .or_default()
                .insert(peer_str);
        }
        iroh_gossip::api::Event::NeighborDown(peer) => {
            let mut p = peers.lock().await;
            p.retain(|x| x != &peer.to_string());
            drop(p);
            if let Some(topic_set) = topic_peers.lock().await.get_mut(topic) {
                topic_set.remove(&peer.to_string());
            }
        }
        _ => {}
    }
}

async fn recv_loop(
    receiver: distributed_topic_tracker::GossipReceiver,
    buffers: Arc<Mutex<HashMap<String, TopicBuffer>>>,
    peers: Arc<Mutex<Vec<String>>>,
    topic_peers: Arc<Mutex<HashMap<String, HashSet<String>>>>,
    topic: String,
) {
    loop {
        match receiver.next().await {
            Some(Ok(event)) => {
                handle_gossip_event(event, &buffers, &peers, &topic_peers, &topic).await;
            }
            Some(Err(e)) => {
                tracing::warn!("carrier recv_loop error on '{}': {}", topic, e);
                // Continue — transient errors should not kill the receiver
            }
            None => {
                tracing::info!("carrier recv_loop ended for '{}' (stream closed)", topic);
                break;
            }
        }
    }
}

// ── Client (for updates) ─────────────────────────────────────────

pub struct CarrierClient {
    conn: iroh::endpoint::Connection,
    _endpoint: Endpoint,
}

impl CarrierClient {
    async fn connect_endpoint_addr(addr: iroh::EndpointAddr, timeout_secs: u64) -> Result<Self> {
        let mut rng_bytes = [0u8; 32];
        getrandom::getrandom(&mut rng_bytes).map_err(|e| anyhow::anyhow!("rng: {}", e))?;
        let secret_key = SecretKey::from_bytes(&rng_bytes);
        let endpoint = Endpoint::builder()
            .secret_key(secret_key)
            .bind()
            .await
            .context("Failed to bind")?;

        let conn = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            endpoint.connect(addr, CARRIER_ALPN),
        )
        .await
        .map_err(|_| anyhow::anyhow!("connect timed out"))?
        .context("connect failed")?;

        Ok(Self {
            conn,
            _endpoint: endpoint,
        })
    }

    pub async fn connect(
        publisher_node_id: &str,
        publisher_addrs: &[String],
        timeout_secs: u64,
    ) -> Result<Self> {
        let public_key: iroh::PublicKey = publisher_node_id.parse().context("Invalid node ID")?;
        let mut addr = iroh::EndpointAddr::from(public_key);
        for addr_str in publisher_addrs {
            if let Ok(sa) = addr_str.parse::<std::net::SocketAddr>() {
                addr = addr.with_addrs([iroh::TransportAddr::Ip(sa)]);
                break;
            }
            if let Some((host, port_str)) = addr_str.rsplit_once(':') {
                if let Ok(port) = port_str.parse::<u16>() {
                    if let Ok(mut resolved) =
                        tokio::net::lookup_host(format!("{}:{}", host, port)).await
                    {
                        if let Some(sa) = resolved.next() {
                            addr = addr.with_addrs([iroh::TransportAddr::Ip(sa)]);
                            break;
                        }
                    }
                }
            }
        }

        Self::connect_endpoint_addr(addr, timeout_secs).await
    }

    pub async fn connect_trusted_source(source: &TrustedSource, timeout_secs: u64) -> Result<Self> {
        let ticket_endpoints = decode_ticket_endpoints(&source.connect_ticket);
        let mut ticket_errors = Vec::new();
        for endpoint in ticket_endpoints {
            match Self::connect_endpoint_addr(endpoint.clone(), timeout_secs).await {
                Ok(client) => return Ok(client),
                Err(err) => ticket_errors.push(err.to_string()),
            }
        }

        let node_id = source_node_id(source)
            .ok_or_else(|| anyhow::anyhow!("trusted source has no usable Carrier node id"))?;
        let addrs = source_carrier_addrs(source);
        match Self::connect(&node_id, &addrs, timeout_secs).await {
            Ok(client) => Ok(client),
            Err(err) if !ticket_errors.is_empty() => Err(anyhow::anyhow!(
                "trusted source Carrier connection failed (ticket errors: {}; fallback error: {})",
                ticket_errors.join(" | "),
                err
            )),
            Err(err) => Err(err),
        }
    }

    pub async fn release_head(&self) -> Result<Option<serde_json::Value>> {
        let (mut send, recv) = self.conn.open_bi().await?;
        let msg = serde_json::json!({"op":"release_head","path":""});
        let mut bytes = serde_json::to_vec(&msg)?;
        bytes.push(b'\n');
        send.write_all(&bytes).await?;
        send.finish()?;
        let mut reader = BufReader::new(recv);
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let resp: serde_json::Value = serde_json::from_str(line.trim())?;
        if resp["ok"].as_bool() == Some(true) {
            Ok(Some(resp["release"].clone()))
        } else {
            Ok(None)
        }
    }

    pub async fn fetch_file(&self, path: &str) -> Result<Vec<u8>> {
        let (mut send, mut recv) = self.conn.open_bi().await?;
        let msg = serde_json::json!({"op":"file","path":path});
        let mut bytes = serde_json::to_vec(&msg)?;
        bytes.push(b'\n');
        send.write_all(&bytes).await?;
        send.finish()?;
        let mut len_buf = [0u8; 8];
        recv.read_exact(&mut len_buf).await?;
        let len = u64::from_be_bytes(len_buf) as usize;
        if len > 200 * 1024 * 1024 {
            let mut error_bytes = len_buf.to_vec();
            let tail = recv.read_to_end(16 * 1024).await?;
            error_bytes.extend_from_slice(&tail);
            if let Ok(text) = String::from_utf8(error_bytes) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(text.trim()) {
                    if json["ok"].as_bool() == Some(false) {
                        let msg = json["error"]
                            .as_str()
                            .unwrap_or("trusted source returned an unknown file error");
                        anyhow::bail!("trusted source file fetch failed for {}: {}", path, msg);
                    }
                }
            }
            anyhow::bail!(
                "invalid trusted source file reply for {} (declared {} bytes)",
                path,
                len
            );
        }
        let mut content = vec![0u8; len];
        recv.read_exact(&mut content).await?;
        Ok(content)
    }
}

async fn fetch_file_with_timeout(
    client: &CarrierClient,
    path: &str,
    timeout_secs: u64,
) -> Result<Vec<u8>> {
    tokio::time::timeout(Duration::from_secs(timeout_secs), client.fetch_file(path))
        .await
        .map_err(|_| anyhow::anyhow!("file fetch timed out after {}s", timeout_secs))?
}

pub async fn fetch_file_from_trusted_source(
    source: &TrustedSource,
    path: &str,
    connect_timeout_secs: u64,
    fetch_timeout_secs: u64,
) -> Result<Vec<u8>> {
    let mut errors = Vec::new();
    let ticket_endpoints = decode_ticket_endpoints(&source.connect_ticket);
    for (index, endpoint) in ticket_endpoints.into_iter().enumerate() {
        match CarrierClient::connect_endpoint_addr(endpoint, connect_timeout_secs).await {
            Ok(client) => match fetch_file_with_timeout(&client, path, fetch_timeout_secs).await {
                Ok(bytes) => return Ok(bytes),
                Err(err) => errors.push(format!("ticket[{index}] fetch failed: {err}")),
            },
            Err(err) => errors.push(format!("ticket[{index}] connect failed: {err}")),
        }
    }

    let relay_endpoints = relay_only_ticket_endpoints(source);
    for (index, endpoint) in relay_endpoints.into_iter().enumerate() {
        match CarrierClient::connect_endpoint_addr(endpoint, connect_timeout_secs).await {
            Ok(client) => match fetch_file_with_timeout(&client, path, fetch_timeout_secs).await {
                Ok(bytes) => return Ok(bytes),
                Err(err) => errors.push(format!("relay[{index}] fetch failed: {err}")),
            },
            Err(err) => errors.push(format!("relay[{index}] connect failed: {err}")),
        }
    }

    let node_id = source_node_id(source)
        .ok_or_else(|| anyhow::anyhow!("trusted source has no usable Carrier node id"))?;
    let addrs = source_carrier_addrs(source);
    match CarrierClient::connect(&node_id, &addrs, connect_timeout_secs).await {
        Ok(client) => match fetch_file_with_timeout(&client, path, fetch_timeout_secs).await {
            Ok(bytes) => Ok(bytes),
            Err(err) => {
                errors.push(format!("direct fetch failed: {err}"));
                Err(anyhow::anyhow!(
                    "trusted source Carrier fetch failed: {}",
                    errors.join(" | ")
                ))
            }
        },
        Err(err) => {
            errors.push(format!("direct connect failed: {err}"));
            Err(anyhow::anyhow!(
                "trusted source Carrier fetch failed: {}",
                errors.join(" | ")
            ))
        }
    }
}

pub async fn try_p2p_discovery(
    publisher_node_id: &str,
    publisher_addrs: &[String],
    timeout_secs: u64,
) -> Option<String> {
    let client = CarrierClient::connect(publisher_node_id, publisher_addrs, timeout_secs)
        .await
        .ok()?;
    let release = client.release_head().await.ok()??;
    release["head_cid"].as_str().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topic_hash_deterministic() {
        let h1 = topic_hash("#general");
        let h2 = topic_hash("#general");
        assert_eq!(h1, h2, "same topic name must produce same hash");

        let h3 = topic_hash("#other");
        assert_ne!(h1, h3, "different topics must produce different hashes");
    }

    #[test]
    fn test_gossip_message_serialization() {
        let msg = GossipMessage {
            sender_id: "did:key:z6MkTest".to_string(),
            sender_nick: "alice".to_string(),
            content: "hello world".to_string(),
            ts: 1700000000,
            nonce: 42,
            signature: None,
            sender_session_id: None,
        };
        let bytes = serde_json::to_vec(&msg).unwrap();
        let decoded: GossipMessage = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded.sender_id, "did:key:z6MkTest");
        assert_eq!(decoded.sender_nick, "alice");
        assert_eq!(decoded.content, "hello world");
        assert_eq!(decoded.ts, 1700000000);
        assert_eq!(decoded.nonce, 42);
        assert!(decoded.signature.is_none());
    }

    #[test]
    fn test_gossip_message_with_signature() {
        let msg = GossipMessage {
            sender_id: "did:key:z6MkTest".to_string(),
            sender_nick: "bob".to_string(),
            content: "signed msg".to_string(),
            ts: 1700000000,
            nonce: 1,
            signature: Some("deadbeef".to_string()),
            sender_session_id: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"signature\":\"deadbeef\""));

        let decoded: GossipMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.signature, Some("deadbeef".to_string()));
    }

    #[test]
    fn test_requested_gossip_ts_prefers_explicit_value() {
        let request = serde_json::json!({ "ts": 1_700_000_123u64 });
        assert_eq!(requested_gossip_ts(&request), 1_700_000_123u64);
    }

    #[test]
    fn test_requested_gossip_nonce_prefers_explicit_value() {
        let request = serde_json::json!({ "nonce": 42u64 });
        assert_eq!(requested_gossip_nonce(&request), 42u64);
    }

    #[test]
    fn test_ticket_encode_decode_roundtrip() {
        // Simulate the ticket format used by get_ticket / connect
        let ticket_json = serde_json::json!({
            "topic": null,
            "endpoints": [{
                "id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "addrs": []
            }],
        });
        let ticket_bytes = serde_json::to_vec(&ticket_json).unwrap();
        let mut encoded = data_encoding::BASE32_NOPAD.encode(&ticket_bytes);
        encoded.make_ascii_lowercase();

        // Decode
        let decoded_bytes = data_encoding::BASE32_NOPAD
            .decode(encoded.to_ascii_uppercase().as_bytes())
            .unwrap();
        let decoded: serde_json::Value = serde_json::from_slice(&decoded_bytes).unwrap();
        assert!(decoded["topic"].is_null());
        assert!(decoded["endpoints"].is_array());
    }

    fn encode_ticket_for(endpoint: iroh::EndpointAddr) -> String {
        let ticket_json = serde_json::json!({
            "topic": null,
            "endpoints": [endpoint],
        });
        let ticket_bytes = serde_json::to_vec(&ticket_json).unwrap();
        let mut encoded = data_encoding::BASE32_NOPAD.encode(&ticket_bytes);
        encoded.make_ascii_lowercase();
        encoded
    }

    #[test]
    fn test_remember_peer_does_not_mark_bootstrap_join_peers() {
        let secret = iroh::SecretKey::from_bytes(&[11u8; 32]);
        let endpoints = parse_ticket_endpoints_or_error(&encode_ticket_for(
            iroh::EndpointAddr::from(secret.public()),
        ))
        .expect("ticket should parse");
        let memory_lookup = MemoryLookup::new();
        let mut bootstrap_peers = Vec::new();

        let added = add_ticket_endpoints(&memory_lookup, &mut bootstrap_peers, &endpoints, false);

        assert_eq!(added.len(), 1);
        assert!(
            bootstrap_peers.is_empty(),
            "trusted-source rendezvous should not force direct bootstrap joins"
        );
    }

    #[test]
    fn test_connect_marks_bootstrap_join_peers() {
        let secret = iroh::SecretKey::from_bytes(&[12u8; 32]);
        let endpoint = iroh::EndpointAddr::from(secret.public());
        let expected_peer = endpoint.id;
        let endpoints = parse_ticket_endpoints_or_error(&encode_ticket_for(endpoint))
            .expect("ticket should parse");
        let memory_lookup = MemoryLookup::new();
        let mut bootstrap_peers = Vec::new();

        let added = add_ticket_endpoints(&memory_lookup, &mut bootstrap_peers, &endpoints, true);

        assert_eq!(added.len(), 1);
        assert_eq!(bootstrap_peers, vec![expected_peer]);
    }

    #[test]
    fn test_cached_release_metadata_does_not_mark_trusted_source_runtime() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(elastos_common::localhost::publisher_root_path(dir.path()))
            .unwrap();
        std::fs::write(publisher_release_head_path(dir.path()), b"{}").unwrap();
        std::fs::write(publisher_release_manifest_path(dir.path()), b"{}").unwrap();

        assert!(
            !is_trusted_source_runtime(dir.path()),
            "cached release metadata on an installed client must not enable trusted-source startup behavior"
        );
    }

    #[test]
    fn test_publisher_install_script_marks_trusted_source_runtime() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(elastos_common::localhost::publisher_root_path(dir.path()))
            .unwrap();
        std::fs::write(publisher_install_script_path(dir.path()), b"#!/bin/sh\n").unwrap();

        assert!(
            is_trusted_source_runtime(dir.path()),
            "actual publisher-serving state should enable trusted-source startup behavior"
        );
    }

    #[test]
    fn test_did_to_public_key_roundtrip() {
        // Derive a DID, convert back to PublicKey, verify it matches the signing key
        let (sk, did) = elastos_identity::derive_did(&[99u8; 32]);
        let pk = did_to_public_key(&did).expect("DID should parse to PublicKey");

        // iroh PublicKey bytes should equal ed25519 verifying key bytes
        let sk_iroh = iroh::SecretKey::from_bytes(&sk.to_bytes());
        assert_eq!(
            *pk,
            *sk_iroh.public(),
            "DID-derived PublicKey must match iroh SecretKey-derived PublicKey"
        );
    }

    /// Integration test: two carrier nodes exchange gossip messages.
    /// Requires unrestricted UDP socket binding — fails in sandboxed environments.
    /// Run explicitly with: cargo test -p elastos-server test_two_node_chat -- --ignored
    #[tokio::test]
    #[ignore = "requires network socket binding (fails in sandboxed environments)"]
    async fn test_two_node_chat() {
        // Spin up two carrier nodes with ephemeral DIDs, same topic, broadcast + receive.
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();

        let key1 = [1u8; 32];
        let key2 = [2u8; 32];
        let (sk1, did1) = elastos_identity::derive_did(&key1);
        let (sk2, did2) = elastos_identity::derive_did(&key2);

        assert_ne!(did1, did2, "different keys must produce different DIDs");

        let node1 = start_carrier_node(&sk1, &did1, dir1.path().to_path_buf())
            .await
            .unwrap();
        let node2 = start_carrier_node(&sk2, &did2, dir2.path().to_path_buf())
            .await
            .unwrap();

        // Add node1's address to node2's address book
        let mut w1 = node1.endpoint.watch_addr();
        let addr1 = w1.get();
        node2.memory_lookup.add_endpoint_info(addr1.clone());

        let topic = topic_hash("#test");

        // node1 subscribes (no peers yet)
        let topic1 = node1
            .gossip
            .subscribe_with_opts(topic, iroh_gossip::api::JoinOptions::with_bootstrap(vec![]))
            .await
            .unwrap();
        let (_sender1, mut receiver1) = topic1.split();

        // node2 joins with node1 as bootstrap peer
        let peer1_id: iroh::EndpointId = addr1.id;
        let topic2 = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            node2.gossip.subscribe_and_join(topic, vec![peer1_id]),
        )
        .await
        .unwrap()
        .unwrap();
        let (sender2, _receiver2) = topic2.split();

        // node2 broadcasts a message
        let msg = GossipMessage {
            sender_id: did2.clone(),
            sender_nick: "bob".to_string(),
            content: "hello from node2".to_string(),
            ts: 1700000000,
            nonce: 99,
            signature: None,
            sender_session_id: None,
        };
        let msg_bytes = serde_json::to_vec(&msg).unwrap();
        sender2.broadcast(msg_bytes.into()).await.unwrap();

        // node1 should receive it
        use futures_lite::StreamExt;
        let event = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while let Some(Ok(event)) = receiver1.next().await {
                if let iroh_gossip::api::Event::Received(received) = event {
                    return Some(received);
                }
            }
            None
        })
        .await
        .unwrap()
        .unwrap();

        let received: GossipMessage = serde_json::from_slice(&event.content).unwrap();
        assert_eq!(received.sender_nick, "bob");
        assert_eq!(received.content, "hello from node2");
        assert_eq!(received.sender_id, did2);

        // Cleanup
        node1.endpoint.close().await;
        node2.endpoint.close().await;
    }

    /// Prove that two consumers sharing the same gossip buffer see each
    /// other's messages. This is the core invariant for same-runtime
    /// native↔WASM chat interop: both capsules use the shared buffer
    /// with different consumer_ids.
    #[tokio::test]
    async fn test_shared_buffer_cross_consumer_delivery() {
        let buffers: Arc<Mutex<HashMap<String, TopicBuffer>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let cursors: Arc<Mutex<HashMap<(String, String), u64>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let topic = "#general".to_string();

        // Create buffer
        {
            let mut bufs = buffers.lock().await;
            bufs.insert(
                topic.clone(),
                TopicBuffer {
                    messages: VecDeque::new(),
                    base_index: 0,
                },
            );
        }

        // Consumer A (native chat) writes a message
        {
            let mut bufs = buffers.lock().await;
            let buf = bufs.get_mut(&topic).unwrap();
            buf.messages.push_back(GossipMessage {
                sender_id: "did:key:zAlice".to_string(),
                sender_nick: "alice".to_string(),
                content: "hello from native".to_string(),
                ts: 1000,
                nonce: 1,
                signature: Some("sig_alice".to_string()),
                sender_session_id: None,
            });
        }

        // Consumer B (WASM chat) reads with different consumer_id
        {
            let bufs = buffers.lock().await;
            let buf = bufs.get(&topic).unwrap();
            let mut curs = cursors.lock().await;
            let cursor = curs
                .entry((topic.clone(), "chat-wasm".to_string()))
                .or_insert(buf.base_index);
            let start = (*cursor - buf.base_index) as usize;
            let messages: Vec<&GossipMessage> = buf.messages.iter().skip(start).take(50).collect();

            assert_eq!(messages.len(), 1, "WASM consumer must see native's message");
            assert_eq!(messages[0].content, "hello from native");
            assert_eq!(messages[0].sender_nick, "alice");
            *cursor = buf.base_index + start as u64 + messages.len() as u64;
        }

        // Consumer A (native chat) reads — should also see its own message
        {
            let bufs = buffers.lock().await;
            let buf = bufs.get(&topic).unwrap();
            let mut curs = cursors.lock().await;
            let cursor = curs
                .entry((topic.clone(), "chat-native".to_string()))
                .or_insert(buf.base_index);
            let start = (*cursor - buf.base_index) as usize;
            let messages: Vec<&GossipMessage> = buf.messages.iter().skip(start).take(50).collect();

            assert_eq!(
                messages.len(),
                1,
                "native consumer must see its own message"
            );
            *cursor = buf.base_index + start as u64 + messages.len() as u64;
        }

        // Consumer B writes a reply
        {
            let mut bufs = buffers.lock().await;
            let buf = bufs.get_mut(&topic).unwrap();
            buf.messages.push_back(GossipMessage {
                sender_id: "did:key:zBob".to_string(),
                sender_nick: "bob".to_string(),
                content: "hello from wasm".to_string(),
                ts: 1001,
                nonce: 2,
                signature: Some("sig_bob".to_string()),
                sender_session_id: None,
            });
        }

        // Consumer A reads again — should see only the new message (cursor advanced)
        {
            let bufs = buffers.lock().await;
            let buf = bufs.get(&topic).unwrap();
            let mut curs = cursors.lock().await;
            let cursor = curs
                .entry((topic.clone(), "chat-native".to_string()))
                .or_insert(buf.base_index);
            let start = (*cursor - buf.base_index) as usize;
            let messages: Vec<&GossipMessage> = buf.messages.iter().skip(start).take(50).collect();

            assert_eq!(
                messages.len(),
                1,
                "native consumer must see WASM's reply (cursor tracks position)"
            );
            assert_eq!(messages[0].content, "hello from wasm");
            assert_eq!(messages[0].sender_nick, "bob");
        }
    }
}
