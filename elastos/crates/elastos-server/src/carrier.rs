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

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine as _;
use iroh::address_lookup::memory::MemoryLookup;
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, SecretKey, Watcher};
use iroh_mdns_address_lookup::MdnsAddressLookup;
// AutoDiscoveryGossip trait extends Gossip with DHT-based peer discovery.
// RecordPublisher publishes topic records to DHT so peers can find each other.
// DHT auto-discovery imports — used by native `elastos chat` path (main.rs).
// Provider gossip_join uses deterministic subscribe_with_opts for now.
#[allow(unused_imports)]
use distributed_topic_tracker::{
    AutoDiscoveryGossip, Config as TopicTrackerConfig, RecordPublisher, TimeoutConfig, TopicId,
};
use futures_lite::StreamExt;
use iroh_gossip::net::Gossip;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tracing::{debug, info};

use elastos_common::localhost::{
    publisher_artifacts_path, publisher_install_script_path, publisher_publish_state_path,
    publisher_release_head_path, publisher_release_manifest_path,
};
use elastos_runtime::provider::{
    Provider, ProviderCarrierInvoker, ProviderCarrierRoute, ProviderError, ProviderInvocation,
    ProviderInvocationTransport, ProviderRegistry, ProviderTransfer, ResourceRequest,
    ResourceResponse,
};

use crate::content::{ContentObjectManifest, CONTENT_OBJECT_MANIFEST_PATH};
use crate::operator_control::{OperatorHandler, OperatorRuntimeContext, OPERATOR_ALPN};
use crate::sources::TrustedSource;

const CARRIER_ALPN: &[u8] = b"elastos/carrier/1";
const BROWSER_CARRIER_STREAM_SCHEMA: &str = "elastos.browser.carrier-stream/v1";
const BROWSER_CARRIER_STREAM_ACK_MAX_BYTES: usize = 16 * 1024;
const CHAT_DISCOVERY_TOPIC_GENERAL: &str = "__elastos_internal/chat-presence-v1/#general";
const CONTENT_AVAILABILITY_ANNOUNCEMENT_SCHEMA: &str =
    "elastos.content.availability.announcement/v1";
const CONTENT_AVAILABILITY_ANNOUNCEMENT_DOMAIN: &str =
    "elastos.content.availability.announcement.v1";
const CONTENT_ADMISSION_DOMAIN: &str = "elastos.content.admission.v1";
const CONTENT_REPAIR_GRAPH_SCHEMA: &str = "elastos.content.repair-graph/v1";
const CONTENT_BLOCK_GRAPH_SCHEMA: &str = "elastos.content.block-graph/v1";
const CONTENT_FEDERATED_QUOTA_LEDGER_POLICY_SCHEMA: &str =
    "elastos.content.federated-quota-ledger-policy/v1";
const CONTENT_STORAGE_MARKET_ADMISSION_POLICY_SCHEMA: &str =
    "elastos.content.storage-market-admission-policy/v1";
const CONTENT_BLOCK_GRAPH_PROVIDER: &str = "content-block-graph-provider";
const CONTENT_BLOCK_GRAPH_TARGET: &str = "block-graph";
const CARRIER_PEER_REPUTATION_SCHEMA: &str = "elastos.carrier.peer-reputation/v1";
const CARRIER_PEER_ATTESTATION_EXCHANGE_POLICY_SCHEMA: &str =
    "elastos.carrier.peer-attestation-exchange-policy/v1";
const CARRIER_PEER_ATTESTATION_EXCHANGE_REQUEST_SCHEMA: &str =
    "elastos.carrier.peer-attestation.exchange-request/v1";
const CARRIER_PEER_ATTESTATION_EXCHANGE_REQUEST_DOMAIN: &str =
    "elastos.carrier.peer-attestation.exchange-request.v1";
const CARRIER_PEER_ATTESTATION_EXCHANGE_RECEIPT_SCHEMA: &str =
    "elastos.carrier.peer-attestation.exchange-receipt/v1";
const CARRIER_PEER_ATTESTATION_EXCHANGE_RECEIPT_DOMAIN: &str =
    "elastos.carrier.peer-attestation.exchange-receipt.v1";
const MAX_CARRIER_REPLICATION_CANDIDATES: usize = 8;
const MAX_CARRIER_AVAILABILITY_TICKET_LEN: usize = 8192;
const MAX_CARRIER_AVAILABILITY_ENDPOINT_ID_LEN: usize = 256;
const MAX_CARRIER_OBJECT_IMPORT_FILES: usize = 512;
const MAX_CARRIER_OBJECT_IMPORT_BYTES: usize = 64 * 1024 * 1024;
const MAX_REMOTE_RECEIPT_REPLICA_SUMMARY_ROWS: usize = 5;
const GOSSIP_SEND_TIMEOUT: Duration = Duration::from_millis(1_500);
const GOSSIP_JOIN_PEERS_TIMEOUT: Duration = Duration::from_millis(1_500);
const GOSSIP_JOIN_EXACT_MAX_PEERS: usize = 16;
const GOSSIP_CARRIER_PUSH_MAX_TOPIC_LEN: usize = 256;
// Maximum complete JSON GossipMessage admitted to the iroh-gossip wire.
const GOSSIP_WIRE_MESSAGE_MAX_BYTES: usize = 192 * 1024;
const GOSSIP_TOPIC_BUFFER_MAX_BYTES: usize = 12 * 1024 * 1024;
const GOSSIP_CURSOR_MAX_CONSUMER_ID_BYTES: usize = 128;
const GOSSIP_PEEK_MAX_MESSAGES: usize = 256;
// Maximum complete encoded provider response returned by one gossip_peek.
const GOSSIP_PEEK_MAX_BATCH_BYTES: usize = GOSSIP_TOPIC_BUFFER_MAX_BYTES;

/// Well-known secret for topic discovery. Any Carrier node with this secret
/// can discover peers on the same topic via DHT.
const TOPIC_DISCOVERY_SECRET: &[u8] = b"elastos-carrier-v1";

/// Hash a topic name to the same 32-byte topic ID used by distributed-topic-tracker.
///
/// Direct bootstrap and DHT auto-discovery must resolve the same logical topic
/// string to the same iroh-gossip topic, otherwise a direct fallback can never
/// meet an auto-discovered topic mesh.
pub fn topic_hash(name: &str) -> iroh_gossip::proto::TopicId {
    let hash = Sha512::digest(name.as_bytes());
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

#[derive(Default)]
struct TopicBuffer {
    messages: VecDeque<GossipMessage>,
    base_index: u64,
    retained_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GossipCursorRange {
    cursor: u64,
    next_cursor: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OutstandingGossipPeek {
    range: GossipCursorRange,
    limit: usize,
}

#[derive(Clone, Copy, Debug)]
struct GossipConsumerCursor {
    position: u64,
    outstanding_peek: Option<OutstandingGossipPeek>,
    last_acknowledged: Option<GossipCursorRange>,
}

impl GossipConsumerCursor {
    fn at(position: u64) -> Self {
        Self {
            position,
            outstanding_peek: None,
            last_acknowledged: None,
        }
    }

    fn normalize_to(&mut self, base_index: u64) {
        if self.position < base_index {
            self.position = base_index;
            self.outstanding_peek = None;
        }
    }
}

const MAX_BUFFER: usize = 10_000;
const MAX_TOPICS: usize = 100;
const MAX_CURSORS: usize = 1_000;

fn same_gossip_delivery(left: &GossipMessage, right: &GossipMessage) -> bool {
    left.sender_id == right.sender_id
        && left.content == right.content
        && left.ts == right.ts
        && left.signature == right.signature
}

fn serialize_gossip_wire_message(message: &GossipMessage) -> std::result::Result<Vec<u8>, usize> {
    let encoded =
        serde_json::to_vec(message).expect("GossipMessage JSON serialization cannot fail");
    if encoded.len() > GOSSIP_WIRE_MESSAGE_MAX_BYTES {
        return Err(encoded.len());
    }
    Ok(encoded)
}

fn gossip_message_encoded_len(message: &GossipMessage) -> usize {
    serde_json::to_vec(message)
        .expect("GossipMessage JSON serialization cannot fail")
        .len()
}

fn pop_gossip_buffer_front(buffer: &mut TopicBuffer) -> Option<GossipMessage> {
    let message = buffer.messages.pop_front()?;
    buffer.retained_bytes = buffer
        .retained_bytes
        .checked_sub(gossip_message_encoded_len(&message))
        .expect("gossip buffer byte accounting must remain exact");
    buffer.base_index += 1;
    Some(message)
}

fn push_gossip_buffer_message(buffer: &mut TopicBuffer, message: GossipMessage) -> bool {
    if buffer
        .messages
        .iter()
        .rev()
        .any(|existing| same_gossip_delivery(existing, &message))
    {
        return false;
    }
    let encoded_len = serialize_gossip_wire_message(&message)
        .expect("only wire-bounded gossip messages may enter a topic buffer")
        .len();
    while buffer.messages.len() >= MAX_BUFFER
        || buffer
            .retained_bytes
            .checked_add(encoded_len)
            .is_none_or(|bytes| bytes > GOSSIP_TOPIC_BUFFER_MAX_BYTES)
    {
        pop_gossip_buffer_front(buffer)
            .expect("one wire-bounded message must fit in the gossip buffer");
    }
    buffer.retained_bytes += encoded_len;
    buffer.messages.push_back(message);
    true
}

fn spawn_carrier_gossip(endpoint: &Endpoint) -> Gossip {
    Gossip::builder()
        .max_message_size(GOSSIP_WIRE_MESSAGE_MAX_BYTES)
        .spawn(endpoint.clone())
}

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

impl CarrierNode {
    pub(crate) async fn shutdown(self) {
        let tasks = {
            let mut state = self.gossip_state.lock().await;
            let tasks = state
                .receiver_tasks
                .drain()
                .map(|(_, task)| task)
                .collect::<Vec<_>>();
            state.senders.clear();
            state.joined_topics.clear();
            tasks
        };
        for task in tasks {
            task.abort();
            let _ = task.await;
        }
        if let Err(err) = self.gossip.shutdown().await {
            tracing::warn!(error = %err, "carrier gossip shutdown failed during runtime shutdown");
        }
        self.endpoint.close().await;
    }
}

pub struct CarrierRuntimeService {
    node: Option<CarrierNode>,
    #[cfg(test)]
    stopped: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl CarrierRuntimeService {
    pub fn new(node: CarrierNode) -> Self {
        Self {
            node: Some(node),
            #[cfg(test)]
            stopped: None,
        }
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        if let Some(node) = self.node.take() {
            node.shutdown().await;
        }
        #[cfg(test)]
        if let Some(stopped) = self.stopped.as_ref() {
            stopped.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn test_pending_service(stopped: Arc<std::sync::atomic::AtomicBool>) -> Self {
        Self {
            node: None,
            stopped: Some(stopped),
        }
    }
}

pub struct GossipState {
    endpoint: Endpoint,
    gossip: Gossip,
    memory_lookup: MemoryLookup,
    signing_key: Option<ed25519_dalek::SigningKey>,
    joined_topics: std::collections::HashSet<String>,
    bootstrap_peers: Vec<iroh::EndpointId>,
    #[cfg(test)]
    last_direct_join_peers: Option<Vec<iroh::EndpointId>>,
    senders: HashMap<String, distributed_topic_tracker::GossipSender>,
    receiver_tasks: HashMap<String, tokio::task::JoinHandle<()>>,
    buffers: Arc<Mutex<HashMap<String, TopicBuffer>>>,
    cursors: Arc<Mutex<HashMap<(String, String), GossipConsumerCursor>>>,
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
            #[cfg(test)]
            last_direct_join_peers: None,
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

impl Drop for GossipState {
    fn drop(&mut self) {
        for task in self.receiver_tasks.values() {
            task.abort();
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
            debug!(
                "carrier: remembered peer {} for DHT rendezvous",
                &peer_id[..12]
            );
        }
    }
    added
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GossipJoinExactRequest {
    op: String,
    topic: String,
    peers: Vec<String>,
}

fn parse_gossip_join_exact_request(
    request: &serde_json::Value,
) -> std::result::Result<(String, Vec<iroh::EndpointId>), serde_json::Value> {
    let request: GossipJoinExactRequest =
        serde_json::from_value(request.clone()).map_err(|err| {
            serde_json::json!({
                "status": "error",
                "code": "invalid_request",
                "message": err.to_string()
            })
        })?;
    if request.op != "gossip_join_exact" || request.topic.is_empty() {
        return Err(serde_json::json!({
            "status": "error",
            "code": "invalid_request",
            "message": "gossip_join_exact requires its exact operation and a non-empty topic"
        }));
    }
    if request.peers.len() > GOSSIP_JOIN_EXACT_MAX_PEERS {
        return Err(serde_json::json!({
            "status": "error",
            "code": "too_many_peers",
            "message": format!("peers must contain at most {GOSSIP_JOIN_EXACT_MAX_PEERS} entries")
        }));
    }

    let mut parsed = Vec::with_capacity(request.peers.len());
    for peer in request.peers {
        let Ok(endpoint_id) = peer.parse::<iroh::EndpointId>() else {
            return Err(serde_json::json!({
                "status": "error",
                "code": "invalid_peer",
                "message": "every peer must be a canonical EndpointId string"
            }));
        };
        if endpoint_id.to_string() != peer {
            return Err(serde_json::json!({
                "status": "error",
                "code": "noncanonical_peer",
                "message": "every peer must use the canonical EndpointId representation"
            }));
        }
        if parsed.contains(&endpoint_id) {
            return Err(serde_json::json!({
                "status": "error",
                "code": "duplicate_peer",
                "message": "peers must not contain duplicates"
            }));
        }
        parsed.push(endpoint_id);
    }
    Ok((request.topic, parsed))
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

fn public_key_to_did(public_key: &iroh::PublicKey) -> String {
    let mut bytes = Vec::with_capacity(34);
    bytes.extend_from_slice(&[0xed, 0x01]);
    bytes.extend_from_slice(public_key.as_bytes());
    format!("did:key:z{}", bs58::encode(bytes).into_string())
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

fn carrier_mdns_enabled() -> bool {
    std::env::var("ELASTOS_CARRIER_MDNS")
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no"
            )
        })
        .unwrap_or(true)
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

async fn register_topic_state(
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
        let _ = existing.await;
    }
}

async fn join_gossip_topic_direct(
    state: &mut GossipState,
    topic_name: &str,
    bootstrap_peers: Vec<iroh::EndpointId>,
) -> Result<()> {
    #[cfg(test)]
    {
        state.last_direct_join_peers = Some(bootstrap_peers.clone());
    }
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
        distributed_topic_tracker::GossipSender::new(iroh_sender, TimeoutConfig::default());
    state
        .buffers
        .lock()
        .await
        .entry(topic_name.to_string())
        .or_default();
    let buffers = state.buffers.clone();
    let peers = state.peers.clone();
    let topic_peers = state.topic_peers.clone();
    let topic_key = topic_name.to_string();
    let receiver_task = tokio::spawn(async move {
        recv_loop(
            CarrierGossipReceiver::Direct(iroh_receiver),
            buffers,
            peers,
            topic_peers,
            topic_key,
        )
        .await;
    });
    register_topic_state(state, topic_name, dtt_sender, receiver_task).await;
    Ok(())
}

async fn join_gossip_topic(
    state: &mut GossipState,
    topic_name: &str,
    force_direct: bool,
) -> Result<()> {
    let bootstrap_peers = state.bootstrap_peers.clone();
    if force_direct {
        return join_gossip_topic_direct(state, topic_name, bootstrap_peers).await;
    }

    let sk_bytes = match &state.signing_key {
        Some(k) => k.to_bytes(),
        None => anyhow::bail!("no signing key"),
    };
    let dtt_sk = ed25519_dalek3::SigningKey::from_bytes(&sk_bytes);
    let topic_id = TopicId::new(topic_name.to_string());
    let record_publisher = RecordPublisher::new(
        topic_id,
        dtt_sk,
        None,
        TOPIC_DISCOVERY_SECRET.to_vec(),
        TopicTrackerConfig::default(),
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
        .or_default();
    let buffers = state.buffers.clone();
    let peers = state.peers.clone();
    let topic_peers = state.topic_peers.clone();
    let topic_key = topic_name.to_string();
    let receiver_task = tokio::spawn(async move {
        recv_loop(
            CarrierGossipReceiver::Discovered(receiver),
            buffers,
            peers,
            topic_peers,
            topic_key,
        )
        .await;
    });
    register_topic_state(state, topic_name, sender, receiver_task).await;
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
    start_carrier_node_with_registry(signing_key, did, data_dir, None).await
}

pub async fn start_carrier_node_with_registry(
    signing_key: &ed25519_dalek::SigningKey,
    did: &str,
    data_dir: PathBuf,
    provider_registry: Option<Weak<ProviderRegistry>>,
) -> Result<CarrierNode> {
    let secret_key = SecretKey::from_bytes(&signing_key.to_bytes());

    // Build an endpoint with Iroh's N0 DNS and relay services unless a custom
    // relay is configured. Topic discovery remains the tracker's DHT layer.
    let mut builder = Endpoint::builder(iroh::endpoint::presets::N0).secret_key(secret_key.clone());
    if let Ok(relay_url) = std::env::var("ELASTOS_RELAY_URL") {
        if let Ok(url) = relay_url.parse::<url::Url>() {
            let config = iroh::RelayConfig::new(url.into(), Some(Default::default()));
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
            Err(_) => Endpoint::builder(iroh::endpoint::presets::N0)
                .secret_key(secret_key)
                .bind()
                .await
                .context("Failed to bind Carrier endpoint")?,
        },
        Err(_) => Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(secret_key)
            .bind()
            .await
            .context("Failed to bind Carrier endpoint")?,
    };

    // Add mDNS for LAN discovery alongside the N0 preset lookup services.
    if carrier_mdns_enabled() {
        if let Ok(mdns) = MdnsAddressLookup::builder().build(endpoint.id()) {
            endpoint
                .address_lookup()
                .context("Carrier endpoint closed before mDNS setup")?
                .add(mdns);
        }
    } else {
        info!("carrier: mDNS discovery disabled by ELASTOS_CARRIER_MDNS");
    }

    // Add MemoryLookup for explicit peer addresses (--connect tickets)
    let memory_lookup = MemoryLookup::new();
    endpoint
        .address_lookup()
        .context("Carrier endpoint closed before address lookup setup")?
        .add(memory_lookup.clone());

    let gossip = spawn_carrier_gossip(&endpoint);

    let gossip_state = Arc::new(Mutex::new(GossipState::new(
        endpoint.clone(),
        gossip.clone(),
        memory_lookup.clone(),
        Some(signing_key.clone()),
        Some(did.to_string()),
    )));

    let file_handler = FileHandler {
        data_dir: data_dir.clone(),
        provider_registry,
        gossip_state: gossip_state.clone(),
    };
    let operator_handler = OperatorHandler::new(OperatorRuntimeContext {
        data_dir: data_dir.clone(),
        local_did: did.to_string(),
        endpoint: endpoint.clone(),
        peers: gossip_state.lock().await.peers.clone(),
        request_serial: Arc::new(Mutex::new(())),
    });
    let router = Router::builder(endpoint.clone())
        .accept(CARRIER_ALPN, file_handler)
        .accept(OPERATOR_ALPN, operator_handler)
        .accept(iroh_gossip::ALPN, gossip.clone())
        .spawn();

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

#[derive(Clone)]
struct FileHandler {
    data_dir: PathBuf,
    provider_registry: Option<Weak<ProviderRegistry>>,
    gossip_state: Arc<Mutex<GossipState>>,
}

impl std::fmt::Debug for FileHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileHandler")
            .field("data_dir", &self.data_dir)
            .field("provider_registry", &self.provider_registry.is_some())
            .field("gossip_state", &"<gossip-state>")
            .finish()
    }
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
        let provider_registry = self.provider_registry.clone();
        let gossip_state = self.gossip_state.clone();
        Box::pin(async move {
            handle_file_connection(conn, &data_dir, provider_registry, gossip_state)
                .await
                .map_err(|e| AcceptError::from(std::io::Error::other(e.to_string())))
        })
    }
}

async fn handle_file_connection(
    conn: iroh::endpoint::Connection,
    data_dir: &std::path::Path,
    provider_registry: Option<Weak<ProviderRegistry>>,
    gossip_state: Arc<Mutex<GossipState>>,
) -> Result<()> {
    let source_endpoint_id = conn.remote_id();
    loop {
        let (mut send, recv) = match conn.accept_bi().await {
            Ok(streams) => streams,
            Err(_) => break,
        };
        let data_dir = data_dir.to_path_buf();
        let provider_registry = provider_registry.clone();
        let gossip_state = gossip_state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_file_stream(
                &mut send,
                recv,
                &data_dir,
                provider_registry,
                gossip_state,
                source_endpoint_id,
            )
            .await
            {
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
    provider_registry: Option<Weak<ProviderRegistry>>,
    gossip_state: Arc<Mutex<GossipState>>,
    source_endpoint_id: iroh::PublicKey,
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
        "content_fetch" => {
            let Some(registry) = provider_registry.and_then(|registry| registry.upgrade()) else {
                send_json(
                    send,
                    &serde_json::json!({
                        "ok": false,
                        "error": "content provider registry unavailable"
                    }),
                )
                .await?;
                return Ok(());
            };
            let cid = msg
                .data
                .get("cid")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let path = msg
                .data
                .get("path")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            match carrier_content_fetch_bytes(&registry, cid, path).await {
                Ok(content) => {
                    let len = content.len() as u64;
                    send.write_all(&len.to_be_bytes()).await?;
                    send.write_all(&content).await?;
                    send.finish()?;
                    send.stopped().await.ok();
                    info!(
                        "carrier: served content {}{}{} ({} bytes)",
                        cid,
                        if path.is_empty() { "" } else { "/" },
                        path,
                        len
                    );
                }
                Err(err) => {
                    send_json(
                        send,
                        &serde_json::json!({
                            "ok": false,
                            "error": err.to_string(),
                        }),
                    )
                    .await?;
                }
            }
        }
        "provider_invoke" => {
            let Some(registry) = provider_registry.and_then(|registry| registry.upgrade()) else {
                send_json(
                    send,
                    &serde_json::json!({
                        "ok": false,
                        "code": "provider_registry_unavailable",
                        "error": "provider registry unavailable"
                    }),
                )
                .await?;
                return Ok(());
            };
            let response =
                carrier_provider_invoke_registry(&registry, &msg.data, &source_endpoint_id).await?;
            send_json(send, &response).await?;
        }
        "browser_exit_stream" => {
            let Some(registry) = provider_registry.and_then(|registry| registry.upgrade()) else {
                send_json(
                    send,
                    &serde_json::json!({
                        "ok": false,
                        "code": "provider_registry_unavailable",
                        "error": "provider registry unavailable"
                    }),
                )
                .await?;
                return Ok(());
            };
            let buffered = reader.buffer().to_vec();
            let recv = reader.into_inner();
            return handle_browser_carrier_exit_stream(send, recv, buffered, registry, &msg.data)
                .await;
        }
        "gossip_push" => {
            let response = carrier_gossip_push(gossip_state, &msg.data).await;
            send_json(send, &response).await?;
        }
        "gossip_pull" => {
            let response = carrier_gossip_pull(gossip_state, &msg.data).await;
            send_json(send, &response).await?;
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

async fn carrier_content_fetch_bytes(
    registry: &ProviderRegistry,
    cid: &str,
    path: &str,
) -> Result<Vec<u8>> {
    validate_content_cid(cid).map_err(anyhow::Error::msg)?;
    validate_carrier_content_path(path).map_err(anyhow::Error::msg)?;
    let mut request = serde_json::json!({
        "op": "cat",
        "cid": cid,
    });
    if !path.is_empty() {
        request["path"] = serde_json::Value::String(path.to_string());
    }
    let response = registry
        .send_raw("ipfs", &request)
        .await
        .map_err(|err| anyhow::anyhow!("ipfs-provider unavailable: {err}"))?;
    if response.get("status").and_then(|status| status.as_str()) == Some("error") {
        let message = response
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("unknown error");
        anyhow::bail!("ipfs-provider fetch failed: {message}");
    }
    let data = response
        .get("data")
        .and_then(|data| data.get("data"))
        .and_then(|data| data.as_str())
        .ok_or_else(|| anyhow::anyhow!("ipfs-provider response missing data"))?;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|err| anyhow::anyhow!("ipfs-provider returned invalid base64: {err}"))
}

fn validate_carrier_gossip_topic(topic: &str) -> std::result::Result<(), String> {
    let topic = topic.trim();
    if topic.is_empty() {
        return Err("topic required".to_string());
    }
    if topic.len() > GOSSIP_CARRIER_PUSH_MAX_TOPIC_LEN {
        return Err("topic too long".to_string());
    }
    Ok(())
}

fn carrier_gossip_message_from_value(
    value: &serde_json::Value,
) -> std::result::Result<GossipMessage, (&'static str, String)> {
    let encoded =
        serde_json::to_vec(value).map_err(|err| ("message_encoding_failed", err.to_string()))?;
    if encoded.len() > GOSSIP_WIRE_MESSAGE_MAX_BYTES {
        return Err((
            "message_too_large",
            "complete gossip message exceeds wire limit".to_string(),
        ));
    }
    let message: GossipMessage = serde_json::from_value(value.clone())
        .map_err(|err| ("invalid_message", format!("invalid gossip message: {err}")))?;
    serialize_gossip_wire_message(&message).map_err(|_| {
        (
            "message_too_large",
            "complete gossip message exceeds wire limit".to_string(),
        )
    })?;
    Ok(message)
}

async fn carrier_gossip_push(
    state: Arc<Mutex<GossipState>>,
    data: &serde_json::Value,
) -> serde_json::Value {
    let topic = data
        .get("topic")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim();
    if let Err(error) = validate_carrier_gossip_topic(topic) {
        return serde_json::json!({ "ok": false, "error": error });
    }
    let Some(message_value) = data.get("message") else {
        return serde_json::json!({ "ok": false, "error": "message required" });
    };
    let message = match carrier_gossip_message_from_value(message_value) {
        Ok(message) => message,
        Err((code, error)) => {
            return serde_json::json!({ "ok": false, "code": code, "error": error })
        }
    };

    let buffers = {
        let state = state.lock().await;
        state.buffers.clone()
    };
    let inserted = {
        let mut buffers = buffers.lock().await;
        let buffer = buffers.entry(topic.to_string()).or_default();
        push_gossip_buffer_message(buffer, message)
    };
    serde_json::json!({
        "ok": true,
        "inserted": inserted,
    })
}

async fn carrier_gossip_pull(
    state: Arc<Mutex<GossipState>>,
    data: &serde_json::Value,
) -> serde_json::Value {
    let topic = data
        .get("topic")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim();
    if let Err(error) = validate_carrier_gossip_topic(topic) {
        return serde_json::json!({ "ok": false, "error": error });
    }
    let limit = data
        .get("limit")
        .and_then(|value| value.as_u64())
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(128)
        .clamp(1, 512);
    let skip_sender_id = data
        .get("skip_sender_id")
        .and_then(|value| value.as_str())
        .unwrap_or_default();

    let buffers = {
        let state = state.lock().await;
        state.buffers.clone()
    };
    let messages = {
        let buffers = buffers.lock().await;
        buffers
            .get(topic)
            .map(|buffer| {
                buffer
                    .messages
                    .iter()
                    .rev()
                    .filter(|message| {
                        skip_sender_id.is_empty() || message.sender_id != skip_sender_id
                    })
                    .take(limit)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let mut messages = messages;
    messages.reverse();
    serde_json::json!({
        "ok": true,
        "messages": messages,
    })
}

async fn handle_browser_carrier_exit_stream(
    send: &mut iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
    buffered: Vec<u8>,
    registry: Arc<ProviderRegistry>,
    data: &serde_json::Value,
) -> Result<()> {
    match browser_carrier_exit_relay_path(&registry, data).await {
        Ok(relay_path) => {
            let stream_id = data
                .get("stream_id")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let target = data
                .get("target")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            write_json_line(send, &serde_json::json!({"ok": true})).await?;
            bridge_browser_carrier_stream_to_relay(
                send, recv, buffered, relay_path, stream_id, target,
            )
            .await
        }
        Err(err) => {
            send_json(
                send,
                &serde_json::json!({
                    "ok": false,
                    "code": "browser_exit_stream_unavailable",
                    "error": err.to_string(),
                }),
            )
            .await
        }
    }
}

async fn browser_carrier_exit_relay_path(
    registry: &ProviderRegistry,
    data: &serde_json::Value,
) -> Result<PathBuf> {
    let schema = data
        .get("schema")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if schema != BROWSER_CARRIER_STREAM_SCHEMA {
        anyhow::bail!(
            "browser_exit_stream schema mismatch: expected {BROWSER_CARRIER_STREAM_SCHEMA}, got {schema}"
        );
    }
    let carrier_service = data
        .get("carrier_service")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if carrier_service != "elastos://exit/open_stream" {
        anyhow::bail!("browser_exit_stream carrier_service must be elastos://exit/open_stream");
    }
    let target = data
        .get("target")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("browser_exit_stream missing target"))?;
    let stream_id = data
        .get("stream_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("browser_exit_stream missing stream_id"))?;
    if !stream_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
    {
        anyhow::bail!("browser_exit_stream stream_id must be a safe identifier");
    }
    let principal_id = data
        .get("principal_id")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let reason = data.get("reason").cloned().unwrap_or_else(|| {
        serde_json::json!(format!(
            "remote Browser Carrier exit stream {}",
            data.get("grant_id")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown-grant")
        ))
    });
    let response = registry
        .send_raw(
            "exit",
            &serde_json::json!({
                "op": "open_stream",
                "target": target,
                "principal_id": principal_id,
                "reason": reason,
                "stream_nonce": stream_id,
            }),
        )
        .await
        .map_err(|err| anyhow::anyhow!("remote exit provider unavailable: {err}"))?;
    if response.get("status").and_then(|value| value.as_str()) == Some("error") {
        let message = response
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("remote exit provider rejected Browser Carrier stream");
        anyhow::bail!("{message}");
    }
    let receipt = response
        .get("data")
        .and_then(|value| value.as_object())
        .ok_or_else(|| anyhow::anyhow!("remote exit provider returned invalid stream receipt"))?;
    let relay_ipc = receipt
        .get("relay_ipc")
        .and_then(|value| value.as_object())
        .ok_or_else(|| anyhow::anyhow!("remote exit provider did not return relay_ipc"))?;
    if relay_ipc.get("schema").and_then(|value| value.as_str()) != Some("elastos.exit.relay-ipc/v1")
    {
        anyhow::bail!("remote exit provider relay_ipc schema mismatch");
    }
    if relay_ipc.get("kind").and_then(|value| value.as_str()) != Some("unix_socket") {
        anyhow::bail!("remote exit provider relay_ipc must use unix_socket");
    }
    let path = relay_ipc
        .get("path")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("remote exit provider relay_ipc missing path"))?;
    if path.is_empty() || !path.starts_with('/') {
        anyhow::bail!("remote exit provider relay_ipc path must be absolute");
    }
    if path
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte == b'\0')
    {
        anyhow::bail!("remote exit provider relay_ipc path must not contain whitespace or NUL");
    }
    Ok(PathBuf::from(path))
}

#[cfg(unix)]
async fn bridge_browser_carrier_stream_to_relay(
    send: &mut iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    buffered: Vec<u8>,
    relay_path: PathBuf,
    stream_id: String,
    target: String,
) -> Result<()> {
    let mut relay_stream = UnixStream::connect(&relay_path)
        .await
        .with_context(|| format!("connect remote exit relay {}", relay_path.display()))?;
    if !buffered.is_empty() {
        relay_stream.write_all(&buffered).await?;
    }
    let (mut relay_read, mut relay_write) = relay_stream.split();
    let to_relay = async {
        let copied = io::copy(&mut recv, &mut relay_write).await?;
        relay_write.shutdown().await.ok();
        Ok::<u64, anyhow::Error>(copied)
    };
    let from_relay = async {
        let copied = io::copy(&mut relay_read, send).await?;
        send.finish()?;
        send.stopped().await.ok();
        Ok::<u64, anyhow::Error>(copied)
    };
    let (to_relay, to_engine) = tokio::try_join!(to_relay, from_relay)?;
    tracing::info!(
        relay = %relay_path.display(),
        stream_id = %stream_id,
        target = %target,
        to_relay,
        to_engine,
        "Browser Carrier exit stream closed"
    );
    Ok(())
}

#[cfg(not(unix))]
async fn bridge_browser_carrier_stream_to_relay(
    _send: &mut iroh::endpoint::SendStream,
    _recv: iroh::endpoint::RecvStream,
    _buffered: Vec<u8>,
    _relay_path: PathBuf,
    _stream_id: String,
    _target: String,
) -> Result<()> {
    anyhow::bail!("Browser Carrier exit stream requires a Unix relay host")
}

async fn carrier_provider_invoke_registry(
    registry: &ProviderRegistry,
    data: &serde_json::Value,
    source_endpoint_id: &iroh::PublicKey,
) -> Result<serde_json::Value> {
    let source = data
        .get("source")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("provider_invoke missing source"))?;
    let target = data
        .get("target")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("provider_invoke missing target"))?;
    let operation = data
        .get("operation")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("provider_invoke missing operation"))?;
    let transfer = data
        .get("transfer")
        .and_then(|value| value.as_str())
        .unwrap_or("json");
    let mut request = data
        .get("request")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("provider_invoke missing request"))?;

    if !carrier_provider_target_allowed(target) {
        return Ok(serde_json::json!({
            "ok": false,
            "code": "unauthorized_provider_target",
            "error": "Carrier provider invocation must target an ElastOS service provider, not a raw backend",
        }));
    }
    if let Err(message) =
        validate_carrier_provider_invocation(source, target, operation, transfer, &request)
    {
        return Ok(serde_json::json!({
            "ok": false,
            "code": "invalid_provider_invocation",
            "error": message,
        }));
    }

    let runtime = request
        .get_mut("_runtime_invocation")
        .and_then(serde_json::Value::as_object_mut)
        .expect("validated provider invocation has Runtime metadata");
    runtime.insert(
        "carrier".to_string(),
        serde_json::json!({
            "source_endpoint_did": public_key_to_did(source_endpoint_id),
        }),
    );

    match registry.send_raw(target, &request).await {
        Ok(result) => Ok(serde_json::json!({
            "ok": true,
            "result": result,
        })),
        Err(err) => Ok(serde_json::json!({
            "ok": false,
            "code": "provider_error",
            "error": err.to_string(),
        })),
    }
}

fn carrier_provider_target_allowed(target: &str) -> bool {
    matches!(
        target,
        "content"
            | "availability"
            | "rights"
            | "key"
            | "decrypt"
            | "drm"
            | "collaboration"
            | "collaboration-direct"
            | "collaboration-profile"
    )
}

fn validate_carrier_provider_invocation(
    source: &str,
    target: &str,
    operation: &str,
    transfer: &str,
    request: &serde_json::Value,
) -> std::result::Result<(), String> {
    if !matches!(transfer, "json" | "bytes" | "stream") {
        return Err(format!(
            "provider_invoke transfer must be json, bytes, or stream, got {transfer}"
        ));
    }
    if request.get("_runtime_transfer").is_some() {
        return Err("provider_invoke request must not predeclare _runtime_transfer".to_string());
    }
    let request_op = request
        .get("op")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if request_op != operation {
        return Err(format!(
            "provider_invoke op mismatch: envelope={operation}, request={request_op}"
        ));
    }
    let runtime = request
        .get("_runtime_invocation")
        .and_then(|value| value.as_object())
        .ok_or_else(|| "provider_invoke requires _runtime_invocation".to_string())?;
    let expected_capability = format!("provider:{source}->{target}:{operation}");
    for (field, expected) in [
        ("schema", "elastos.provider.invocation/v1"),
        ("source", source),
        ("target", target),
        ("op", operation),
        ("capability", expected_capability.as_str()),
        ("transport", "carrier-provider-plane"),
        ("transfer", transfer),
    ] {
        let actual = runtime
            .get(field)
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        if actual != expected {
            return Err(format!(
                "provider_invoke runtime field {field} mismatch: expected {expected}, got {actual}"
            ));
        }
    }
    let carrier = runtime
        .get("carrier")
        .ok_or_else(|| "provider_invoke requires carrier metadata".to_string())?;
    if !carrier.is_null() {
        return Err("provider_invoke carrier metadata must be null".to_string());
    }
    if transfer == "stream" {
        validate_carrier_provider_stream_contract(runtime)?;
    }
    Ok(())
}

fn validate_carrier_provider_stream_contract(
    runtime: &serde_json::Map<String, serde_json::Value>,
) -> std::result::Result<(), String> {
    let stream = runtime
        .get("stream")
        .and_then(|value| value.as_object())
        .ok_or_else(|| "provider_invoke stream transfer requires stream metadata".to_string())?;
    let schema = stream
        .get("schema")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if schema != "elastos.provider.stream/v1" {
        return Err(format!(
            "provider_invoke stream schema mismatch: expected elastos.provider.stream/v1, got {schema}"
        ));
    }
    let encoding = stream
        .get("encoding")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if encoding != "base64-chunks" {
        return Err(format!(
            "provider_invoke stream encoding mismatch: expected base64-chunks, got {encoding}"
        ));
    }
    let chunk_size = stream
        .get("chunk_size")
        .and_then(|value| value.as_u64())
        .unwrap_or_default();
    if chunk_size == 0 {
        return Err("provider_invoke stream chunk_size must be greater than zero".to_string());
    }
    Ok(())
}

fn validate_carrier_content_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Ok(());
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Err("content fetch path must be relative".to_string());
    }
    if path.contains('\\') || path.contains('\0') {
        return Err("content fetch path contains invalid characters".to_string());
    }
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err("content fetch path contains an invalid segment".to_string());
        }
    }
    Ok(())
}

async fn send_json(send: &mut iroh::endpoint::SendStream, value: &serde_json::Value) -> Result<()> {
    write_json_line(send, value).await?;
    send.finish()?;
    send.stopped().await.ok();
    Ok(())
}

async fn write_json_line(
    send: &mut iroh::endpoint::SendStream,
    value: &serde_json::Value,
) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    send.write_all(&bytes).await?;
    Ok(())
}

async fn read_browser_carrier_stream_ack(recv: &mut iroh::endpoint::RecvStream) -> Result<()> {
    let mut line = Vec::new();
    loop {
        let mut byte = [0_u8; 1];
        recv.read_exact(&mut byte).await?;
        line.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
        if line.len() > BROWSER_CARRIER_STREAM_ACK_MAX_BYTES {
            anyhow::bail!("Browser Carrier stream ack is too large");
        }
    }
    let response: serde_json::Value =
        serde_json::from_slice(line.strip_suffix(b"\n").unwrap_or(line.as_slice()))?;
    if response.get("ok").and_then(|value| value.as_bool()) == Some(true) {
        return Ok(());
    }
    let message = response
        .get("error")
        .and_then(|value| value.as_str())
        .unwrap_or("Browser Carrier stream open failed");
    anyhow::bail!("{message}");
}

/// Runtime content availability provider backed by Carrier gossip.
///
/// `content-provider` still owns CID policy, receipts, and local Kubo/IPFS
/// backend use. This provider only announces content availability through the
/// Carrier plane so apps keep using `elastos://content/*` instead of raw
/// peer/topic/IPFS authority.
pub struct CarrierAvailabilityProvider {
    state: Arc<Mutex<GossipState>>,
    provider_registry: Option<Weak<ProviderRegistry>>,
    peer_reputation: Arc<Mutex<HashMap<String, CarrierPeerReputation>>>,
    data_dir: Option<PathBuf>,
    peer_attestation_exchange: Option<CarrierPeerAttestationExchangeClient>,
}

#[derive(Debug, Clone)]
pub struct CarrierPeerAttestationExchangeClient {
    endpoints: Vec<CarrierPeerAttestationExchangeEndpoint>,
    quorum: usize,
}

#[derive(Debug, Clone)]
struct CarrierPeerAttestationExchangeEndpoint {
    id: String,
    url: String,
    authorization: Option<String>,
    timeout_secs: u64,
}

impl CarrierAvailabilityProvider {
    pub fn new(state: Arc<Mutex<GossipState>>) -> Self {
        Self {
            state,
            provider_registry: None,
            peer_reputation: Arc::new(Mutex::new(HashMap::new())),
            data_dir: None,
            peer_attestation_exchange: None,
        }
    }

    pub fn with_provider_registry(
        state: Arc<Mutex<GossipState>>,
        provider_registry: Weak<ProviderRegistry>,
    ) -> Self {
        Self {
            state,
            provider_registry: Some(provider_registry),
            peer_reputation: Arc::new(Mutex::new(HashMap::new())),
            data_dir: None,
            peer_attestation_exchange: None,
        }
    }

    pub fn with_provider_registry_and_data_dir(
        state: Arc<Mutex<GossipState>>,
        provider_registry: Weak<ProviderRegistry>,
        data_dir: PathBuf,
    ) -> Self {
        Self::with_provider_registry_data_dir_and_peer_attestation_exchange_config(
            state,
            provider_registry,
            data_dir,
            None,
        )
    }

    pub fn with_provider_registry_data_dir_and_peer_attestation_exchange_config(
        state: Arc<Mutex<GossipState>>,
        provider_registry: Weak<ProviderRegistry>,
        data_dir: PathBuf,
        peer_attestation_exchange_config: Option<serde_json::Value>,
    ) -> Self {
        let peer_reputation = load_carrier_peer_reputation(&data_dir);
        let peer_attestation_exchange = peer_attestation_exchange_config.and_then(|config| {
            match CarrierPeerAttestationExchangeClient::from_config(config) {
                Ok(client) => Some(client),
                Err(err) => {
                    tracing::warn!("carrier peer-attestation exchange disabled: {}", err);
                    None
                }
            }
        });
        Self {
            state,
            provider_registry: Some(provider_registry),
            peer_reputation: Arc::new(Mutex::new(peer_reputation)),
            data_dir: Some(data_dir),
            peer_attestation_exchange,
        }
    }

    async fn record_peer_reputation(&self, node_did: &str, success: bool) {
        let snapshot = {
            let mut reputation = self.peer_reputation.lock().await;
            let entry = reputation.entry(node_did.to_string()).or_default();
            if success {
                entry.successes = entry.successes.saturating_add(1);
            } else {
                entry.failures = entry.failures.saturating_add(1);
            }
            reputation.clone()
        };
        if let Some(data_dir) = &self.data_dir {
            if let Err(err) = save_carrier_peer_reputation(data_dir, &snapshot) {
                tracing::debug!("carrier peer reputation save failed: {}", err);
            }
        }
    }

    async fn exchange_peer_attestations(
        &self,
        exchange_request: CarrierPeerAttestationExchangeRequest<'_>,
    ) -> Option<serde_json::Value> {
        let Some(exchange) = &self.peer_attestation_exchange else {
            return None;
        };
        if exchange_request.remote_proofs.is_empty() {
            return None;
        }
        let request = match carrier_peer_attestation_exchange_request(
            exchange_request.signing_key,
            exchange_request.cid,
            exchange_request.topic_uri,
            exchange_request.local_node_did,
            exchange_request.remote_proofs,
            exchange_request.live_multi_peer_proof,
            exchange_request.requested_at,
        ) {
            Ok(request) => request,
            Err(err) => {
                return Some(serde_json::json!({
                    "schema": CARRIER_PEER_ATTESTATION_EXCHANGE_RECEIPT_SCHEMA,
                    "provider": "carrier-availability",
                    "scope": "content-availability",
                    "configured": true,
                    "accepted": false,
                    "status": "failed",
                    "reason": format!("peer-attestation exchange request build failed: {err}"),
                    "exchange": exchange.redacted_status_json(),
                    "credential_exposed": false,
                }))
            }
        };
        match exchange.exchange(&request).await {
            Ok(receipt) => Some(receipt),
            Err(err) => Some(serde_json::json!({
                "schema": CARRIER_PEER_ATTESTATION_EXCHANGE_RECEIPT_SCHEMA,
                "provider": "carrier-availability",
                "scope": "content-availability",
                "configured": true,
                "accepted": false,
                "status": "failed",
                "reason": err,
                "exchange": exchange.redacted_status_json(),
                "credential_exposed": false,
            })),
        }
    }
}

impl std::fmt::Debug for CarrierAvailabilityProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CarrierAvailabilityProvider").finish()
    }
}

impl CarrierPeerAttestationExchangeClient {
    pub fn from_config(config: serde_json::Value) -> Result<Self, String> {
        let payload = config
            .get("extra")
            .filter(|extra| !extra.is_null())
            .unwrap_or(&config);
        let default_authorization = payload
            .get("authorization")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if let Some(value) = &default_authorization {
            validate_carrier_authorization_header_value(value)?;
        }
        let default_timeout_secs = payload
            .get("timeout_secs")
            .and_then(|value| value.as_u64())
            .unwrap_or(5)
            .clamp(1, 60);
        let endpoints = match payload.get("endpoints").and_then(|value| value.as_array()) {
            Some(values) if !values.is_empty() => values
                .iter()
                .enumerate()
                .map(|(index, endpoint)| {
                    CarrierPeerAttestationExchangeEndpoint::from_config(
                        endpoint,
                        index,
                        default_authorization.as_deref(),
                        default_timeout_secs,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => vec![CarrierPeerAttestationExchangeEndpoint::from_config(
                payload,
                0,
                default_authorization.as_deref(),
                default_timeout_secs,
            )?],
        };
        if endpoints.len() > 5 {
            return Err(
                "carrier peer-attestation exchange supports at most 5 endpoints".to_string(),
            );
        }
        let quorum = payload
            .get("quorum")
            .or_else(|| payload.get("required_quorum"))
            .and_then(|value| value.as_u64())
            .map(|value| value as usize)
            .unwrap_or(endpoints.len());
        if quorum == 0 || quorum > endpoints.len() {
            return Err(format!(
                "carrier peer-attestation exchange quorum must be between 1 and {}",
                endpoints.len()
            ));
        }
        Ok(Self { endpoints, quorum })
    }

    fn redacted_status_json(&self) -> serde_json::Value {
        let first = self.endpoints.first();
        let parsed = first.and_then(|endpoint| url::Url::parse(&endpoint.url).ok());
        serde_json::json!({
            "configured": true,
            "delivery": "carrier_peer_attestation_exchange",
            "endpoint_count": self.endpoints.len(),
            "multi_endpoint": self.endpoints.len() > 1,
            "quorum_required": self.quorum,
            "endpoints": self
                .endpoints
                .iter()
                .map(CarrierPeerAttestationExchangeEndpoint::redacted_status_json)
                .collect::<Vec<_>>(),
            "scheme": parsed.as_ref().map(|url| url.scheme()).unwrap_or("unknown"),
            "host": parsed
                .as_ref()
                .and_then(|url| url.host_str())
                .unwrap_or("unknown"),
            "port": parsed.as_ref().and_then(|url| url.port()),
            "path_configured": parsed
                .as_ref()
                .map(|url| !url.path().trim_matches('/').is_empty())
                .unwrap_or(false),
            "authorization_configured": self
                .endpoints
                .iter()
                .any(|endpoint| endpoint.authorization.is_some()),
            "timeout_secs": first.map(|endpoint| endpoint.timeout_secs).unwrap_or(0),
            "credential_exposed": false,
        })
    }

    async fn exchange(
        &self,
        request_payload: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let mut endpoint_receipts = Vec::new();
        let mut accepted_receipts = 0_usize;
        let mut rejected_receipts = 0_usize;
        let mut failed_receipts = 0_usize;
        let mut verified_receipts = 0_usize;
        let mut first_verified_signed_receipt = None;
        let mut reasons = Vec::new();

        for endpoint in &self.endpoints {
            let receipt = endpoint
                .exchange(request_payload)
                .await
                .unwrap_or_else(|err| {
                    failed_receipts = failed_receipts.saturating_add(1);
                    carrier_peer_attestation_endpoint_unavailable(err, endpoint)
                });
            if receipt
                .get("accepted")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                accepted_receipts = accepted_receipts.saturating_add(1);
            } else if receipt
                .get("status")
                .and_then(|value| value.as_str())
                .is_some_and(|status| status == "rejected")
            {
                rejected_receipts = rejected_receipts.saturating_add(1);
            }
            if receipt
                .get("signed_receipt")
                .and_then(|value| value.get("verified"))
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                verified_receipts = verified_receipts.saturating_add(1);
                if first_verified_signed_receipt.is_none() {
                    first_verified_signed_receipt = receipt.get("signed_receipt").cloned();
                }
            }
            if let Some(reason) = receipt.get("reason").and_then(|value| value.as_str()) {
                reasons.push(reason.to_string());
            }
            endpoint_receipts.push(receipt);
        }

        let accepted = accepted_receipts >= self.quorum;
        let reason = if accepted {
            format!(
                "carrier peer-attestation quorum accepted: {accepted_receipts}/{} verified endpoints accepted",
                self.endpoints.len()
            )
        } else if reasons.is_empty() {
            format!(
                "carrier peer-attestation quorum rejected: {accepted_receipts}/{} accepted, quorum {}",
                self.endpoints.len(),
                self.quorum
            )
        } else {
            format!(
                "carrier peer-attestation quorum rejected: {accepted_receipts}/{} accepted, quorum {}; {}",
                self.endpoints.len(),
                self.quorum,
                reasons.join("; ")
            )
        };
        let mut signed_receipt = first_verified_signed_receipt.unwrap_or_else(|| {
            serde_json::json!({
                "verified": false,
            })
        });
        signed_receipt["verified"] = serde_json::Value::Bool(verified_receipts > 0);
        signed_receipt["verified_receipts"] = serde_json::Value::from(verified_receipts);

        Ok(serde_json::json!({
            "schema": CARRIER_PEER_ATTESTATION_EXCHANGE_RECEIPT_SCHEMA,
            "provider": "carrier-availability",
            "scope": "content-availability",
            "configured": true,
            "accepted": accepted,
            "status": if accepted { "accepted" } else { "rejected" },
            "exchange": self.redacted_status_json(),
            "quorum": {
                "required": self.quorum,
                "endpoint_count": self.endpoints.len(),
                "accepted": accepted_receipts,
                "rejected": rejected_receipts,
                "failed": failed_receipts,
                "verified": verified_receipts,
            },
            "endpoint_receipts": endpoint_receipts,
            "signed_receipt": signed_receipt,
            "reason": reason,
            "credential_exposed": false,
        }))
    }
}

impl CarrierPeerAttestationExchangeEndpoint {
    fn from_config(
        payload: &serde_json::Value,
        index: usize,
        default_authorization: Option<&str>,
        default_timeout_secs: u64,
    ) -> Result<Self, String> {
        let url = payload
            .get("url")
            .or_else(|| payload.get("exchange_url"))
            .or_else(|| payload.get("endpoint_url"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "carrier peer-attestation exchange endpoint requires url".to_string())?;
        validate_carrier_external_endpoint_url(url)?;
        let authorization = payload
            .get("authorization")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or(default_authorization)
            .map(str::to_string);
        if let Some(value) = &authorization {
            validate_carrier_authorization_header_value(value)?;
        }
        let timeout_secs = payload
            .get("timeout_secs")
            .and_then(|value| value.as_u64())
            .unwrap_or(default_timeout_secs)
            .clamp(1, 60);
        let id = payload
            .get("id")
            .or_else(|| payload.get("provider_id"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("peer-attestation-{}", index + 1));
        Ok(Self {
            id,
            url: url.to_string(),
            authorization,
            timeout_secs,
        })
    }

    fn redacted_status_json(&self) -> serde_json::Value {
        let parsed = url::Url::parse(&self.url).ok();
        serde_json::json!({
            "id": self.id,
            "scheme": parsed.as_ref().map(|url| url.scheme()).unwrap_or("unknown"),
            "host": parsed
                .as_ref()
                .and_then(|url| url.host_str())
                .unwrap_or("unknown"),
            "port": parsed.as_ref().and_then(|url| url.port()),
            "path_configured": parsed
                .as_ref()
                .map(|url| !url.path().trim_matches('/').is_empty())
                .unwrap_or(false),
            "authorization_configured": self.authorization.is_some(),
            "timeout_secs": self.timeout_secs,
            "credential_exposed": false,
        })
    }

    async fn exchange(
        &self,
        request_payload: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()
            .map_err(|err| format!("peer-attestation exchange client build failed: {err}"))?;
        let mut request = client.post(&self.url).json(request_payload);
        if let Some(authorization) = &self.authorization {
            request = request.header("Authorization", authorization);
        }
        let response = request
            .send()
            .await
            .map_err(|err| format!("peer-attestation exchange request failed: {err}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!(
                "peer-attestation exchange returned HTTP {}",
                status.as_u16()
            ));
        }
        let response_json = response
            .json::<serde_json::Value>()
            .await
            .map_err(|err| format!("peer-attestation exchange response decode failed: {err}"))?;
        carrier_peer_attestation_exchange_receipt_from_response(
            &response_json,
            self.redacted_status_json(),
            status.as_u16(),
        )
    }
}

fn content_availability_topic_name(cid: &str) -> String {
    let digest = Sha256::digest(cid.as_bytes());
    format!("__elastos_content/v1/{}", hex::encode(digest))
}

fn content_availability_topic_uri(cid: &str) -> String {
    let digest = Sha256::digest(cid.as_bytes());
    format!(
        "elastos://carrier/content/{}/availability",
        hex::encode(digest)
    )
}

fn carrier_availability_error(code: &str, message: impl Into<String>) -> serde_json::Value {
    serde_json::json!({
        "status": "error",
        "code": code,
        "message": message.into(),
    })
}

fn validate_content_cid(cid: &str) -> Result<(), String> {
    let cid = cid.trim();
    if cid.len() < 8 || cid.len() > 128 {
        return Err("content availability requires a valid CID".to_string());
    }
    if !cid
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("content availability CID contains unsupported characters".to_string());
    }
    Ok(())
}

fn validate_carrier_external_endpoint_url(raw: &str) -> Result<(), String> {
    let url = url::Url::parse(raw)
        .map_err(|err| format!("invalid carrier external endpoint URL: {err}"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(
            "carrier external endpoint URL must not contain inline credentials".to_string(),
        );
    }
    match url.scheme() {
        "https" => Ok(()),
        "http" if matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1")) => Ok(()),
        _ => Err("carrier external endpoint URL must use https or local loopback http".to_string()),
    }
}

fn validate_carrier_authorization_header_value(value: &str) -> Result<(), String> {
    if value.bytes().any(|byte| matches!(byte, b'\r' | b'\n')) {
        return Err("carrier authorization header contains invalid newline".to_string());
    }
    Ok(())
}

fn local_replica_count(request: &serde_json::Value) -> u32 {
    request
        .get("local")
        .and_then(|value| value.get("replicas"))
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0)
}

fn carrier_peer_attestation_remote_proofs_json(
    remote_proofs: &[CarrierReplicationProof],
) -> Vec<serde_json::Value> {
    remote_proofs
        .iter()
        .map(|proof| {
            serde_json::json!({
                "node_did": proof.node_did.clone(),
                "endpoint_id": proof.endpoint_id.clone(),
                "announced_at": proof.announced_at,
                "score": proof.score,
                "selection_reason": proof.selection_reason.clone(),
                "local_reputation": {
                    "scope": "local_runtime",
                    "score_delta": proof.reputation_score,
                    "reason": proof.reputation_reason.clone(),
                },
                "admission": proof.admission.clone(),
                "ensure_status": proof.ensure_status.clone(),
                "status": proof
                    .status_availability
                    .get("status")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown"),
                "remote_receipt": proof.remote_receipt.as_ref().map(|receipt| {
                    serde_json::json!({
                        "schema": receipt.get("schema").cloned().unwrap_or(serde_json::Value::Null),
                        "cid": receipt.get("cid").cloned().unwrap_or(serde_json::Value::Null),
                        "status": receipt.get("status").cloned().unwrap_or(serde_json::Value::Null),
                        "signer_did": receipt.get("signer_did").cloned().unwrap_or(serde_json::Value::Null),
                        "verified": receipt.get("verified").cloned().unwrap_or(serde_json::Value::Bool(false)),
                    })
                }),
                "checked_at": proof.checked_at,
            })
        })
        .collect()
}

fn carrier_peer_attestation_exchange_request(
    signing_key: &ed25519_dalek::SigningKey,
    cid: &str,
    topic_uri: &str,
    local_node_did: &str,
    remote_proofs: &[CarrierReplicationProof],
    live_multi_peer_proof: bool,
    requested_at: u64,
) -> Result<serde_json::Value, ProviderError> {
    let payload = serde_json::json!({
        "schema": CARRIER_PEER_ATTESTATION_EXCHANGE_REQUEST_SCHEMA,
        "provider": "carrier-availability",
        "scope": "content-availability",
        "cid": cid,
        "topic": topic_uri,
        "local_node_did": local_node_did,
        "live_multi_peer_proof": live_multi_peer_proof,
        "remote_provider_proofs": remote_proofs.len(),
        "remote_proofs": carrier_peer_attestation_remote_proofs_json(remote_proofs),
        "requested_at": requested_at,
        "authority": {
            "runtime_invocation_required": true,
            "provider_owned_exchange": true,
            "raw_carrier_ticket_exposed": false,
            "raw_backend_access": false,
        },
    });
    let canonical = serde_json::to_string(&payload).map_err(|err| {
        ProviderError::Provider(format!(
            "Carrier peer-attestation request serialization failed: {err}"
        ))
    })?;
    let (signature, signer_did) = crate::crypto::domain_separated_sign(
        signing_key,
        CARRIER_PEER_ATTESTATION_EXCHANGE_REQUEST_DOMAIN,
        canonical.as_bytes(),
    );
    Ok(serde_json::json!({
        "payload": payload,
        "signature": signature,
        "signer_did": signer_did,
    }))
}

fn carrier_peer_attestation_exchange_receipt_from_response(
    response: &serde_json::Value,
    exchange: serde_json::Value,
    http_status: u16,
) -> Result<serde_json::Value, String> {
    let accepted = response
        .get("accepted")
        .and_then(|value| value.as_bool())
        .ok_or_else(|| {
            "peer-attestation exchange response requires accepted boolean".to_string()
        })?;
    let signed_receipt = response.get("receipt").cloned();
    let verified_receipt = match signed_receipt.as_ref() {
        Some(receipt) => {
            let signer_did = receipt
                .get("signer_did")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    "peer-attestation exchange signed receipt requires signer_did".to_string()
                })?;
            let receipt_bytes = serde_json::to_vec(receipt)
                .map_err(|err| format!("peer-attestation exchange receipt encode failed: {err}"))?;
            let expected_signers = [signer_did.to_string()];
            crate::crypto::verify_signed_json_envelope_against_dids(
                &receipt_bytes,
                CARRIER_PEER_ATTESTATION_EXCHANGE_RECEIPT_DOMAIN,
                &expected_signers,
            )
            .map_err(|err| {
                format!("peer-attestation exchange receipt verification failed: {err}")
            })?;
            let payload = receipt
                .get("payload")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            Some(serde_json::json!({
                "verified": true,
                "signer_did": signer_did,
                "payload_schema": payload
                    .get("schema")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                "exchange_id": payload
                    .get("exchange_id")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                "receipt_id": payload
                    .get("receipt_id")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            }))
        }
        None if accepted => {
            return Err(
                "peer-attestation exchange accepted response requires signed receipt".to_string(),
            )
        }
        None => None,
    };
    Ok(serde_json::json!({
        "schema": CARRIER_PEER_ATTESTATION_EXCHANGE_RECEIPT_SCHEMA,
        "provider": "carrier-availability",
        "scope": "content-availability",
        "configured": true,
        "accepted": accepted,
        "status": if accepted { "accepted" } else { "rejected" },
        "http_status": http_status,
        "exchange": exchange,
        "remote_schema": response.get("schema").cloned().unwrap_or(serde_json::Value::Null),
        "remote_exchange_id": response.get("exchange_id").cloned().unwrap_or(serde_json::Value::Null),
        "remote_receipt_id": response.get("receipt_id").cloned().unwrap_or(serde_json::Value::Null),
        "signed_receipt": verified_receipt.unwrap_or_else(|| {
            serde_json::json!({
                "verified": false,
                "reason": "no signed receipt returned",
            })
        }),
        "reason": response.get("reason").cloned().unwrap_or(serde_json::Value::Null),
        "credential_exposed": false,
    }))
}

fn carrier_peer_attestation_endpoint_unavailable(
    reason: String,
    endpoint: &CarrierPeerAttestationExchangeEndpoint,
) -> serde_json::Value {
    serde_json::json!({
        "schema": CARRIER_PEER_ATTESTATION_EXCHANGE_RECEIPT_SCHEMA,
        "provider": "carrier-availability",
        "scope": "content-availability",
        "configured": true,
        "accepted": false,
        "status": "failed",
        "exchange": endpoint.redacted_status_json(),
        "signed_receipt": {
            "verified": false,
            "reason": reason,
        },
        "reason": reason,
        "credential_exposed": false,
    })
}

#[derive(Debug, Clone, Copy)]
struct CarrierAvailabilityRequirements {
    min_replicas: u32,
    max_replicas: Option<u32>,
    require_live_multi_peer_proof: bool,
    repair_graph_kind: CarrierRepairGraphKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CarrierRepairGraphKind {
    Auto,
    ObjectManifest,
    ExactBytes,
    IpldDag,
}

impl CarrierAvailabilityRequirements {
    fn from_request(request: &serde_json::Value) -> Self {
        let requirements = request
            .get("requirements")
            .or_else(|| request.get("availability_requirements"));
        let min_replicas = requirements
            .and_then(|value| value.get("min_replicas"))
            .and_then(|value| value.as_u64())
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(1)
            .max(1);
        let max_replicas = requirements
            .and_then(|value| value.get("max_replicas"))
            .and_then(|value| value.as_u64())
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0);
        let require_live_multi_peer_proof = requirements
            .and_then(|value| value.get("require_live_multi_peer_proof"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let repair_graph_kind = CarrierRepairGraphKind::from_requirements(requirements);
        Self {
            min_replicas,
            max_replicas,
            require_live_multi_peer_proof,
            repair_graph_kind,
        }
    }

    fn effective_max(self) -> u32 {
        self.max_replicas
            .unwrap_or(MAX_CARRIER_REPLICATION_CANDIDATES as u32 + 1)
            .min(MAX_CARRIER_REPLICATION_CANDIDATES as u32 + 1)
            .max(1)
    }

    fn to_json(self) -> serde_json::Value {
        serde_json::json!({
            "min_replicas": self.min_replicas,
            "max_replicas": self.max_replicas,
            "require_live_multi_peer_proof": self.require_live_multi_peer_proof,
            "repair_graph_kind": self.repair_graph_kind.as_str(),
        })
    }
}

impl CarrierRepairGraphKind {
    fn from_requirements(requirements: Option<&serde_json::Value>) -> Self {
        let raw = requirements
            .and_then(|value| {
                value
                    .get("repair_graph_kind")
                    .or_else(|| value.get("content_graph_kind"))
                    .or_else(|| value.get("graph_kind"))
                    .or_else(|| {
                        value
                            .get("repair_graph")
                            .and_then(|graph| graph.get("kind"))
                    })
                    .or_else(|| {
                        value
                            .get("content_graph")
                            .and_then(|graph| graph.get("kind"))
                    })
            })
            .and_then(|value| value.as_str())
            .unwrap_or("auto");
        match raw {
            "object_manifest" | "manifest" => Self::ObjectManifest,
            "exact_bytes" | "exact" | "single_block" | "file" => Self::ExactBytes,
            "ipld_dag" | "block_dag" | "dag" | "arbitrary_dag" => Self::IpldDag,
            _ => Self::Auto,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::ObjectManifest => "object_manifest",
            Self::ExactBytes => "exact_bytes",
            Self::IpldDag => "ipld_dag",
        }
    }

    fn supports_current_import_fallback(self) -> bool {
        !matches!(self, Self::IpldDag)
    }
}

#[derive(Debug, Clone)]
struct CarrierAvailabilityReplica {
    node_did: String,
    endpoint_id: Option<String>,
    connect_ticket: String,
    announced_at: u64,
    score: u32,
    selection_reason: String,
    reputation_score: i32,
    reputation_reason: String,
}

#[derive(Debug, Clone)]
struct CarrierReplicationProof {
    node_did: String,
    endpoint_id: Option<String>,
    announced_at: u64,
    score: u32,
    selection_reason: String,
    reputation_score: i32,
    reputation_reason: String,
    ensure_status: String,
    admission: Option<serde_json::Value>,
    status_availability: serde_json::Value,
    remote_receipt: Option<serde_json::Value>,
    transfer: Option<serde_json::Value>,
    checked_at: u64,
}

#[derive(Clone, Copy)]
struct CarrierPeerAttestationExchangeView<'a> {
    configured: bool,
    receipt: Option<&'a serde_json::Value>,
}

struct CarrierPeerAttestationExchangeRequest<'a> {
    signing_key: &'a ed25519_dalek::SigningKey,
    cid: &'a str,
    topic_uri: &'a str,
    local_node_did: &'a str,
    remote_proofs: &'a [CarrierReplicationProof],
    live_multi_peer_proof: bool,
    requested_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CarrierPeerReputation {
    successes: u32,
    failures: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CarrierPeerReputationStore {
    schema: String,
    peers: BTreeMap<String, CarrierPeerReputation>,
}

#[async_trait::async_trait]
impl Provider for CarrierAvailabilityProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "use send_raw for typed content availability operations".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["availability"]
    }

    fn name(&self) -> &'static str {
        "carrier-availability"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        match request.get("op").and_then(|value| value.as_str()) {
            Some("ensure") | Some("repair") => self.announce_availability(request).await,
            Some("fetch") => self.fetch_from_announced_carrier_peers(request).await,
            Some("status") => {
                let state = self.state.lock().await;
                let node_did = state
                    .did
                    .clone()
                    .unwrap_or_else(|| state.endpoint.id().to_string());
                Ok(serde_json::json!({
                    "status": "ok",
                    "data": {
                        "provider": "carrier-availability",
                        "node_did": node_did,
                        "transport": "carrier-gossip",
                        "joined_topic_count": state.joined_topics.len(),
                    }
                }))
            }
            _ => Ok(carrier_availability_error(
                "unsupported_op",
                "unsupported Carrier availability operation",
            )),
        }
    }
}

impl CarrierAvailabilityProvider {
    async fn announce_availability(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let cid = match request.get("cid").and_then(|value| value.as_str()) {
            Some(cid) => cid.trim(),
            None => {
                return Ok(carrier_availability_error(
                    "invalid_request",
                    "Carrier availability requires cid",
                ))
            }
        };
        if let Err(err) = validate_content_cid(cid) {
            return Ok(carrier_availability_error("invalid_cid", err));
        }
        let uri = request
            .get("uri")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("elastos://{cid}"));
        let policy = request
            .get("policy")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("carrier_default");
        let local = request
            .get("local")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let replicas = local_replica_count(request);
        let requirements = CarrierAvailabilityRequirements::from_request(request);
        let desired_replicas = requirements
            .min_replicas
            .max(if replicas > 0 { 2 } else { 1 })
            .min(requirements.effective_max());
        let topic_name = content_availability_topic_name(cid);
        let topic_uri = content_availability_topic_uri(cid);
        let announced_at = now_secs();
        let mut state = self.state.lock().await;

        if !state.joined_topics.contains(&topic_name) {
            if state.joined_topics.len() >= MAX_TOPICS {
                return Ok(carrier_availability_error(
                    "too_many_topics",
                    "Carrier availability topic limit reached",
                ));
            }
            if let Err(err) = join_gossip_topic(&mut state, &topic_name, false).await {
                return Ok(carrier_availability_error(
                    "join_failed",
                    format!("Carrier availability topic join failed: {err}"),
                ));
            }
        }

        let node_did = state
            .did
            .clone()
            .unwrap_or_else(|| state.endpoint.id().to_string());
        let existing_messages = {
            let buffers = state.buffers.lock().await;
            buffers
                .get(&topic_name)
                .map(|buffer| buffer.messages.iter().rev().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        };
        let reputation_snapshot = self.peer_reputation.lock().await.clone();
        let remote_candidate_limit =
            carrier_remote_candidate_limit(requirements, replicas, desired_replicas);
        let remote_candidate_pool = content_availability_replicas_with_reputation(
            &existing_messages,
            cid,
            &reputation_snapshot,
        )
        .into_iter()
        .filter(|replica| replica.node_did != node_did)
        .collect::<Vec<_>>();
        let remote_candidate_count = remote_candidate_pool.len();
        let remote_candidate_limit_applied = remote_candidate_count > remote_candidate_limit;
        let remote_candidates = remote_candidate_pool
            .into_iter()
            .take(remote_candidate_limit)
            .collect::<Vec<_>>();
        let fetch_descriptor = if replicas > 0 {
            Some(serde_json::json!({
                "transport": "carrier-file",
                "endpoint_id": state.endpoint.id().to_string(),
                "connect_ticket": carrier_connect_ticket(&state.endpoint),
            }))
        } else {
            None
        };
        let signing_key = match state.signing_key.as_ref() {
            Some(signing_key) => signing_key,
            None => {
                return Ok(carrier_availability_error(
                    "signer_unavailable",
                    "Carrier availability signer unavailable",
                ))
            }
        };
        let mut payload = serde_json::json!({
            "schema": CONTENT_AVAILABILITY_ANNOUNCEMENT_SCHEMA,
            "cid": cid,
            "uri": uri,
            "policy": policy,
            "provider": "carrier-availability",
            "node_did": node_did,
            "topic": topic_uri,
            "local": local,
            "announced_at": announced_at,
        });
        if let Some(fetch_descriptor) = fetch_descriptor {
            payload["fetch"] = fetch_descriptor;
        }
        if let Some(object_did) = request.get("object_did").and_then(|value| value.as_str()) {
            payload["object_did"] = serde_json::Value::String(object_did.to_string());
        }
        if let Some(publisher_did) = request
            .get("publisher_did")
            .and_then(|value| value.as_str())
        {
            payload["publisher_did"] = serde_json::Value::String(publisher_did.to_string());
        }
        let canonical = serde_json::to_string(&payload).map_err(|err| {
            ProviderError::Provider(format!(
                "Carrier availability announcement serialization failed: {err}"
            ))
        })?;
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            signing_key,
            CONTENT_AVAILABILITY_ANNOUNCEMENT_DOMAIN,
            canonical.as_bytes(),
        );
        let peer_attestation_signing_key = signing_key.clone();
        let announcement = serde_json::json!({
            "payload": payload,
            "signature": signature,
            "signer_did": signer_did,
        });
        let content = serde_json::to_string(&announcement).map_err(|err| {
            ProviderError::Provider(format!(
                "Carrier availability announcement envelope failed: {err}"
            ))
        })?;
        let msg = GossipMessage {
            sender_id: signer_did.clone(),
            sender_nick: "content-provider".to_string(),
            content,
            ts: announced_at,
            nonce: random_gossip_nonce(),
            signature: Some(signature.clone()),
            sender_session_id: request
                .get("publisher_did")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        };
        let bytes = serialize_gossip_wire_message(&msg).map_err(|encoded_bytes| {
            ProviderError::Provider(format!(
                "Carrier availability gossip message too large: {encoded_bytes} bytes"
            ))
        })?;

        {
            let mut buffers = state.buffers.lock().await;
            if let Some(buffer) = buffers.get_mut(&topic_name) {
                push_gossip_buffer_message(buffer, msg.clone());
            }
        }

        let delivery = match state.senders.get(&topic_name) {
            Some(sender) => {
                match tokio::time::timeout(GOSSIP_SEND_TIMEOUT, sender.broadcast(bytes)).await {
                    Err(_) => {
                        tracing::debug!("Carrier availability broadcast timed out");
                        "local_only"
                    }
                    Ok(Ok(_)) => "carrier",
                    Ok(Err(err)) => {
                        tracing::debug!("Carrier availability broadcast failed: {}", err);
                        "local_only"
                    }
                }
            }
            None => "local_only",
        };
        drop(state);

        let mut remote_proofs = Vec::new();
        let mut replication_errors = Vec::new();
        let mut attempted_remote_invocations = 0_u32;
        if !remote_candidates.is_empty() {
            match self
                .provider_registry
                .as_ref()
                .and_then(|registry| registry.upgrade())
            {
                Some(registry) => {
                    for candidate in remote_candidates {
                        attempted_remote_invocations =
                            attempted_remote_invocations.saturating_add(1);
                        match ensure_content_via_carrier_provider_invocation(
                            &registry, &candidate, cid, request,
                        )
                        .await
                        {
                            Ok(proof) => {
                                self.record_peer_reputation(&candidate.node_did, true).await;
                                remote_proofs.push(proof)
                            }
                            Err(err) => {
                                self.record_peer_reputation(&candidate.node_did, false)
                                    .await;
                                replication_errors.push(format!("{}: {err}", candidate.node_did))
                            }
                        }
                    }
                }
                None => replication_errors
                    .push("Carrier replication requires Runtime provider registry".to_string()),
            }
        }

        let proven_remote_replicas = remote_proofs.len() as u32;
        let total_replicas = replicas.saturating_add(proven_remote_replicas);
        let live_multi_peer_proof = proven_remote_replicas > 0;
        let peer_attestation_exchange_receipt = self
            .exchange_peer_attestations(CarrierPeerAttestationExchangeRequest {
                signing_key: &peer_attestation_signing_key,
                cid,
                topic_uri: &topic_uri,
                local_node_did: &node_did,
                remote_proofs: &remote_proofs,
                live_multi_peer_proof,
                requested_at: announced_at,
            })
            .await;
        let meets_replica_requirement = total_replicas >= requirements.min_replicas;
        let meets_live_requirement =
            !requirements.require_live_multi_peer_proof || live_multi_peer_proof;
        let status = if meets_replica_requirement && meets_live_requirement && live_multi_peer_proof
        {
            "network_available"
        } else if meets_replica_requirement && meets_live_requirement {
            "carrier_announced"
        } else {
            "repair_needed"
        };
        let repair_scheduled = status == "repair_needed"
            || total_replicas < desired_replicas
            || (requirements.require_live_multi_peer_proof && !live_multi_peer_proof);
        let mut availability = serde_json::json!({
            "status": status,
            "provider": "carrier-availability",
            "policy": policy,
            "replicas": total_replicas,
            "transport": "carrier-gossip",
            "delivery": delivery,
            "topic": topic_uri,
            "peer_selection": carrier_peer_selection_json(
                &topic_uri,
                &node_did,
                replicas,
                &remote_proofs,
                live_multi_peer_proof,
                CarrierPeerAttestationExchangeView {
                    configured: self.peer_attestation_exchange.is_some(),
                    receipt: peer_attestation_exchange_receipt.as_ref(),
                },
            ),
            "quota": carrier_quota_json(requirements, total_replicas, desired_replicas),
            "repair_worker": carrier_repair_worker_json(repair_scheduled, status),
            "repair_graph": carrier_repair_graph_policy_json(requirements),
            "storage_market": carrier_storage_market_policy_json(
                total_replicas,
                live_multi_peer_proof,
            ),
            "abuse_controls": carrier_abuse_controls_json(
                remote_candidate_count,
                remote_candidate_limit,
                attempted_remote_invocations,
                replication_errors.len() as u32,
                remote_candidate_limit_applied,
            ),
            "checked_at": announced_at,
        });
        if status == "repair_needed" {
            availability["reason"] = serde_json::Value::String(carrier_repair_reason(
                requirements,
                total_replicas,
                live_multi_peer_proof,
                &replication_errors,
            ));
        } else if delivery == "local_only" && remote_proofs.is_empty() {
            availability["reason"] = serde_json::Value::String(
                "Carrier announcement was recorded locally; no remote peer delivery was observed"
                    .to_string(),
            );
        }
        Ok(serde_json::json!({
            "status": "ok",
            "data": {
                "availability": availability,
            }
        }))
    }

    async fn fetch_from_announced_carrier_peers(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let cid = match request.get("cid").and_then(|value| value.as_str()) {
            Some(cid) => cid.trim(),
            None => {
                return Ok(carrier_availability_error(
                    "invalid_request",
                    "Carrier availability fetch requires cid",
                ))
            }
        };
        if let Err(err) = validate_content_cid(cid) {
            return Ok(carrier_availability_error("invalid_cid", err));
        }
        let path = request
            .get("path")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if let Err(err) = validate_carrier_content_path(path) {
            return Ok(carrier_availability_error("invalid_path", err));
        }
        let topic_name = content_availability_topic_name(cid);
        let messages = {
            let mut state = self.state.lock().await;
            if !state.joined_topics.contains(&topic_name) {
                if state.joined_topics.len() >= MAX_TOPICS {
                    return Ok(carrier_availability_error(
                        "too_many_topics",
                        "Carrier availability topic limit reached",
                    ));
                }
                if let Err(err) = join_gossip_topic(&mut state, &topic_name, false).await {
                    return Ok(carrier_availability_error(
                        "join_failed",
                        format!("Carrier availability topic join failed: {err}"),
                    ));
                }
            }
            let buffers = state.buffers.clone();
            drop(state);
            let buffers = buffers.lock().await;
            buffers
                .get(&topic_name)
                .map(|buffer| buffer.messages.iter().rev().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        };
        let tickets = content_availability_fetch_tickets(&messages, cid);
        if tickets.is_empty() {
            return Ok(carrier_availability_error(
                "carrier_fetch_unavailable",
                "no Carrier availability announcement with a fetch ticket is available for this CID",
            ));
        }

        let Some(registry) = self
            .provider_registry
            .as_ref()
            .and_then(|registry| registry.upgrade())
        else {
            return Ok(carrier_availability_error(
                "carrier_provider_invocation_unavailable",
                "Carrier availability fetch requires Runtime provider registry",
            ));
        };

        let mut errors = Vec::new();
        for ticket in tickets {
            match fetch_content_via_carrier_provider_invocation(&registry, &ticket, cid, path).await
            {
                Ok((bytes, remote_transfer)) => {
                    return Ok(serde_json::json!({
                        "status": "ok",
                        "data": {
                            "data": base64::engine::general_purpose::STANDARD.encode(bytes),
                            "availability": {
                                "status": "network_available",
                                "provider": "carrier-availability",
                                "policy": "carrier_provider_invoke",
                                "replicas": 1,
                                "transport": "carrier-provider-plane",
                                "remote_transfer": remote_transfer,
                                "checked_at": now_secs(),
                            }
                        }
                    }))
                }
                Err(err) => errors.push(err.to_string()),
            }
        }

        Ok(carrier_availability_error(
            "carrier_fetch_failed",
            format!("Carrier content fetch failed: {}", errors.join(" | ")),
        ))
    }
}

async fn fetch_content_via_carrier_provider_invocation(
    registry: &ProviderRegistry,
    ticket: &str,
    cid: &str,
    path: &str,
) -> Result<(Vec<u8>, Option<serde_json::Value>)> {
    validate_content_cid(cid).map_err(anyhow::Error::msg)?;
    validate_carrier_content_path(path).map_err(anyhow::Error::msg)?;

    let mut request = serde_json::json!({
        "op": "fetch",
        "cid": cid,
        "local_only": true,
        "transfer": "stream",
    });
    if !path.is_empty() {
        request["path"] = serde_json::Value::String(path.to_string());
    }

    let response = registry
        .invoke_provider(ProviderInvocation {
            source: "carrier-availability".to_string(),
            target: "content".to_string(),
            op: "fetch".to_string(),
            request,
            transfer: ProviderTransfer::Stream,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Carrier(ProviderCarrierRoute::ConnectTicket {
                connect_ticket: ticket.to_string(),
                peer_did: None,
                timeout_ms: Some(5_000),
            }),
        })
        .await
        .map_err(|err| anyhow::anyhow!("Carrier provider invocation failed: {err}"))?;

    if response.get("status").and_then(|status| status.as_str()) == Some("error") {
        let message = response
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("unknown provider error");
        anyhow::bail!("remote content provider fetch failed: {message}");
    }
    let remote_transfer = response.get("_runtime_transfer").cloned();
    let bytes = remote_content_provider_response_bytes(&response)?;
    Ok((bytes, remote_transfer))
}

async fn ensure_content_via_carrier_provider_invocation(
    registry: &ProviderRegistry,
    replica: &CarrierAvailabilityReplica,
    cid: &str,
    source_request: &serde_json::Value,
) -> Result<CarrierReplicationProof> {
    validate_content_cid(cid).map_err(anyhow::Error::msg)?;
    let route = ProviderCarrierRoute::ConnectTicket {
        connect_ticket: replica.connect_ticket.clone(),
        peer_did: Some(replica.node_did.clone()),
        timeout_ms: Some(5_000),
    };
    let admission =
        content_admission_via_carrier_provider_invocation(registry, &route, cid, source_request)
            .await?;
    let mut ensure_request = serde_json::json!({
        "op": "ensure",
        "cid": cid,
        "availability_policy": "carrier_replica",
        "availability_requirements": {
            "min_replicas": 1,
            "max_replicas": 1,
            "require_live_multi_peer_proof": false,
        },
    });
    if let Some(object_did) = source_request
        .get("object_did")
        .and_then(|value| value.as_str())
    {
        ensure_request["object_did"] = serde_json::Value::String(object_did.to_string());
    }
    if let Some(publisher_did) = source_request
        .get("publisher_did")
        .and_then(|value| value.as_str())
    {
        ensure_request["publisher_did"] = serde_json::Value::String(publisher_did.to_string());
    }

    let mut ensure_response = registry
        .invoke_provider(ProviderInvocation {
            source: "carrier-availability".to_string(),
            target: "content".to_string(),
            op: "ensure".to_string(),
            request: ensure_request,
            transfer: ProviderTransfer::Json,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Carrier(route.clone()),
        })
        .await
        .map_err(|err| anyhow::anyhow!("remote content ensure failed: {err}"))?;
    if ensure_response
        .get("status")
        .and_then(|value| value.as_str())
        == Some("error")
    {
        let message = ensure_response
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown provider error");
        ensure_response = import_content_via_carrier_provider_invocation(
            registry,
            replica,
            cid,
            source_request,
            Some(message),
        )
        .await?;
    }
    let ensure_status = ensure_response
        .get("data")
        .and_then(|data| data.get("availability"))
        .and_then(|availability| availability.get("status"))
        .and_then(|value| value.as_str())
        .unwrap_or("unknown")
        .to_string();
    if matches!(
        ensure_status.as_str(),
        "repair_needed" | "local_unpinned" | "unknown"
    ) {
        ensure_response = import_content_via_carrier_provider_invocation(
            registry,
            replica,
            cid,
            source_request,
            Some(&ensure_status),
        )
        .await?;
    }
    let ensure_status = ensure_response
        .get("data")
        .and_then(|data| data.get("availability"))
        .and_then(|availability| availability.get("status"))
        .and_then(|value| value.as_str())
        .unwrap_or("unknown")
        .to_string();
    if matches!(
        ensure_status.as_str(),
        "repair_needed" | "local_unpinned" | "unknown"
    ) {
        anyhow::bail!(
            "remote content import/ensure did not prove a pinned replica: {ensure_status}"
        );
    }
    let remote_receipt = remote_content_receipt_summary(&ensure_response, cid)?;

    let status_response = registry
        .invoke_provider(ProviderInvocation {
            source: "carrier-availability".to_string(),
            target: "content".to_string(),
            op: "status".to_string(),
            request: serde_json::json!({
                "op": "status",
                "cid": cid,
            }),
            transfer: ProviderTransfer::Json,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Carrier(route),
        })
        .await
        .map_err(|err| anyhow::anyhow!("remote content status failed: {err}"))?;
    if status_response
        .get("status")
        .and_then(|value| value.as_str())
        == Some("error")
    {
        let message = status_response
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown provider error");
        anyhow::bail!("remote content status returned error: {message}");
    }
    let status_cid = status_response
        .get("data")
        .and_then(|data| data.get("cid"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if status_cid != cid {
        anyhow::bail!("remote content status CID mismatch");
    }
    let status_availability = status_response
        .get("data")
        .and_then(|data| data.get("availability"))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("remote content status missing availability"))?;
    let status = status_availability
        .get("status")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    if matches!(status, "repair_needed" | "local_unpinned" | "unknown") {
        anyhow::bail!("remote content status did not prove a live replica: {status}");
    }

    Ok(CarrierReplicationProof {
        node_did: replica.node_did.clone(),
        endpoint_id: replica.endpoint_id.clone(),
        announced_at: replica.announced_at,
        score: replica.score,
        selection_reason: replica.selection_reason.clone(),
        reputation_score: replica.reputation_score,
        reputation_reason: replica.reputation_reason.clone(),
        ensure_status,
        admission: Some(admission),
        status_availability,
        remote_receipt,
        transfer: status_response.get("_runtime_transfer").cloned(),
        checked_at: now_secs(),
    })
}

async fn content_admission_via_carrier_provider_invocation(
    registry: &ProviderRegistry,
    route: &ProviderCarrierRoute,
    cid: &str,
    source_request: &serde_json::Value,
) -> Result<serde_json::Value> {
    let mut admission_request = serde_json::json!({
        "op": "admission",
        "cid": cid,
        "availability_policy": "carrier_replica",
        "availability_requirements": carrier_source_requirements_json(source_request),
    });
    if let Some(estimated_content_bytes) = carrier_admission_estimated_content_bytes(source_request)
    {
        admission_request["estimated_content_bytes"] =
            serde_json::Value::from(estimated_content_bytes);
    }
    if let Some(accounting) = source_request
        .get("accounting")
        .filter(|value| value.is_object())
    {
        admission_request["accounting"] = accounting.clone();
    }
    if let Some(object_did) = source_request
        .get("object_did")
        .and_then(|value| value.as_str())
    {
        admission_request["object_did"] = serde_json::Value::String(object_did.to_string());
    }
    if let Some(publisher_did) = source_request
        .get("publisher_did")
        .and_then(|value| value.as_str())
    {
        admission_request["publisher_did"] = serde_json::Value::String(publisher_did.to_string());
    }

    let response = registry
        .invoke_provider(ProviderInvocation {
            source: "carrier-availability".to_string(),
            target: "content".to_string(),
            op: "admission".to_string(),
            request: admission_request,
            transfer: ProviderTransfer::Json,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Carrier(route.clone()),
        })
        .await
        .map_err(|err| anyhow::anyhow!("remote content admission failed: {err}"))?;
    if response.get("status").and_then(|value| value.as_str()) == Some("error") {
        let message = response
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown provider error");
        anyhow::bail!("remote content admission returned error: {message}");
    }
    let mut admission = response
        .get("data")
        .and_then(|data| data.get("admission"))
        .filter(|value| value.is_object())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("remote content admission missing admission receipt"))?;
    let receipt_summary = remote_content_admission_receipt_summary(&response, &admission, cid)?;
    if let Some(admission) = admission.as_object_mut() {
        admission.insert("receipt".to_string(), receipt_summary);
    }
    if admission.get("accepted").and_then(|value| value.as_bool()) != Some(true) {
        let status = admission
            .get("status")
            .and_then(|value| value.as_str())
            .unwrap_or("rejected");
        let reason = admission
            .get("reason")
            .and_then(|value| value.as_str())
            .unwrap_or("remote content provider rejected admission");
        anyhow::bail!("remote content admission rejected: {status}: {reason}");
    }
    Ok(admission)
}

fn remote_content_admission_receipt_summary(
    response: &serde_json::Value,
    admission: &serde_json::Value,
    cid: &str,
) -> Result<serde_json::Value> {
    let receipt = response
        .get("data")
        .and_then(|data| data.get("receipt"))
        .filter(|value| value.is_object())
        .ok_or_else(|| anyhow::anyhow!("remote content admission missing signed receipt"))?;
    let payload = receipt
        .get("payload")
        .ok_or_else(|| anyhow::anyhow!("remote content admission receipt missing payload"))?;
    if payload != admission {
        anyhow::bail!("remote content admission receipt payload mismatch");
    }
    let payload_cid = payload
        .get("cid")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if payload_cid != cid {
        anyhow::bail!("remote content admission receipt CID mismatch");
    }
    let signer_did = receipt
        .get("signer_did")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("remote content admission receipt missing signer_did"))?;
    let receipt_bytes = serde_json::to_vec(receipt)
        .map_err(|err| anyhow::anyhow!("remote content admission receipt encode failed: {err}"))?;
    let expected_signers = [signer_did.to_string()];
    crate::crypto::verify_signed_json_envelope_against_dids(
        &receipt_bytes,
        CONTENT_ADMISSION_DOMAIN,
        &expected_signers,
    )
    .map_err(|err| {
        anyhow::anyhow!("remote content admission receipt verification failed: {err}")
    })?;
    Ok(serde_json::json!({
        "schema": payload
            .get("schema")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "signer_did": signer_did,
        "verified": true,
    }))
}

fn carrier_source_requirements_json(source_request: &serde_json::Value) -> serde_json::Value {
    source_request
        .get("requirements")
        .or_else(|| source_request.get("availability_requirements"))
        .or_else(|| source_request.get("replication_requirements"))
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| {
            serde_json::json!({
                "min_replicas": 1,
                "max_replicas": 1,
                "require_live_multi_peer_proof": false,
            })
        })
}

fn carrier_admission_estimated_content_bytes(source_request: &serde_json::Value) -> Option<u64> {
    ["estimated_content_bytes", "incoming_content_bytes"]
        .into_iter()
        .find_map(|field| source_request.get(field).and_then(|value| value.as_u64()))
        .or_else(|| {
            source_request
                .get("accounting")
                .and_then(|accounting| accounting.get("content_bytes"))
                .and_then(|value| value.as_u64())
        })
        .or_else(|| {
            source_request
                .get("local")
                .and_then(|local| local.get("accounting"))
                .and_then(|accounting| accounting.get("content_bytes"))
                .and_then(|value| value.as_u64())
        })
}

fn remote_content_receipt_summary(
    response: &serde_json::Value,
    cid: &str,
) -> Result<Option<serde_json::Value>> {
    let Some(receipt) = response.get("data").and_then(|data| data.get("receipt")) else {
        return Ok(None);
    };
    let payload = receipt
        .get("payload")
        .ok_or_else(|| anyhow::anyhow!("remote content receipt missing payload"))?;
    let receipt_cid = payload
        .get("cid")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if receipt_cid != cid {
        anyhow::bail!("remote content receipt CID mismatch");
    }
    let signer_did = receipt
        .get("signer_did")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("remote content receipt missing signer_did"))?;
    let receipt_bytes = serde_json::to_vec(receipt)
        .map_err(|err| anyhow::anyhow!("remote content receipt encode failed: {err}"))?;
    let expected_signers = [signer_did.to_string()];
    crate::crypto::verify_signed_json_envelope_against_dids(
        &receipt_bytes,
        "elastos.content.availability.receipt.v1",
        &expected_signers,
    )
    .map_err(|err| anyhow::anyhow!("remote content receipt verification failed: {err}"))?;
    Ok(Some(serde_json::json!({
        "schema": payload.get("schema").cloned().unwrap_or(serde_json::Value::Null),
        "cid": receipt_cid,
        "status": payload.get("status").cloned().unwrap_or(serde_json::Value::Null),
        "provider": payload.get("provider").cloned().unwrap_or(serde_json::Value::Null),
        "policy": payload.get("policy").cloned().unwrap_or(serde_json::Value::Null),
        "replicas": payload.get("replicas").cloned().unwrap_or(serde_json::Value::Null),
        "peer_selection": remote_content_receipt_peer_selection_summary(payload.get("peer_selection")),
        "quota": remote_content_receipt_quota_summary(payload.get("quota")),
        "repair_worker": remote_content_receipt_repair_worker_summary(payload.get("repair_worker")),
        "repair_graph": remote_content_receipt_repair_graph_summary(payload.get("repair_graph")),
        "storage_market": remote_content_receipt_storage_market_summary(payload.get("storage_market")),
        "accounting": remote_content_receipt_accounting_summary(payload.get("accounting")),
        "abuse_controls": remote_content_receipt_abuse_controls_summary(payload.get("abuse_controls")),
        "checked_at": payload.get("checked_at").cloned().unwrap_or(serde_json::Value::Null),
        "signer_did": signer_did,
        "verified": true,
    })))
}

fn remote_content_receipt_peer_selection_summary(
    peer_selection: Option<&serde_json::Value>,
) -> serde_json::Value {
    let Some(peer_selection) = peer_selection else {
        return serde_json::json!({"mode": "unknown"});
    };
    let replicas = remote_content_receipt_peer_selection_replicas_summary(peer_selection);
    let replica_count = peer_selection
        .get("replicas")
        .and_then(|value| value.as_array())
        .map(|replicas| replicas.len())
        .unwrap_or(0);
    let remote_replicas = peer_selection
        .get("replicas")
        .and_then(|value| value.as_array())
        .map(|replicas| {
            replicas
                .iter()
                .filter(|replica| {
                    replica.get("role").and_then(|value| value.as_str()) == Some("remote")
                })
                .count()
        })
        .unwrap_or(0);
    serde_json::json!({
        "mode": peer_selection
            .get("mode")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "strategy": peer_selection
            .get("strategy")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "live_multi_peer_proof": peer_selection
            .get("live_multi_peer_proof")
            .cloned()
            .unwrap_or(serde_json::Value::Bool(false)),
        "peer_reputation_policy": peer_selection
            .get("peer_reputation_policy")
            .cloned()
            .unwrap_or_else(default_carrier_peer_reputation_policy_json),
        "peer_attestation_exchange_policy": peer_selection
            .get("peer_attestation_exchange_policy")
            .cloned()
            .unwrap_or_else(default_carrier_peer_attestation_exchange_policy_json),
        "replica_count": replica_count,
        "remote_replicas": remote_replicas,
        "replica_summary_limit": MAX_REMOTE_RECEIPT_REPLICA_SUMMARY_ROWS,
        "replicas_truncated": replica_count > replicas.len(),
        "replicas": replicas,
    })
}

fn default_carrier_peer_reputation_policy_json() -> serde_json::Value {
    serde_json::json!({
        "schema": CARRIER_PEER_REPUTATION_SCHEMA,
        "policy": "not_reported",
        "scope": "content-availability",
        "status": "not_reported",
        "federation": {
            "configured": false,
            "cross_runtime_reputation": false,
        },
    })
}

fn default_carrier_peer_attestation_exchange_policy_json() -> serde_json::Value {
    serde_json::json!({
        "schema": CARRIER_PEER_ATTESTATION_EXCHANGE_POLICY_SCHEMA,
        "policy": "not_reported",
        "scope": "content-availability",
        "status": "not_reported",
        "attestation_exchange": {
            "configured": false,
            "signed_reputation_receipts": false,
            "third_party_attestations": false,
            "cross_runtime_trust_policy": false,
        },
    })
}

fn remote_content_receipt_peer_selection_replicas_summary(
    peer_selection: &serde_json::Value,
) -> Vec<serde_json::Value> {
    peer_selection
        .get("replicas")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .take(MAX_REMOTE_RECEIPT_REPLICA_SUMMARY_ROWS)
        .map(|replica| {
            serde_json::json!({
                "role": replica
                    .get("role")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                "node_did": replica
                    .get("node_did")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                "endpoint_id": replica
                    .get("endpoint_id")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                "score": replica
                    .get("score")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                "selection_reason": replica
                    .get("selection_reason")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                "local_reputation": replica
                    .get("local_reputation")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                "status": replica
                    .get("status")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            })
        })
        .collect()
}

fn remote_content_receipt_quota_summary(quota: Option<&serde_json::Value>) -> serde_json::Value {
    let Some(quota) = quota else {
        return serde_json::json!({"policy": "unknown"});
    };
    serde_json::json!({
        "policy": quota.get("policy").cloned().unwrap_or(serde_json::Value::Null),
        "status": quota.get("status").cloned().unwrap_or(serde_json::Value::Null),
        "enforced": quota
            .get("enforced")
            .cloned()
            .unwrap_or(serde_json::Value::Bool(false)),
        "used_replicas": quota
            .get("used_replicas")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "effective_max_replicas": quota
            .get("effective_max_replicas")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "federated_quota_ledger_policy": quota
            .get("federated_quota_ledger_policy")
            .cloned()
            .unwrap_or_else(default_carrier_federated_quota_ledger_policy_json),
    })
}

fn remote_content_receipt_repair_worker_summary(
    repair_worker: Option<&serde_json::Value>,
) -> serde_json::Value {
    let Some(repair_worker) = repair_worker else {
        return serde_json::json!({"status": "unknown"});
    };
    serde_json::json!({
        "scheduled": repair_worker
            .get("scheduled")
            .cloned()
            .unwrap_or(serde_json::Value::Bool(false)),
        "status": repair_worker
            .get("status")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "worker": repair_worker
            .get("worker")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    })
}

fn remote_content_receipt_storage_market_summary(
    storage_market: Option<&serde_json::Value>,
) -> serde_json::Value {
    let Some(storage_market) = storage_market else {
        return serde_json::json!({
            "schema": "elastos.content.storage-market/v1",
            "status": "not_reported",
            "settlement": "not_configured",
            "admission_policy": default_carrier_storage_market_admission_policy_json(),
        });
    };
    serde_json::json!({
        "schema": storage_market
            .get("schema")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::String("elastos.content.storage-market/v1".to_string())),
        "mode": storage_market
            .get("mode")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "status": storage_market
            .get("status")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "settlement": storage_market
            .get("settlement")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::String("not_configured".to_string())),
        "escrow": storage_market
            .get("escrow")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::String("not_configured".to_string())),
        "quota_enforced": storage_market
            .get("quota_enforced")
            .cloned()
            .unwrap_or(serde_json::Value::Bool(false)),
        "admission_policy": storage_market
            .get("admission_policy")
            .cloned()
            .unwrap_or_else(default_carrier_storage_market_admission_policy_json),
        "settlement_policy": storage_market
            .get("settlement_policy")
            .cloned()
            .unwrap_or_else(default_carrier_storage_settlement_policy_json),
    })
}

fn carrier_storage_market_admission_policy_json(
    mode: &str,
    market_status: &str,
    quota_enforced: bool,
    live_multi_peer_proof: bool,
    remote_admission_preflight: bool,
) -> serde_json::Value {
    serde_json::json!({
        "schema": CONTENT_STORAGE_MARKET_ADMISSION_POLICY_SCHEMA,
        "policy": "proof_path_admission_no_production_market",
        "scope": "content-availability",
        "status": if remote_admission_preflight {
            "remote_admission_preflight_no_market_admission"
        } else if quota_enforced {
            "local_quota_admission_no_market_admission"
        } else {
            "production_storage_market_admission_not_configured"
        },
        "market": {
            "mode": mode,
            "status": market_status,
            "quota_enforced": quota_enforced,
            "live_multi_peer_proof": live_multi_peer_proof,
        },
        "current_admission": {
            "local_principal_quota_ledger": quota_enforced,
            "remote_content_admission_preflight": remote_admission_preflight,
            "signed_admission_receipts": remote_admission_preflight,
            "content_admission_schema": "elastos.content.admission/v1",
            "content_admission_receipt_domain": CONTENT_ADMISSION_DOMAIN,
            "provider_invocation_required": true,
            "signed_availability_receipts": true,
        },
        "production_market": {
            "configured": false,
            "provider_admission_network": false,
            "provider_offer_receipts": false,
            "price_discovery": false,
            "sla_admission": false,
            "abuse_economic_controls": false,
            "reason": "Carrier verifies signed remote content/admission receipts in this branch; production storage-market admission needs provider offers, pricing, SLA, and trust policy receipts",
        },
    })
}

fn default_carrier_storage_market_admission_policy_json() -> serde_json::Value {
    carrier_storage_market_admission_policy_json(
        "not_reported",
        "not_reported",
        false,
        false,
        false,
    )
}

fn carrier_storage_settlement_policy_json(
    mode: &str,
    market_status: &str,
    quota_enforced: bool,
    live_multi_peer_proof: bool,
) -> serde_json::Value {
    serde_json::json!({
        "schema": "elastos.content.storage-settlement-policy/v1",
        "policy": "no_settlement_receipt_policy",
        "scope": "content-availability",
        "status": "settlement_not_configured",
        "market": {
            "mode": mode,
            "status": market_status,
            "quota_enforced": quota_enforced,
            "live_multi_peer_proof": live_multi_peer_proof,
        },
        "settlement": {
            "pricing": "not_configured",
            "escrow": "not_configured",
            "payment_settlement": "not_configured",
            "sla_enforcement": "not_configured",
        },
        "production_federation": {
            "configured": false,
            "storage_market_admission": false,
            "cross_provider_escrow": false,
            "settlement_receipts": false,
            "reason": "Carrier can prove provider replicas in this branch; pricing, escrow, settlement, and SLA policy require production storage-market providers",
        },
    })
}

fn default_carrier_storage_settlement_policy_json() -> serde_json::Value {
    carrier_storage_settlement_policy_json("not_reported", "not_reported", false, false)
}

fn remote_content_receipt_repair_graph_summary(
    repair_graph: Option<&serde_json::Value>,
) -> serde_json::Value {
    let Some(repair_graph) = repair_graph else {
        return serde_json::json!({
            "schema": CONTENT_REPAIR_GRAPH_SCHEMA,
            "status": "not_reported",
        });
    };
    serde_json::json!({
        "schema": repair_graph
            .get("schema")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::String(CONTENT_REPAIR_GRAPH_SCHEMA.to_string())),
        "policy": repair_graph
            .get("policy")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "requested_kind": repair_graph
            .get("requested_kind")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "status": repair_graph
            .get("status")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "refuses_exact_fallback_for_arbitrary_dag": repair_graph
            .get("refuses_exact_fallback_for_arbitrary_dag")
            .cloned()
            .unwrap_or(serde_json::Value::Bool(false)),
    })
}

fn remote_content_receipt_accounting_summary(
    accounting: Option<&serde_json::Value>,
) -> serde_json::Value {
    let Some(accounting) = accounting else {
        return serde_json::json!({"observed": false});
    };
    serde_json::json!({
        "schema": accounting
            .get("schema")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "observed": accounting
            .get("observed")
            .cloned()
            .unwrap_or(serde_json::Value::Bool(false)),
        "files": accounting
            .get("files")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "content_bytes": accounting
            .get("content_bytes")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "replica_bytes_estimate": accounting
            .get("replica_bytes_estimate")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "storage_quota_status": accounting
            .get("storage_quota")
            .and_then(|quota| quota.get("status"))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    })
}

fn remote_content_receipt_abuse_controls_summary(
    abuse_controls: Option<&serde_json::Value>,
) -> serde_json::Value {
    let Some(abuse_controls) = abuse_controls else {
        return serde_json::json!({"policy": "unknown", "enforced": false});
    };
    serde_json::json!({
        "schema": abuse_controls
            .get("schema")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "policy": abuse_controls
            .get("policy")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "enforced": abuse_controls
            .get("enforced")
            .cloned()
            .unwrap_or(serde_json::Value::Bool(false)),
        "candidate_count": abuse_controls
            .get("candidate_count")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "attempt_limit": abuse_controls
            .get("attempt_limit")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "attempted_operations": abuse_controls
            .get("attempted_operations")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "failed_operations": abuse_controls
            .get("failed_operations")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "throttled": abuse_controls
            .get("throttled")
            .cloned()
            .unwrap_or(serde_json::Value::Bool(false)),
    })
}

async fn import_content_via_carrier_provider_invocation(
    registry: &ProviderRegistry,
    replica: &CarrierAvailabilityReplica,
    cid: &str,
    source_request: &serde_json::Value,
    ensure_failure: Option<&str>,
) -> Result<serde_json::Value> {
    let requirements = CarrierAvailabilityRequirements::from_request(source_request);
    if !requirements
        .repair_graph_kind
        .supports_current_import_fallback()
    {
        return import_ipld_dag_content_via_carrier_block_graph_provider(
            registry,
            replica,
            cid,
            source_request,
            ensure_failure,
            requirements,
        )
        .await;
    }
    match import_object_content_via_carrier_provider_invocation(
        registry,
        replica,
        cid,
        source_request,
        ensure_failure,
    )
    .await
    {
        Ok(response) => Ok(response),
        Err(object_err) => import_exact_content_via_carrier_provider_invocation(
            registry,
            replica,
            cid,
            source_request,
            ensure_failure,
        )
        .await
        .map_err(|exact_err| {
            anyhow::anyhow!(
                "remote content object import failed: {object_err}; exact import fallback failed: {exact_err}"
            )
        }),
    }
}

async fn import_ipld_dag_content_via_carrier_block_graph_provider(
    registry: &ProviderRegistry,
    replica: &CarrierAvailabilityReplica,
    cid: &str,
    source_request: &serde_json::Value,
    ensure_failure: Option<&str>,
    requirements: CarrierAvailabilityRequirements,
) -> Result<serde_json::Value> {
    let mut export_request = serde_json::json!({
        "op": "export_graph",
        "cid": cid,
        "schema": CONTENT_BLOCK_GRAPH_SCHEMA,
        "repair_graph_kind": requirements.repair_graph_kind.as_str(),
        "availability_requirements": requirements.to_json(),
        "policy": "carrier_block_graph_repair",
    });
    copy_optional_content_identity(source_request, &mut export_request);

    let export_response = registry
        .invoke_provider(ProviderInvocation {
            source: "carrier-availability".to_string(),
            target: CONTENT_BLOCK_GRAPH_TARGET.to_string(),
            op: "export_graph".to_string(),
            request: export_request,
            transfer: ProviderTransfer::Json,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Local,
        })
        .await
        .map_err(|err| {
            anyhow::anyhow!(
                "local block-graph export failed for arbitrary DAG repair: {err}; Carrier refused object/exact fallback"
            )
        })?;
    ensure_provider_ok(&export_response, "local block-graph export")?;
    let graph = exported_block_graph(&export_response, cid)?;

    let route = ProviderCarrierRoute::ConnectTicket {
        connect_ticket: replica.connect_ticket.clone(),
        peer_did: Some(replica.node_did.clone()),
        timeout_ms: Some(5_000),
    };
    let mut import_request = serde_json::json!({
        "op": "import_graph",
        "cid": cid,
        "graph": graph,
        "availability_policy": "carrier_block_graph_import",
        "availability_requirements": requirements.to_json(),
        "ensure_failure": ensure_failure,
    });
    copy_optional_content_identity(source_request, &mut import_request);

    let import_response = registry
        .invoke_provider(ProviderInvocation {
            source: "carrier-availability".to_string(),
            target: CONTENT_BLOCK_GRAPH_TARGET.to_string(),
            op: "import_graph".to_string(),
            request: import_request,
            transfer: ProviderTransfer::Json,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Carrier(route),
        })
        .await
        .map_err(|err| anyhow::anyhow!("remote block-graph import failed: {err}"))?;
    ensure_provider_ok(&import_response, "remote block-graph import")?;

    let mut ensure_request = serde_json::json!({
        "op": "ensure",
        "cid": cid,
        "availability_policy": "carrier_block_graph_import",
        "availability_requirements": requirements.to_json(),
    });
    copy_optional_content_identity(source_request, &mut ensure_request);
    let route = ProviderCarrierRoute::ConnectTicket {
        connect_ticket: replica.connect_ticket.clone(),
        peer_did: Some(replica.node_did.clone()),
        timeout_ms: Some(5_000),
    };
    let ensure_response = registry
        .invoke_provider(ProviderInvocation {
            source: "carrier-availability".to_string(),
            target: "content".to_string(),
            op: "ensure".to_string(),
            request: ensure_request,
            transfer: ProviderTransfer::Json,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Carrier(route),
        })
        .await
        .map_err(|err| {
            anyhow::anyhow!("remote content ensure after block-graph import failed: {err}")
        })?;
    ensure_provider_ok(
        &ensure_response,
        "remote content ensure after block-graph import",
    )?;
    Ok(ensure_response)
}

fn ensure_provider_ok(response: &serde_json::Value, label: &str) -> Result<()> {
    if response.get("status").and_then(|status| status.as_str()) == Some("error") {
        let message = response
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("unknown provider error");
        anyhow::bail!("{label} returned error: {message}");
    }
    Ok(())
}

fn exported_block_graph(response: &serde_json::Value, cid: &str) -> Result<serde_json::Value> {
    let graph = response
        .get("data")
        .and_then(|data| data.get("graph"))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("local block-graph export missing data.graph"))?;
    if graph.get("schema").and_then(|value| value.as_str()) != Some(CONTENT_BLOCK_GRAPH_SCHEMA) {
        anyhow::bail!("local block-graph export returned unsupported graph schema");
    }
    let root_cid = graph
        .get("root_cid")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if root_cid != cid {
        anyhow::bail!("local block-graph export root CID mismatch");
    }
    Ok(graph)
}

fn copy_optional_content_identity(source: &serde_json::Value, target: &mut serde_json::Value) {
    for key in ["object_did", "publisher_did"] {
        if let Some(value) = source.get(key).cloned() {
            target[key] = value;
        }
    }
}

async fn local_content_fetch_bytes_for_import(
    registry: &ProviderRegistry,
    cid: &str,
    path: Option<&str>,
) -> Result<Vec<u8>> {
    let mut request = serde_json::json!({
        "op": "fetch",
        "cid": cid,
        "local_only": true,
        "transfer": "stream",
    });
    if let Some(path) = path.filter(|path| !path.is_empty()) {
        request["path"] = serde_json::Value::String(path.to_string());
    }
    let response = registry
        .invoke_provider(ProviderInvocation {
            source: "carrier-availability".to_string(),
            target: "content".to_string(),
            op: "fetch".to_string(),
            request,
            transfer: ProviderTransfer::Stream,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Local,
        })
        .await
        .map_err(|err| anyhow::anyhow!("local content fetch for object import failed: {err}"))?;
    if response.get("status").and_then(|value| value.as_str()) == Some("error") {
        let message = response
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown provider error");
        anyhow::bail!("local content fetch for object import returned error: {message}");
    }
    remote_content_provider_response_bytes(&response)
}

async fn import_object_content_via_carrier_provider_invocation(
    registry: &ProviderRegistry,
    replica: &CarrierAvailabilityReplica,
    cid: &str,
    source_request: &serde_json::Value,
    ensure_failure: Option<&str>,
) -> Result<serde_json::Value> {
    let manifest_bytes =
        local_content_fetch_bytes_for_import(registry, cid, Some(CONTENT_OBJECT_MANIFEST_PATH))
            .await?;
    let manifest: ContentObjectManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|err| {
            anyhow::anyhow!("local content object manifest decode failed for {cid}: {err}")
        })?;
    if manifest.files.is_empty() {
        anyhow::bail!("local content object manifest has no files");
    }
    if manifest.files.len() > MAX_CARRIER_OBJECT_IMPORT_FILES {
        anyhow::bail!(
            "local content object manifest exceeds {} files",
            MAX_CARRIER_OBJECT_IMPORT_FILES
        );
    }
    let mut files = Vec::with_capacity(manifest.files.len());
    let mut total_bytes = 0_usize;
    for file in &manifest.files {
        let bytes = local_content_fetch_bytes_for_import(registry, cid, Some(&file.path)).await?;
        if bytes.len() as u64 != file.size {
            anyhow::bail!(
                "local content object file {} size mismatch: manifest {}, fetched {}",
                file.path,
                file.size,
                bytes.len()
            );
        }
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        if sha256 != file.sha256 {
            anyhow::bail!(
                "local content object file {} digest mismatch: manifest {}, fetched {}",
                file.path,
                file.sha256,
                sha256
            );
        }
        total_bytes = total_bytes.saturating_add(bytes.len());
        if total_bytes > MAX_CARRIER_OBJECT_IMPORT_BYTES {
            anyhow::bail!(
                "local content object import exceeds {} bytes",
                MAX_CARRIER_OBJECT_IMPORT_BYTES
            );
        }
        files.push(serde_json::json!({
            "path": file.path.clone(),
            "data": base64::engine::general_purpose::STANDARD.encode(bytes),
        }));
    }
    let file_count = files.len();
    let mut import_request = serde_json::json!({
        "op": "import_object",
        "cid": cid,
        "object_kind": manifest.kind.clone(),
        "files": files,
    });
    if let Some(reason) = ensure_failure {
        import_request["ensure_failure"] = serde_json::Value::String(reason.to_string());
    }
    if !manifest.links.is_empty() {
        import_request["links"] = serde_json::to_value(&manifest.links)
            .map_err(|err| anyhow::anyhow!("content object links encode failed: {err}"))?;
    }
    if let Some(object_did) = manifest.object_did.or_else(|| {
        source_request
            .get("object_did")
            .and_then(|value| value.as_str())
            .map(str::to_string)
    }) {
        import_request["object_did"] = serde_json::Value::String(object_did);
    }
    if let Some(publisher_did) = manifest.publisher_did.or_else(|| {
        source_request
            .get("publisher_did")
            .and_then(|value| value.as_str())
            .map(str::to_string)
    }) {
        import_request["publisher_did"] = serde_json::Value::String(publisher_did);
    }
    import_request["import_summary"] = serde_json::json!({
        "schema": "elastos.content.import-object.request-summary/v1",
        "files": file_count,
        "bytes": total_bytes,
        "source": "local-object-manifest",
    });

    let response = registry
        .invoke_provider(ProviderInvocation {
            source: "carrier-availability".to_string(),
            target: "content".to_string(),
            op: "import_object".to_string(),
            request: import_request,
            transfer: ProviderTransfer::Json,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Carrier(ProviderCarrierRoute::ConnectTicket {
                connect_ticket: replica.connect_ticket.clone(),
                peer_did: Some(replica.node_did.clone()),
                timeout_ms: Some(5_000),
            }),
        })
        .await
        .map_err(|err| anyhow::anyhow!("remote content object import failed: {err}"))?;
    if response.get("status").and_then(|value| value.as_str()) == Some("error") {
        let message = response
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown provider error");
        anyhow::bail!("remote content object import returned error: {message}");
    }
    Ok(response)
}

async fn import_exact_content_via_carrier_provider_invocation(
    registry: &ProviderRegistry,
    replica: &CarrierAvailabilityReplica,
    cid: &str,
    source_request: &serde_json::Value,
    ensure_failure: Option<&str>,
) -> Result<serde_json::Value> {
    let local_fetch = registry
        .invoke_provider(ProviderInvocation {
            source: "carrier-availability".to_string(),
            target: "content".to_string(),
            op: "fetch".to_string(),
            request: serde_json::json!({
                "op": "fetch",
                "cid": cid,
                "local_only": true,
                "transfer": "stream",
            }),
            transfer: ProviderTransfer::Stream,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Local,
        })
        .await
        .map_err(|err| anyhow::anyhow!("local content fetch for exact import failed: {err}"))?;
    if local_fetch.get("status").and_then(|value| value.as_str()) == Some("error") {
        let message = local_fetch
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown provider error");
        anyhow::bail!("local content fetch for exact import returned error: {message}");
    }
    let stream = local_fetch
        .get("data")
        .and_then(|data| data.get("stream"))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("local content fetch missing stream payload"))?;
    let mut import_request = serde_json::json!({
        "op": "import_exact",
        "cid": cid,
        "stream": stream,
        "filename": "content.bin",
    });
    if let Some(reason) = ensure_failure {
        import_request["ensure_failure"] = serde_json::Value::String(reason.to_string());
    }
    if let Some(object_did) = source_request
        .get("object_did")
        .and_then(|value| value.as_str())
    {
        import_request["object_did"] = serde_json::Value::String(object_did.to_string());
    }
    if let Some(publisher_did) = source_request
        .get("publisher_did")
        .and_then(|value| value.as_str())
    {
        import_request["publisher_did"] = serde_json::Value::String(publisher_did.to_string());
    }

    let response = registry
        .invoke_provider(ProviderInvocation {
            source: "carrier-availability".to_string(),
            target: "content".to_string(),
            op: "import_exact".to_string(),
            request: import_request,
            transfer: ProviderTransfer::Json,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Carrier(ProviderCarrierRoute::ConnectTicket {
                connect_ticket: replica.connect_ticket.clone(),
                peer_did: Some(replica.node_did.clone()),
                timeout_ms: Some(5_000),
            }),
        })
        .await
        .map_err(|err| anyhow::anyhow!("remote content exact import failed: {err}"))?;
    if response.get("status").and_then(|value| value.as_str()) == Some("error") {
        let message = response
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown provider error");
        anyhow::bail!("remote content exact import returned error: {message}");
    }
    Ok(response)
}

fn remote_content_provider_response_bytes(response: &serde_json::Value) -> Result<Vec<u8>> {
    let data = response
        .get("data")
        .and_then(|data| data.as_object())
        .ok_or_else(|| anyhow::anyhow!("remote content provider response missing data"))?;
    if let Some(stream) = data.get("stream") {
        return decode_carrier_provider_stream_payload(stream);
    }
    let data_value = data
        .get("data")
        .ok_or_else(|| anyhow::anyhow!("remote content provider response missing data"))?;
    let encoded = data_value
        .as_str()
        .or_else(|| data_value.get("data").and_then(|value| value.as_str()))
        .ok_or_else(|| anyhow::anyhow!("remote content provider response missing base64 data"))?;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|err| anyhow::anyhow!("remote content provider returned invalid base64: {err}"))
}

fn decode_carrier_provider_stream_payload(stream: &serde_json::Value) -> Result<Vec<u8>> {
    let object = stream
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("remote content provider stream must be an object"))?;
    let schema = object
        .get("schema")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if schema != "elastos.provider.stream/v1" {
        anyhow::bail!(
            "remote content provider stream schema mismatch: expected elastos.provider.stream/v1, got {schema}"
        );
    }
    let encoding = object
        .get("encoding")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if encoding != "base64-chunks" {
        anyhow::bail!(
            "remote content provider stream encoding mismatch: expected base64-chunks, got {encoding}"
        );
    }
    let chunks = object
        .get("chunks")
        .and_then(|value| value.as_array())
        .ok_or_else(|| anyhow::anyhow!("remote content provider stream missing chunks"))?;
    let mut bytes = Vec::new();
    for (expected_index, chunk) in chunks.iter().enumerate() {
        let chunk = chunk.as_object().ok_or_else(|| {
            anyhow::anyhow!("remote content provider stream chunk must be an object")
        })?;
        let index = chunk
            .get("index")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| anyhow::anyhow!("remote content provider stream chunk missing index"))?;
        if index != expected_index as u64 {
            anyhow::bail!(
                "remote content provider stream chunk index mismatch: expected {expected_index}, got {index}"
            );
        }
        let offset = chunk
            .get("offset")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| {
                anyhow::anyhow!("remote content provider stream chunk missing offset")
            })?;
        if offset != bytes.len() as u64 {
            anyhow::bail!(
                "remote content provider stream chunk {index} offset mismatch: expected {}, got {offset}",
                bytes.len()
            );
        }
        let encoded = chunk
            .get("data")
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow::anyhow!("remote content provider stream chunk missing data"))?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|err| {
                anyhow::anyhow!("remote content provider stream chunk has invalid base64: {err}")
            })?;
        if let Some(length) = chunk.get("length").and_then(|value| value.as_u64()) {
            if length != decoded.len() as u64 {
                anyhow::bail!(
                    "remote content provider stream chunk {index} length {length} does not match decoded length {}",
                    decoded.len()
                );
            }
        }
        bytes.extend_from_slice(&decoded);
    }
    if let Some(total_bytes) = object.get("total_bytes").and_then(|value| value.as_u64()) {
        if total_bytes != bytes.len() as u64 {
            anyhow::bail!(
                "remote content provider stream total_bytes {total_bytes} does not match decoded length {}",
                bytes.len()
            );
        }
    }
    Ok(bytes)
}

fn carrier_connect_ticket(endpoint: &Endpoint) -> String {
    let mut watcher = endpoint.watch_addr();
    let addr = watcher.get();
    let ticket_json = serde_json::json!({
        "topic": null,
        "endpoints": [addr],
    });
    let ticket_bytes = serde_json::to_vec(&ticket_json).unwrap_or_default();
    let mut ticket_str = data_encoding::BASE32_NOPAD.encode(&ticket_bytes);
    ticket_str.make_ascii_lowercase();
    ticket_str
}

fn carrier_peer_selection_json(
    topic_uri: &str,
    local_node_did: &str,
    local_replicas: u32,
    remote_proofs: &[CarrierReplicationProof],
    live_multi_peer_proof: bool,
    peer_attestation_exchange: CarrierPeerAttestationExchangeView<'_>,
) -> serde_json::Value {
    let mut replicas = Vec::new();
    if local_replicas > 0 {
        replicas.push(serde_json::json!({
            "role": "local",
            "node_did": local_node_did,
            "status": "local_pinned",
        }));
    }
    replicas.extend(remote_proofs.iter().map(|proof| {
        serde_json::json!({
            "role": "remote",
            "node_did": proof.node_did.clone(),
            "endpoint_id": proof.endpoint_id.clone(),
            "announced_at": proof.announced_at,
            "score": proof.score,
            "selection_reason": proof.selection_reason.clone(),
            "local_reputation": {
                "scope": "local_runtime",
                "score_delta": proof.reputation_score,
                "reason": proof.reputation_reason.clone(),
            },
            "admission": proof.admission.clone(),
            "ensure_status": proof.ensure_status.clone(),
            "status": proof
                .status_availability
                .get("status")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown"),
            "remote_receipt": proof.remote_receipt.clone(),
            "transfer": proof.transfer.clone(),
            "checked_at": proof.checked_at,
        })
    }));
    serde_json::json!({
        "mode": if live_multi_peer_proof {
            "carrier_provider_replication"
        } else {
            "carrier_topic"
        },
        "strategy": "signed_announcement_then_provider_invoke",
        "topic": topic_uri,
        "live_multi_peer_proof": live_multi_peer_proof,
        "peer_reputation_policy": carrier_peer_reputation_policy_json(
            remote_proofs,
            live_multi_peer_proof,
        ),
        "peer_attestation_exchange_policy": carrier_peer_attestation_exchange_policy_json(
            remote_proofs,
            live_multi_peer_proof,
            peer_attestation_exchange,
        ),
        "replicas": replicas,
    })
}

fn carrier_peer_reputation_policy_json(
    remote_proofs: &[CarrierReplicationProof],
    live_multi_peer_proof: bool,
) -> serde_json::Value {
    let scored_remote_peers = remote_proofs
        .iter()
        .filter(|proof| proof.reputation_reason != "no_local_history")
        .count();
    serde_json::json!({
        "schema": CARRIER_PEER_REPUTATION_SCHEMA,
        "policy": "local_runtime_reputation",
        "scope": "content-availability",
        "status": if scored_remote_peers > 0 {
            "local_history_applied"
        } else if live_multi_peer_proof {
            "live_peer_proof_without_local_history"
        } else {
            "no_remote_peer_proof"
        },
        "local_runtime": {
            "used_for_candidate_score": true,
            "history_store": "carrier-peer-reputation.json",
            "scored_remote_peers": scored_remote_peers,
            "max_positive_score_delta": 20,
            "max_negative_score_delta": -30,
        },
        "federation": {
            "configured": false,
            "cross_runtime_reputation": false,
            "signed_reputation_receipts": false,
            "third_party_attestations": false,
            "reason": "this branch uses local Runtime success/failure history only; federated peer reputation needs signed cross-provider reputation receipts and trust policy",
        },
    })
}

fn carrier_peer_attestation_exchange_policy_json(
    remote_proofs: &[CarrierReplicationProof],
    live_multi_peer_proof: bool,
    exchange: CarrierPeerAttestationExchangeView<'_>,
) -> serde_json::Value {
    let remote_provider_proofs = remote_proofs.len();
    let verified_remote_content_receipts = remote_proofs
        .iter()
        .filter(|proof| {
            proof
                .remote_receipt
                .as_ref()
                .and_then(|receipt| receipt.get("verified"))
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
        })
        .count();
    let exchange_status = exchange
        .receipt
        .and_then(|receipt| receipt.get("status"))
        .and_then(|value| value.as_str());
    let exchange_accepted = exchange
        .receipt
        .and_then(|receipt| receipt.get("accepted"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    serde_json::json!({
        "schema": CARRIER_PEER_ATTESTATION_EXCHANGE_POLICY_SCHEMA,
        "policy": if exchange.configured {
            "configured_peer_attestation_exchange"
        } else {
            "no_cross_runtime_attestation_exchange"
        },
        "scope": "content-availability",
        "status": if exchange_accepted {
            "attestation_exchange_accepted"
        } else if exchange.configured && exchange.receipt.is_some() {
            exchange_status.unwrap_or("attestation_exchange_failed")
        } else if exchange.configured && live_multi_peer_proof {
            "attestation_exchange_configured_without_receipt"
        } else if exchange.configured {
            "attestation_exchange_configured_no_remote_peer_proof"
        } else if live_multi_peer_proof {
            "live_peer_proof_without_attestation_exchange"
        } else {
            "no_remote_peer_proof"
        },
        "local_proof": {
            "signed_availability_announcements": true,
            "verified_remote_content_receipts": verified_remote_content_receipts,
            "remote_provider_proofs": remote_provider_proofs,
            "local_runtime_reputation": true,
            "peer_reputation_schema": CARRIER_PEER_REPUTATION_SCHEMA,
        },
        "attestation_exchange": {
            "configured": exchange.configured,
            "signed_reputation_receipts": exchange_accepted,
            "third_party_attestations": false,
            "cross_runtime_trust_policy": if exchange.configured {
                "configured_endpoint"
            } else {
                "not_configured"
            },
            "revocation": "not_configured",
            "receipt": exchange.receipt.cloned().unwrap_or(serde_json::Value::Null),
            "reason": if exchange_accepted {
                "configured peer-attestation exchange accepted a signed Carrier proof receipt"
            } else if exchange.configured {
                "a peer-attestation exchange endpoint is configured, but this proof has no accepted signed exchange receipt"
            } else {
                "this branch verifies signed availability announcements and remote content receipts only; signed cross-runtime reputation attestations need a federated trust policy and receipt exchange"
            },
        },
    })
}

fn carrier_quota_json(
    requirements: CarrierAvailabilityRequirements,
    replicas: u32,
    desired_replicas: u32,
) -> serde_json::Value {
    let effective_max_replicas = requirements.effective_max();
    let requirements_exceed_quota = requirements.min_replicas > effective_max_replicas;
    let quota_status = if requirements_exceed_quota {
        "requirements_exceed_quota"
    } else if replicas >= effective_max_replicas {
        "at_quota"
    } else {
        "within_quota"
    };
    serde_json::json!({
        "policy": "carrier_provider_quota",
        "scope": "content-availability",
        "enforced": true,
        "status": quota_status,
        "min_replicas": requirements.min_replicas,
        "desired_replicas": desired_replicas,
        "max_replicas": requirements.max_replicas.unwrap_or(MAX_CARRIER_REPLICATION_CANDIDATES as u32 + 1),
        "effective_max_replicas": effective_max_replicas,
        "used_replicas": replicas,
        "candidate_limit": MAX_CARRIER_REPLICATION_CANDIDATES,
        "requirements_exceed_quota": requirements_exceed_quota,
        "requirements": requirements.to_json(),
        "federated_quota_ledger_policy": carrier_federated_quota_ledger_policy_json(
            "carrier_provider_quota",
            quota_status,
            true,
            true,
        ),
    })
}

fn carrier_federated_quota_ledger_policy_json(
    mode: &str,
    quota_status: &str,
    local_principal_ledger: bool,
    remote_admission_preflight: bool,
) -> serde_json::Value {
    serde_json::json!({
        "schema": CONTENT_FEDERATED_QUOTA_LEDGER_POLICY_SCHEMA,
        "policy": "local_principal_ledger_plus_remote_admission_preflight",
        "scope": "content-availability",
        "status": "federated_quota_ledger_not_configured",
        "quota": {
            "mode": mode,
            "status": quota_status,
            "enforced": true,
        },
        "local": {
            "principal_storage_ledger": local_principal_ledger,
            "ledger_schema": "elastos.content.storage-accounting.ledger/v1",
        },
        "remote": {
            "admission_preflight": remote_admission_preflight,
            "signed_admission_receipts": remote_admission_preflight,
            "admission_schema": "elastos.content.admission/v1",
            "admission_receipt_domain": CONTENT_ADMISSION_DOMAIN,
        },
        "federation": {
            "configured": false,
            "cross_provider_quota_ledger": false,
            "storage_admission_network": false,
            "signed_admission_receipt_exchange": remote_admission_preflight,
            "quota_receipt_exchange": false,
            "production_quota_receipt_exchange": false,
            "reason": if remote_admission_preflight {
                "Carrier verifies signed remote content/admission receipts for this proof path; federated quota ledgers and production storage-admission networks remain unconfigured"
            } else {
                "Carrier local quota exists, but remote signed admission and federated quota ledgers are not configured for this path"
            },
        },
    })
}

fn default_carrier_federated_quota_ledger_policy_json() -> serde_json::Value {
    carrier_federated_quota_ledger_policy_json("not_reported", "not_reported", false, false)
}

fn carrier_repair_worker_json(scheduled: bool, availability_status: &str) -> serde_json::Value {
    serde_json::json!({
        "scheduled": scheduled,
        "status": if scheduled { "queued" } else { "healthy" },
        "worker": "carrier-availability",
        "reason": if scheduled {
            format!("availability status is {availability_status}")
        } else {
            "replica requirements satisfied".to_string()
        },
    })
}

fn carrier_repair_graph_policy_json(
    requirements: CarrierAvailabilityRequirements,
) -> serde_json::Value {
    let current_modes = ["object_manifest", "exact_bytes"];
    let requested_kind = requirements.repair_graph_kind.as_str();
    let supported = requirements
        .repair_graph_kind
        .supports_current_import_fallback();
    serde_json::json!({
        "schema": CONTENT_REPAIR_GRAPH_SCHEMA,
        "policy": "carrier_provider_bounded_graph_repair",
        "requested_kind": requested_kind,
        "status": if supported {
            "bounded_import_supported"
        } else {
            "unsupported_without_block_graph_provider"
        },
        "supported_import_fallbacks": current_modes,
        "refuses_exact_fallback_for_arbitrary_dag": true,
        "block_graph_contract": {
            "provider": CONTENT_BLOCK_GRAPH_PROVIDER,
            "target": CONTENT_BLOCK_GRAPH_TARGET,
            "schema": CONTENT_BLOCK_GRAPH_SCHEMA,
            "operations": ["export_graph", "import_graph", "status"]
        },
        "requires_provider": if supported {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(CONTENT_BLOCK_GRAPH_PROVIDER.to_string())
        },
    })
}

fn carrier_storage_market_policy_json(
    replicas: u32,
    live_multi_peer_proof: bool,
) -> serde_json::Value {
    let status = if live_multi_peer_proof {
        "receipt_proven_no_market_settlement"
    } else {
        "local_or_announced_no_market_settlement"
    };
    serde_json::json!({
        "schema": "elastos.content.storage-market/v1",
        "mode": "carrier_provider_receipts",
        "status": status,
        "settlement": "not_configured",
        "escrow": "not_configured",
        "quota_enforced": true,
        "replicas": replicas,
        "live_multi_peer_proof": live_multi_peer_proof,
        "remote_admission_preflight": live_multi_peer_proof,
        "admission_policy": carrier_storage_market_admission_policy_json(
            "carrier_provider_receipts",
            status,
            true,
            live_multi_peer_proof,
            live_multi_peer_proof,
        ),
        "settlement_policy": carrier_storage_settlement_policy_json(
            "carrier_provider_receipts",
            status,
            true,
            live_multi_peer_proof,
        ),
        "next": "Production storage markets need pricing, escrow/settlement, storage-market admission, and cross-peer SLA policy before enabling."
    })
}

fn carrier_abuse_controls_json(
    candidate_count: usize,
    attempt_limit: usize,
    attempted_operations: u32,
    failed_operations: u32,
    candidate_limit_applied: bool,
) -> serde_json::Value {
    serde_json::json!({
        "schema": "elastos.content.abuse-controls/v1",
        "policy": "carrier_provider_invocation_guardrail",
        "scope": "content-availability",
        "enforced": true,
        "candidate_limit": MAX_CARRIER_REPLICATION_CANDIDATES,
        "candidate_count": candidate_count,
        "attempt_limit": attempt_limit,
        "attempted_operations": attempted_operations,
        "failed_operations": failed_operations,
        "throttled": candidate_limit_applied,
        "reason": if candidate_limit_applied {
            "candidate attempt limit applied"
        } else {
            "candidate attempts within bounded provider-invocation budget"
        },
    })
}

fn carrier_remote_candidate_limit(
    requirements: CarrierAvailabilityRequirements,
    local_replicas: u32,
    desired_replicas: u32,
) -> usize {
    let replica_shortfall = desired_replicas.saturating_sub(local_replicas);
    let remaining_quota = requirements.effective_max().saturating_sub(local_replicas);
    let live_remote_required =
        u32::from(requirements.require_live_multi_peer_proof && remaining_quota > 0);
    replica_shortfall
        .max(live_remote_required)
        .min(remaining_quota)
        .min(MAX_CARRIER_REPLICATION_CANDIDATES as u32) as usize
}

fn carrier_repair_reason(
    requirements: CarrierAvailabilityRequirements,
    replicas: u32,
    live_multi_peer_proof: bool,
    errors: &[String],
) -> String {
    let mut reasons = Vec::new();
    if replicas < requirements.min_replicas {
        reasons.push(format!(
            "only {replicas} replica(s) proven; {} required",
            requirements.min_replicas
        ));
    }
    if requirements.require_live_multi_peer_proof && !live_multi_peer_proof {
        reasons.push(
            "live multi-peer proof is required but no independent remote replica was proven"
                .to_string(),
        );
    }
    if !errors.is_empty() {
        reasons.push(format!("replication errors: {}", errors.join(" | ")));
    }
    if reasons.is_empty() {
        "Carrier availability repair is required".to_string()
    } else {
        reasons.join("; ")
    }
}

fn content_availability_replicas(
    messages: &[GossipMessage],
    cid: &str,
) -> Vec<CarrierAvailabilityReplica> {
    content_availability_replicas_with_reputation(messages, cid, &HashMap::new())
}

fn content_availability_replicas_with_reputation(
    messages: &[GossipMessage],
    cid: &str,
    reputation: &HashMap<String, CarrierPeerReputation>,
) -> Vec<CarrierAvailabilityReplica> {
    let mut replicas = Vec::new();
    for message in messages {
        let Ok((envelope, signer_did)) = crate::crypto::verify_signed_json_envelope_against_dids(
            message.content.as_bytes(),
            CONTENT_AVAILABILITY_ANNOUNCEMENT_DOMAIN,
            &[],
        ) else {
            continue;
        };
        let Some(payload) = envelope.get("payload") else {
            continue;
        };
        if payload.get("schema").and_then(|value| value.as_str())
            != Some(CONTENT_AVAILABILITY_ANNOUNCEMENT_SCHEMA)
        {
            continue;
        }
        if payload.get("cid").and_then(|value| value.as_str()) != Some(cid) {
            continue;
        }
        if payload.get("node_did").and_then(|value| value.as_str()) != Some(signer_did.as_str()) {
            continue;
        }
        let Some(ticket) = payload
            .get("fetch")
            .and_then(|value| value.get("connect_ticket"))
            .and_then(|value| value.as_str())
            .filter(|value| {
                let value = value.trim();
                !value.is_empty() && value.len() <= MAX_CARRIER_AVAILABILITY_TICKET_LEN
            })
        else {
            continue;
        };
        let raw_endpoint_id = payload
            .get("fetch")
            .and_then(|value| value.get("endpoint_id"))
            .and_then(|value| value.as_str());
        if raw_endpoint_id
            .map(|value| value.len() > MAX_CARRIER_AVAILABILITY_ENDPOINT_ID_LEN)
            .unwrap_or(false)
        {
            continue;
        }
        let endpoint_id = raw_endpoint_id
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string);
        let node_did = signer_did.to_string();
        if replicas
            .iter()
            .any(|replica: &CarrierAvailabilityReplica| replica.node_did == node_did)
        {
            continue;
        }
        let announced_at = payload
            .get("announced_at")
            .and_then(|value| value.as_u64())
            .unwrap_or(message.ts);
        let (score, selection_reason, reputation_score, reputation_reason) =
            carrier_replica_candidate_score(
                endpoint_id.as_deref(),
                announced_at,
                message.ts,
                reputation.get(&node_did),
            );
        replicas.push(CarrierAvailabilityReplica {
            node_did,
            endpoint_id,
            connect_ticket: ticket.to_string(),
            announced_at,
            score,
            selection_reason,
            reputation_score,
            reputation_reason,
        });
    }
    replicas.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.announced_at.cmp(&a.announced_at))
            .then_with(|| a.node_did.cmp(&b.node_did))
    });
    replicas
}

fn carrier_replica_candidate_score(
    endpoint_id: Option<&str>,
    announced_at: u64,
    message_ts: u64,
    reputation: Option<&CarrierPeerReputation>,
) -> (u32, String, i32, String) {
    let mut score = 50_u32;
    let mut reasons = vec!["signed_announcement"];
    if endpoint_id
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
    {
        score = score.saturating_add(20);
        reasons.push("endpoint_advertised");
    }
    if announced_at >= message_ts.saturating_sub(60 * 60) {
        score = score.saturating_add(20);
        reasons.push("fresh");
    } else {
        reasons.push("stale");
    }
    let (reputation_score, reputation_reason) = carrier_reputation_score(reputation);
    if reputation_score > 0 {
        score = score.saturating_add(reputation_score as u32);
        reasons.push("local_reputation_positive");
    } else if reputation_score < 0 {
        score = score.saturating_sub(reputation_score.unsigned_abs());
        reasons.push("local_reputation_negative");
    } else {
        reasons.push("local_reputation_neutral");
    }
    (
        score.min(100),
        reasons.join("+"),
        reputation_score,
        reputation_reason,
    )
}

fn carrier_reputation_score(reputation: Option<&CarrierPeerReputation>) -> (i32, String) {
    let Some(reputation) = reputation else {
        return (0, "no_local_history".to_string());
    };
    let successes = reputation.successes.min(5) as i32;
    let failures = reputation.failures.min(5) as i32;
    let score = (successes * 4 - failures * 8).clamp(-30, 20);
    (
        score,
        format!(
            "local_runtime_successes:{};failures:{}",
            reputation.successes, reputation.failures
        ),
    )
}

fn carrier_peer_reputation_path(data_dir: &std::path::Path) -> PathBuf {
    data_dir
        .join("ElastOS")
        .join("SystemServices")
        .join("Content")
        .join("carrier-peer-reputation.json")
}

fn load_carrier_peer_reputation(
    data_dir: &std::path::Path,
) -> HashMap<String, CarrierPeerReputation> {
    let path = carrier_peer_reputation_path(data_dir);
    let Ok(bytes) = std::fs::read(&path) else {
        return HashMap::new();
    };
    let Ok(store) = serde_json::from_slice::<CarrierPeerReputationStore>(&bytes) else {
        tracing::debug!("carrier peer reputation decode failed: {}", path.display());
        return HashMap::new();
    };
    if store.schema != CARRIER_PEER_REPUTATION_SCHEMA {
        tracing::debug!(
            "carrier peer reputation schema mismatch at {}: {}",
            path.display(),
            store.schema
        );
        return HashMap::new();
    }
    store.peers.into_iter().collect()
}

fn save_carrier_peer_reputation(
    data_dir: &std::path::Path,
    reputation: &HashMap<String, CarrierPeerReputation>,
) -> Result<()> {
    let path = carrier_peer_reputation_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let store = CarrierPeerReputationStore {
        schema: CARRIER_PEER_REPUTATION_SCHEMA.to_string(),
        peers: reputation
            .iter()
            .map(|(node_did, reputation)| (node_did.clone(), reputation.clone()))
            .collect(),
    };
    let bytes = serde_json::to_vec_pretty(&store)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

fn content_availability_fetch_tickets(messages: &[GossipMessage], cid: &str) -> Vec<String> {
    content_availability_replicas(messages, cid)
        .into_iter()
        .map(|replica| replica.connect_ticket)
        .collect()
}

fn gossip_cursor_error(code: &str, message: &str) -> serde_json::Value {
    serde_json::json!({"status":"error","code":code,"message":message})
}

fn parse_gossip_cursor_identity(
    request: &serde_json::Value,
) -> std::result::Result<(String, String), serde_json::Value> {
    let topic = request
        .get("topic")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| gossip_cursor_error("invalid_topic", "topic must be a string"))?;
    if topic != topic.trim()
        || topic.chars().any(char::is_control)
        || validate_carrier_gossip_topic(topic).is_err()
    {
        return Err(gossip_cursor_error(
            "invalid_topic",
            "topic must be non-empty, canonical, and within the topic bound",
        ));
    }

    let consumer_id = request
        .get("consumer_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            gossip_cursor_error("invalid_consumer_id", "consumer_id must be a string")
        })?;
    if consumer_id.is_empty()
        || consumer_id != consumer_id.trim()
        || consumer_id.len() > GOSSIP_CURSOR_MAX_CONSUMER_ID_BYTES
        || consumer_id.chars().any(char::is_control)
    {
        return Err(gossip_cursor_error(
            "invalid_consumer_id",
            "consumer_id must be non-empty, canonical, and within the consumer bound",
        ));
    }

    Ok((topic.to_string(), consumer_id.to_string()))
}

fn parse_gossip_peek_limit(
    request: &serde_json::Value,
) -> std::result::Result<usize, serde_json::Value> {
    let limit = request
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| gossip_cursor_error("invalid_limit", "limit must be an integer"))?;
    if limit == 0 || limit > GOSSIP_PEEK_MAX_MESSAGES as u64 {
        return Err(gossip_cursor_error(
            "invalid_limit",
            "limit must be within the bounded gossip peek range",
        ));
    }
    Ok(limit as usize)
}

fn parse_gossip_ack_range(
    request: &serde_json::Value,
) -> std::result::Result<GossipCursorRange, serde_json::Value> {
    let cursor = request
        .get("cursor")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| gossip_cursor_error("invalid_cursor", "cursor must be an integer"))?;
    let next_cursor = request
        .get("next_cursor")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            gossip_cursor_error("invalid_next_cursor", "next_cursor must be an integer")
        })?;
    Ok(GossipCursorRange {
        cursor,
        next_cursor,
    })
}

fn evict_gossip_cursors_for_new_consumer(
    cursors: &mut HashMap<(String, String), GossipConsumerCursor>,
    cursor_key: &(String, String),
) {
    if !cursors.contains_key(cursor_key) && cursors.len() >= MAX_CURSORS {
        let to_remove = cursors
            .keys()
            .take(MAX_CURSORS / 10)
            .cloned()
            .collect::<Vec<_>>();
        for key in to_remove {
            cursors.remove(&key);
        }
    }
}

struct BoundedJsonByteCounter {
    bytes: usize,
    max_bytes: usize,
}

impl std::io::Write for BoundedJsonByteCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let Some(next) = self.bytes.checked_add(bytes.len()) else {
            return Err(std::io::Error::other("JSON byte count overflow"));
        };
        if next > self.max_bytes {
            return Err(std::io::Error::other("JSON byte limit exceeded"));
        }
        self.bytes = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn gossip_peek_response(
    cursor: u64,
    next_cursor: u64,
    messages: &[&GossipMessage],
    limit: usize,
) -> serde_json::Value {
    serde_json::json!({
        "status":"ok",
        "data":{
            "cursor": cursor,
            "next_cursor": next_cursor,
            "messages": messages,
            "scanned": messages.len(),
            "limit": limit
        }
    })
}

fn gossip_peek_response_envelope_max_bytes() -> usize {
    let response = gossip_peek_response(u64::MAX, u64::MAX, &[], usize::MAX);
    let mut counter = BoundedJsonByteCounter {
        bytes: 0,
        max_bytes: usize::MAX,
    };
    if serde_json::to_writer(&mut counter, &response).is_err() {
        return GOSSIP_PEEK_MAX_BATCH_BYTES;
    }
    counter.bytes.saturating_sub(2) // Exclude the empty messages array itself.
}

fn bounded_gossip_peek_prefix<'a>(
    messages: impl Iterator<Item = &'a GossipMessage>,
    limit: usize,
) -> std::result::Result<Vec<&'a GossipMessage>, ()> {
    let mut selected = Vec::with_capacity(limit);
    let Some(encoded_messages_limit) =
        GOSSIP_PEEK_MAX_BATCH_BYTES.checked_sub(gossip_peek_response_envelope_max_bytes())
    else {
        return Err(());
    };
    let mut encoded_batch_bytes = 2_usize; // JSON array brackets.
    for message in messages.take(limit) {
        let separator_bytes = usize::from(!selected.is_empty());
        let Some(message_limit) = encoded_messages_limit
            .checked_sub(encoded_batch_bytes)
            .and_then(|remaining| remaining.checked_sub(separator_bytes))
        else {
            if selected.is_empty() {
                return Err(());
            }
            break;
        };
        let mut counter = BoundedJsonByteCounter {
            bytes: 0,
            max_bytes: message_limit,
        };
        if serde_json::to_writer(&mut counter, message).is_err() {
            if selected.is_empty() {
                return Err(());
            }
            break;
        }
        encoded_batch_bytes += separator_bytes + counter.bytes;
        selected.push(message);
    }
    Ok(selected)
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

            "gossip_join_exact" => {
                let (topic_name, exact_peers) = match parse_gossip_join_exact_request(request) {
                    Ok(parsed) => parsed,
                    Err(error) => return Ok(error),
                };
                if state.joined_topics.contains(&topic_name) {
                    return Ok(
                        serde_json::json!({"status":"error","code":"already_joined","message":"already joined"}),
                    );
                }
                if state.joined_topics.len() >= MAX_TOPICS {
                    return Ok(
                        serde_json::json!({"status":"error","code":"too_many_topics","message":"topic limit reached"}),
                    );
                }

                match join_gossip_topic_direct(&mut state, &topic_name, exact_peers).await {
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
                    let _ = task.await;
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
                let bytes = match serialize_gossip_wire_message(&msg) {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        return Ok(serde_json::json!({
                            "status":"error",
                            "code":"message_too_large",
                            "message":"complete gossip message exceeds wire limit"
                        }))
                    }
                };

                // Insert into local buffer so other local clients (native chat,
                // WASM bridge, microVM bridge) on the same runtime see the message.
                {
                    let mut bufs = state.buffers.lock().await;
                    if let Some(buf) = bufs.get_mut(topic_name) {
                        push_gossip_buffer_message(buf, msg.clone());
                    }
                }

                let remote_peer_count = state
                    .topic_peers
                    .lock()
                    .await
                    .get(topic_name)
                    .map(|peers| peers.len())
                    .unwrap_or_default();
                match tokio::time::timeout(GOSSIP_SEND_TIMEOUT, sender.broadcast(bytes)).await {
                    Ok(Ok(_)) if remote_peer_count > 0 => Ok(serde_json::json!({
                        "status":"ok",
                        "data": {"remote_peer_count": remote_peer_count}
                    })),
                    Ok(Ok(_)) => Ok(serde_json::json!({
                        "status":"ok",
                        "broadcast":"local_only",
                        "data": {"remote_peer_count": remote_peer_count}
                    })),
                    Ok(Err(e)) => {
                        // Broadcast may fail with 0 peers — message is still in
                        // the local buffer for same-runtime clients, but remote
                        // peers did NOT receive it. Report honestly.
                        tracing::debug!("gossip broadcast to external peers failed: {}", e);
                        Ok(serde_json::json!({
                            "status":"ok",
                            "broadcast":"local_only",
                            "data": {"remote_peer_count": remote_peer_count}
                        }))
                    }
                    Err(_) => {
                        tracing::debug!("gossip broadcast to external peers timed out");
                        Ok(serde_json::json!({
                            "status":"ok",
                            "broadcast":"local_only",
                            "data": {"remote_peer_count": remote_peer_count}
                        }))
                    }
                }
            }

            "gossip_peek" => {
                let (topic_name, consumer_id) = match parse_gossip_cursor_identity(request) {
                    Ok(identity) => identity,
                    Err(error) => return Ok(error),
                };
                let limit = match parse_gossip_peek_limit(request) {
                    Ok(limit) => limit,
                    Err(error) => return Ok(error),
                };
                if !state.joined_topics.contains(&topic_name) {
                    return Ok(gossip_cursor_error(
                        "not_joined",
                        "gossip topic is not joined",
                    ));
                }

                let buffers = state.buffers.lock().await;
                let Some(buffer) = buffers.get(&topic_name) else {
                    return Ok(gossip_cursor_error(
                        "missing_topic_buffer",
                        "joined gossip topic has no buffer",
                    ));
                };
                let Some(buffer_tail) = buffer.base_index.checked_add(buffer.messages.len() as u64)
                else {
                    return Ok(gossip_cursor_error(
                        "invalid_topic_buffer",
                        "gossip topic buffer cursor overflow",
                    ));
                };

                let mut cursors = state.cursors.lock().await;
                let cursor_key = (topic_name, consumer_id);
                evict_gossip_cursors_for_new_consumer(&mut cursors, &cursor_key);
                let cursor = cursors
                    .entry(cursor_key)
                    .or_insert_with(|| GossipConsumerCursor::at(buffer.base_index));
                cursor.normalize_to(buffer.base_index);
                if cursor.position > buffer_tail {
                    return Ok(gossip_cursor_error(
                        "invalid_cursor_state",
                        "consumer cursor is beyond the gossip buffer tail",
                    ));
                }

                let start = (cursor.position - buffer.base_index) as usize;
                let outstanding = cursor.outstanding_peek;
                let batch_limit = match outstanding {
                    Some(peek) if peek.limit != limit => {
                        return Ok(gossip_cursor_error(
                            "peek_limit_conflict",
                            "retry limit must match the outstanding gossip peek limit",
                        ))
                    }
                    Some(peek)
                        if peek.range.cursor == cursor.position
                            && peek.range.next_cursor >= peek.range.cursor
                            && peek.range.next_cursor <= buffer_tail =>
                    {
                        (peek.range.next_cursor - peek.range.cursor) as usize
                    }
                    Some(_) => {
                        return Ok(gossip_cursor_error(
                            "invalid_cursor_state",
                            "outstanding gossip peek range is inconsistent with the buffer",
                        ))
                    }
                    None => limit,
                };
                let messages = match bounded_gossip_peek_prefix(
                    buffer.messages.iter().skip(start),
                    batch_limit,
                ) {
                    Ok(messages) => messages,
                    Err(()) => {
                        return Ok(gossip_cursor_error(
                            "peek_frame_too_large",
                            "first retained gossip frame exceeds the encoded batch byte limit",
                        ))
                    }
                };
                let scanned = messages.len();
                if outstanding.is_some() && scanned != batch_limit {
                    return Ok(gossip_cursor_error(
                        "invalid_cursor_state",
                        "outstanding gossip peek range is no longer fully retained",
                    ));
                }
                let next_cursor = cursor.position + scanned as u64;
                cursor.outstanding_peek = Some(OutstandingGossipPeek {
                    range: GossipCursorRange {
                        cursor: cursor.position,
                        next_cursor,
                    },
                    limit,
                });

                Ok(gossip_peek_response(
                    cursor.position,
                    next_cursor,
                    &messages,
                    limit,
                ))
            }

            "gossip_ack" => {
                let (topic_name, consumer_id) = match parse_gossip_cursor_identity(request) {
                    Ok(identity) => identity,
                    Err(error) => return Ok(error),
                };
                let acknowledged = match parse_gossip_ack_range(request) {
                    Ok(range) => range,
                    Err(error) => return Ok(error),
                };
                if !state.joined_topics.contains(&topic_name) {
                    return Ok(gossip_cursor_error(
                        "not_joined",
                        "gossip topic is not joined",
                    ));
                }

                let buffers = state.buffers.lock().await;
                let Some(buffer) = buffers.get(&topic_name) else {
                    return Ok(gossip_cursor_error(
                        "missing_topic_buffer",
                        "joined gossip topic has no buffer",
                    ));
                };
                let Some(buffer_tail) = buffer.base_index.checked_add(buffer.messages.len() as u64)
                else {
                    return Ok(gossip_cursor_error(
                        "invalid_topic_buffer",
                        "gossip topic buffer cursor overflow",
                    ));
                };
                if acknowledged.next_cursor < acknowledged.cursor {
                    return Ok(gossip_cursor_error(
                        "backward_ack",
                        "next_cursor must not precede cursor",
                    ));
                }
                if acknowledged.next_cursor > buffer_tail {
                    return Ok(gossip_cursor_error(
                        "ack_beyond_tail",
                        "next_cursor exceeds the current gossip buffer tail",
                    ));
                }

                let mut cursors = state.cursors.lock().await;
                let cursor_key = (topic_name, consumer_id);
                let Some(cursor) = cursors.get_mut(&cursor_key) else {
                    return Ok(gossip_cursor_error(
                        "unknown_cursor",
                        "gossip acknowledgement requires an existing consumer cursor",
                    ));
                };
                let acknowledges_current_peek =
                    cursor.outstanding_peek.map(|peek| peek.range) == Some(acknowledged);
                if !acknowledges_current_peek && cursor.last_acknowledged == Some(acknowledged) {
                    return Ok(serde_json::json!({
                        "status":"ok",
                        "data":{
                            "cursor": acknowledged.cursor,
                            "next_cursor": acknowledged.next_cursor,
                            "advanced": false
                        }
                    }));
                }
                cursor.normalize_to(buffer.base_index);
                if cursor.position > buffer_tail {
                    return Ok(gossip_cursor_error(
                        "invalid_cursor_state",
                        "consumer cursor is beyond the gossip buffer tail",
                    ));
                }
                if acknowledged.cursor != cursor.position {
                    return Ok(gossip_cursor_error(
                        "cursor_conflict",
                        "acknowledgement cursor does not match the current consumer cursor",
                    ));
                }
                if cursor.outstanding_peek.map(|peek| peek.range) != Some(acknowledged) {
                    return Ok(gossip_cursor_error(
                        "unobserved_ack_range",
                        "acknowledgement must match the exact outstanding gossip peek range",
                    ));
                }

                let advanced = acknowledged.next_cursor > acknowledged.cursor;
                cursor.position = acknowledged.next_cursor;
                cursor.outstanding_peek = None;
                cursor.last_acknowledged = Some(acknowledged);

                Ok(serde_json::json!({
                    "status":"ok",
                    "data":{
                        "cursor": acknowledged.cursor,
                        "next_cursor": acknowledged.next_cursor,
                        "advanced": advanced
                    }
                }))
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
                    None => {
                        return Ok(serde_json::json!({
                            "status":"ok",
                            "data":{"messages":[],"scanned":0,"limit":limit,"next_cursor":null}
                        }))
                    }
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
                let cursor = cursors
                    .entry(cursor_key)
                    .or_insert_with(|| GossipConsumerCursor::at(buf.base_index));

                let start = if cursor.position >= buf.base_index {
                    (cursor.position - buf.base_index) as usize
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

                let next_cursor = buf.base_index + start as u64 + count as u64;
                cursor.position = next_cursor;
                cursor.outstanding_peek = None;
                cursor.last_acknowledged = None;

                Ok(serde_json::json!({
                    "status":"ok",
                    "data":{
                        "messages": messages,
                        "scanned": count,
                        "limit": limit,
                        "next_cursor": next_cursor
                    }
                }))
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
                match tokio::time::timeout(
                    GOSSIP_JOIN_PEERS_TIMEOUT,
                    sender.join_peers(peer_ids, None),
                )
                .await
                {
                    Ok(Ok(_)) => {
                        Ok(serde_json::json!({"status":"ok","data":{"topic": topic_name}}))
                    }
                    Ok(Err(err)) => Ok(
                        serde_json::json!({"status":"error","code":"join_failed","message": err.to_string()}),
                    ),
                    Err(_) => Ok(
                        serde_json::json!({"status":"error","code":"join_timeout","message":"peer join timed out"}),
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
            if msg.content.len() > GOSSIP_WIRE_MESSAGE_MAX_BYTES {
                return;
            }
            let Ok(gossip_msg) = serde_json::from_slice::<GossipMessage>(&msg.content) else {
                return;
            };
            if serialize_gossip_wire_message(&gossip_msg).is_err() {
                return;
            }
            let mut bufs = buffers.lock().await;
            if let Some(buf) = bufs.get_mut(topic) {
                push_gossip_buffer_message(buf, gossip_msg);
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

enum CarrierGossipReceiver {
    Direct(iroh_gossip::api::GossipReceiver),
    Discovered(distributed_topic_tracker::GossipReceiver),
}

impl CarrierGossipReceiver {
    async fn next(&mut self) -> anyhow::Result<Option<iroh_gossip::api::Event>> {
        match self {
            Self::Direct(receiver) => match receiver.next().await {
                Some(Ok(event)) => Ok(Some(event)),
                Some(Err(err)) => Err(err.into()),
                None => Ok(None),
            },
            Self::Discovered(receiver) => match receiver.next().await {
                Ok(event) => Ok(Some(event)),
                Err(err) => {
                    if receiver.is_joined().await.unwrap_or(false) {
                        Err(err.into())
                    } else {
                        tracing::warn!("carrier discovered gossip receiver ended: {err}");
                        Ok(None)
                    }
                }
            },
        }
    }
}

async fn recv_loop(
    mut receiver: CarrierGossipReceiver,
    buffers: Arc<Mutex<HashMap<String, TopicBuffer>>>,
    peers: Arc<Mutex<Vec<String>>>,
    topic_peers: Arc<Mutex<HashMap<String, HashSet<String>>>>,
    topic: String,
) {
    loop {
        match receiver.next().await {
            Ok(Some(event)) => {
                handle_gossip_event(event, &buffers, &peers, &topic_peers, &topic).await;
            }
            Err(e) => {
                tracing::warn!("carrier recv_loop error on '{}': {}", topic, e);
                // Continue — transient errors should not kill the receiver
            }
            Ok(None) => {
                tracing::info!("carrier recv_loop ended for '{}' (stream closed)", topic);
                break;
            }
        }
    }
}

// ── Client and provider-plane invocation ─────────────────────────

/// Runtime-owned Carrier peer invoker.
///
/// Peer-DID and connect-ticket routes use the long-lived Carrier endpoint, so
/// every invocation keeps one Runtime endpoint identity and its discovery
/// context. An invoker built without that endpoint has no verified remote
/// route and fails before a provider effect.
#[derive(Default)]
pub struct CarrierProviderInvoker {
    peer_endpoint: Option<Endpoint>,
}

#[derive(Debug, Clone)]
pub struct BrowserCarrierStreamRequest {
    pub connect_ticket: String,
    pub peer_did: Option<String>,
    pub carrier_service: String,
    pub grant_id: String,
    pub stream_id: String,
    pub target: String,
    pub principal_id: Option<String>,
    pub reason: Option<String>,
    pub timeout_ms: Option<u64>,
}

pub struct BrowserCarrierStream {
    pub send: iroh::endpoint::SendStream,
    pub recv: iroh::endpoint::RecvStream,
    _client: CarrierClient,
}

impl CarrierProviderInvoker {
    pub fn new() -> Self {
        Self {
            peer_endpoint: None,
        }
    }

    /// Binds the invoker to the long-lived Carrier endpoint that owns peer
    /// discovery. Only this form can resolve a peer-DID route.
    pub fn with_carrier_endpoint(endpoint: Endpoint) -> Self {
        Self {
            peer_endpoint: Some(endpoint),
        }
    }
}

#[async_trait::async_trait]
impl ProviderCarrierInvoker for CarrierProviderInvoker {
    async fn invoke_carrier_provider(
        &self,
        route: &ProviderCarrierRoute,
        invocation: &ProviderInvocation,
        request: serde_json::Value,
    ) -> std::result::Result<serde_json::Value, ProviderError> {
        let timeout_secs = carrier_route_timeout_secs(route);
        match route {
            ProviderCarrierRoute::ConnectTicket {
                connect_ticket,
                peer_did,
                ..
            } => {
                let peer_endpoint = self.peer_endpoint.as_ref().ok_or_else(|| {
                    ProviderError::Unavailable(
                        "Carrier peer routing is not available on this Runtime".to_string(),
                    )
                })?;
                let mut endpoints = decode_ticket_endpoints(connect_ticket);
                if let Some(peer_did) = peer_did.as_deref() {
                    endpoints.retain(|endpoint| carrier_endpoint_matches_peer(endpoint, peer_did));
                    if endpoints.is_empty() {
                        return Err(ProviderError::Provider(
                            "Carrier provider invocation peer_did does not match connect_ticket"
                                .to_string(),
                        ));
                    }
                }
                if endpoints.is_empty() {
                    return Err(ProviderError::Provider(
                        "Carrier provider invocation connect_ticket has no endpoints".to_string(),
                    ));
                }

                let mut errors = Vec::new();
                for (index, endpoint) in endpoints.into_iter().enumerate() {
                    match CarrierClient::connect_known_endpoint(
                        peer_endpoint,
                        endpoint,
                        timeout_secs,
                    )
                    .await
                    {
                        Ok(client) => {
                            match client.invoke_provider(invocation, request.clone()).await {
                                Ok(response) => return Ok(response),
                                Err(err) => {
                                    errors.push(format!("ticket[{index}] invoke failed: {err}"))
                                }
                            }
                        }
                        Err(err) => errors.push(format!("ticket[{index}] connect failed: {err}")),
                    }
                }

                Err(ProviderError::Provider(format!(
                    "Carrier provider invocation failed: {}",
                    errors.join(" | ")
                )))
            }
            ProviderCarrierRoute::PeerDid { peer_did, .. } => {
                let public_key = did_to_public_key(peer_did).ok_or_else(|| {
                    ProviderError::Provider(
                        "Carrier provider invocation peer_did is invalid".to_string(),
                    )
                })?;
                // Runtime owns peer resolution. Without the long-lived Carrier
                // endpoint there is no verified route, so report the peer as
                // unavailable before any provider effect.
                let peer_endpoint = self.peer_endpoint.as_ref().ok_or_else(|| {
                    ProviderError::Unavailable(
                        "Carrier peer routing is not available on this Runtime".to_string(),
                    )
                })?;
                let client =
                    CarrierClient::connect_resolved_peer(peer_endpoint, public_key, timeout_secs)
                        .await
                        .map_err(|err| {
                            // Keep the cause. A message that will not send is
                            // hard enough to diagnose without the Runtime
                            // throwing away the reason on its way out.
                            tracing::debug!(
                                peer = %peer_did,
                                error = %err,
                                "Carrier peer connect failed"
                            );
                            ProviderError::Unavailable(
                                "no verified Carrier route to the requested peer".to_string(),
                            )
                        })?;
                client
                    .invoke_provider(invocation, request)
                    .await
                    .map_err(|err| {
                        tracing::debug!(
                            peer = %peer_did,
                            error = %err,
                            "Carrier peer invocation failed"
                        );
                        ProviderError::Provider(
                            "Carrier provider invocation peer_did route failed".to_string(),
                        )
                    })
            }
        }
    }
}

pub async fn open_browser_carrier_stream(
    request: &BrowserCarrierStreamRequest,
) -> Result<BrowserCarrierStream> {
    let timeout_ms = request.timeout_ms.unwrap_or(5_000).clamp(1, 60_000);
    let timeout_secs = timeout_ms.div_ceil(1_000);
    let mut endpoints = decode_ticket_endpoints(&request.connect_ticket);
    if let Some(peer_did) = request.peer_did.as_deref() {
        endpoints.retain(|endpoint| carrier_endpoint_matches_peer(endpoint, peer_did));
        if endpoints.is_empty() {
            anyhow::bail!("Browser Carrier stream peer_did does not match connect_ticket");
        }
    }
    if endpoints.is_empty() {
        anyhow::bail!("Browser Carrier stream connect_ticket has no endpoints");
    }

    let mut errors = Vec::new();
    for (index, endpoint) in endpoints.into_iter().enumerate() {
        match CarrierClient::connect_endpoint_addr(endpoint, timeout_secs).await {
            Ok(client) => match client.open_browser_exit_stream(request).await {
                Ok((send, recv)) => {
                    return Ok(BrowserCarrierStream {
                        send,
                        recv,
                        _client: client,
                    })
                }
                Err(err) => errors.push(format!("ticket[{index}] stream open failed: {err}")),
            },
            Err(err) => errors.push(format!("ticket[{index}] connect failed: {err}")),
        }
    }

    anyhow::bail!("Browser Carrier stream open failed: {}", errors.join(" | "));
}

fn carrier_route_timeout_secs(route: &ProviderCarrierRoute) -> u64 {
    let timeout_ms = route.timeout_ms().unwrap_or(5_000).clamp(1, 60_000);
    timeout_ms.div_ceil(1_000)
}

fn carrier_endpoint_matches_peer(endpoint: &iroh::EndpointAddr, peer_did: &str) -> bool {
    if let Some(public_key) = did_to_public_key(peer_did) {
        return endpoint.id == public_key;
    }
    endpoint.id.to_string() == peer_did
}

pub struct CarrierClient {
    conn: iroh::endpoint::Connection,
    _endpoint: Endpoint,
}

impl CarrierClient {
    /// Connects to a peer through an existing long-lived endpoint.
    ///
    /// The caller's endpoint owns discovery, its address book, and its relay
    /// context, so resolution reuses warm state instead of starting from a
    /// discovery-isolated endpoint. The accepted connection must present the
    /// requested endpoint identity; a substituted peer is rejected before any
    /// provider request is written.
    pub(crate) async fn connect_resolved_peer(
        endpoint: &Endpoint,
        peer: iroh::PublicKey,
        timeout_secs: u64,
    ) -> Result<Self> {
        Self::connect_known_endpoint(endpoint, iroh::EndpointAddr::from(peer), timeout_secs).await
    }

    pub(crate) async fn connect_known_endpoint(
        endpoint: &Endpoint,
        addr: iroh::EndpointAddr,
        timeout_secs: u64,
    ) -> Result<Self> {
        let peer = addr.id;
        let conn = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            endpoint.connect(addr, CARRIER_ALPN),
        )
        .await
        .map_err(|_| anyhow::anyhow!("connect timed out"))?
        .context("connect failed")?;
        if conn.remote_id() != peer {
            anyhow::bail!("resolved Carrier endpoint identity does not match the requested peer");
        }
        Ok(Self {
            conn,
            _endpoint: endpoint.clone(),
        })
    }

    pub(crate) async fn connect_endpoint_addr(
        addr: iroh::EndpointAddr,
        timeout_secs: u64,
    ) -> Result<Self> {
        let mut rng_bytes = [0u8; 32];
        getrandom::getrandom(&mut rng_bytes).map_err(|e| anyhow::anyhow!("rng: {}", e))?;
        let secret_key = SecretKey::from_bytes(&rng_bytes);
        let endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
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
        read_carrier_len_prefixed_bytes(&mut recv, &format!("trusted source file fetch for {path}"))
            .await
    }

    pub async fn fetch_content(&self, cid: &str, path: Option<&str>) -> Result<Vec<u8>> {
        let (mut send, mut recv) = self.conn.open_bi().await?;
        let mut msg = serde_json::json!({
            "op": "content_fetch",
            "cid": cid,
        });
        if let Some(path) = path.filter(|path| !path.is_empty()) {
            msg["path"] = serde_json::Value::String(path.to_string());
        }
        let mut bytes = serde_json::to_vec(&msg)?;
        bytes.push(b'\n');
        send.write_all(&bytes).await?;
        send.finish()?;
        read_carrier_len_prefixed_bytes(&mut recv, "content fetch").await
    }

    pub async fn invoke_provider(
        &self,
        invocation: &ProviderInvocation,
        request: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let (mut send, recv) = self.conn.open_bi().await?;
        let msg = serde_json::json!({
            "op": "provider_invoke",
            "source": invocation.source.as_str(),
            "target": invocation.target.as_str(),
            "operation": invocation.op.as_str(),
            "transfer": invocation.transfer.as_str(),
            "range": invocation.range.map(|range| serde_json::json!({
                "start": range.start,
                "end": range.end,
            })),
            "progress": invocation.progress.as_ref().map(|progress| serde_json::json!({
                "request_id": progress.request_id.as_str(),
                "expected_bytes": progress.expected_bytes,
            })),
            "request": request,
        });
        let mut bytes = serde_json::to_vec(&msg)?;
        bytes.push(b'\n');
        send.write_all(&bytes).await?;
        send.finish()?;

        let mut reader = BufReader::new(recv);
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let response: serde_json::Value = serde_json::from_str(line.trim())?;
        if response.get("ok").and_then(|value| value.as_bool()) == Some(true) {
            return Ok(response
                .get("result")
                .cloned()
                .unwrap_or(serde_json::Value::Null));
        }
        let message = response
            .get("error")
            .and_then(|value| value.as_str())
            .unwrap_or("Carrier provider invocation failed");
        anyhow::bail!("{message}");
    }

    pub async fn open_browser_exit_stream(
        &self,
        request: &BrowserCarrierStreamRequest,
    ) -> Result<(iroh::endpoint::SendStream, iroh::endpoint::RecvStream)> {
        let (mut send, mut recv) = self.conn.open_bi().await?;
        let msg = serde_json::json!({
            "op": "browser_exit_stream",
            "schema": BROWSER_CARRIER_STREAM_SCHEMA,
            "carrier_service": request.carrier_service,
            "grant_id": request.grant_id,
            "stream_id": request.stream_id,
            "target": request.target,
            "principal_id": request.principal_id,
            "reason": request.reason,
        });
        write_json_line(&mut send, &msg).await?;
        read_browser_carrier_stream_ack(&mut recv).await?;
        Ok((send, recv))
    }

    pub async fn push_gossip_message(&self, topic: &str, message: &GossipMessage) -> Result<()> {
        let (mut send, recv) = self.conn.open_bi().await?;
        let msg = serde_json::json!({
            "op": "gossip_push",
            "topic": topic,
            "message": message,
        });
        let mut bytes = serde_json::to_vec(&msg)?;
        bytes.push(b'\n');
        send.write_all(&bytes).await?;
        send.finish()?;

        let mut reader = BufReader::new(recv);
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let response: serde_json::Value = serde_json::from_str(line.trim())?;
        if response.get("ok").and_then(|value| value.as_bool()) == Some(true) {
            return Ok(());
        }
        let message = response
            .get("error")
            .and_then(|value| value.as_str())
            .unwrap_or("Carrier gossip push failed");
        anyhow::bail!("{message}");
    }

    pub async fn pull_gossip_messages(
        &self,
        topic: &str,
        limit: usize,
        skip_sender_id: Option<&str>,
    ) -> Result<Vec<GossipMessage>> {
        let (mut send, recv) = self.conn.open_bi().await?;
        let mut msg = serde_json::json!({
            "op": "gossip_pull",
            "topic": topic,
            "limit": limit,
        });
        if let Some(skip_sender_id) = skip_sender_id.filter(|value| !value.trim().is_empty()) {
            msg["skip_sender_id"] = serde_json::Value::String(skip_sender_id.to_string());
        }
        let mut bytes = serde_json::to_vec(&msg)?;
        bytes.push(b'\n');
        send.write_all(&bytes).await?;
        send.finish()?;

        let mut reader = BufReader::new(recv);
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let response: serde_json::Value = serde_json::from_str(line.trim())?;
        if response.get("ok").and_then(|value| value.as_bool()) != Some(true) {
            let message = response
                .get("error")
                .and_then(|value| value.as_str())
                .unwrap_or("Carrier gossip pull failed");
            anyhow::bail!("{message}");
        }
        response
            .get("messages")
            .and_then(|value| value.as_array())
            .map(|messages| {
                messages
                    .iter()
                    .cloned()
                    .map(serde_json::from_value)
                    .collect::<std::result::Result<Vec<GossipMessage>, _>>()
            })
            .transpose()?
            .ok_or_else(|| anyhow::anyhow!("Carrier gossip pull response missing messages"))
    }
}

async fn read_carrier_len_prefixed_bytes(
    recv: &mut iroh::endpoint::RecvStream,
    operation: &str,
) -> Result<Vec<u8>> {
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
                        .unwrap_or("Carrier returned an unknown error");
                    anyhow::bail!("{operation} failed: {msg}");
                }
            }
        }
        anyhow::bail!("{operation} returned invalid byte reply ({len} bytes declared)");
    }
    let mut content = vec![0u8; len];
    recv.read_exact(&mut content).await?;
    Ok(content)
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
    use std::sync::{Mutex as StdMutex, OnceLock};

    fn authenticated_test_source_endpoint() -> iroh::PublicKey {
        iroh::SecretKey::from_bytes(&[42u8; 32]).public()
    }

    fn test_topic_buffer(
        messages: impl IntoIterator<Item = GossipMessage>,
        base_index: u64,
    ) -> TopicBuffer {
        let messages: VecDeque<_> = messages.into_iter().collect();
        assert!(messages.len() <= MAX_BUFFER);
        let retained_bytes = messages
            .iter()
            .map(|message| {
                let encoded_len = gossip_message_encoded_len(message);
                assert!(encoded_len <= GOSSIP_WIRE_MESSAGE_MAX_BYTES);
                encoded_len
            })
            .sum();
        assert!(retained_bytes <= GOSSIP_TOPIC_BUFFER_MAX_BYTES);
        TopicBuffer {
            messages,
            base_index,
            retained_bytes,
        }
    }

    fn corrupt_test_topic_buffer(
        messages: impl IntoIterator<Item = GossipMessage>,
        base_index: u64,
    ) -> TopicBuffer {
        let messages: VecDeque<_> = messages.into_iter().collect();
        let retained_bytes = messages.iter().map(gossip_message_encoded_len).sum();
        TopicBuffer {
            messages,
            base_index,
            retained_bytes,
        }
    }

    fn gossip_test_message_with_encoded_len(index: u64, encoded_len: usize) -> GossipMessage {
        let mut message = cursor_test_message(index);
        message.content.clear();
        let empty_len = gossip_message_encoded_len(&message);
        assert!(encoded_len >= empty_len);
        message.content = "x".repeat(encoded_len - empty_len);
        assert_eq!(gossip_message_encoded_len(&message), encoded_len);
        message
    }

    fn env_lock() -> &'static StdMutex<()> {
        static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| StdMutex::new(()))
    }

    async fn shutdown_test_carrier_node(node: CarrierNode) {
        let tasks = {
            let mut state = node.gossip_state.lock().await;
            let tasks = state
                .receiver_tasks
                .drain()
                .map(|(_, task)| task)
                .collect::<Vec<_>>();
            state.senders.clear();
            state.joined_topics.clear();
            tasks
        };
        for task in tasks {
            task.abort();
            let _ = task.await;
        }
        tokio::time::timeout(Duration::from_secs(5), node.gossip.shutdown())
            .await
            .expect("test gossip shutdown timed out")
            .expect("test gossip shutdown failed");
        node.endpoint.close().await;
    }

    #[tokio::test]
    async fn test_carrier_gossip_leave_rejoin_and_shutdown_finish_receiver_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let (sk, did) = elastos_identity::derive_did(&[73u8; 32]);
        let node = start_carrier_node(&sk, &did, dir.path().to_path_buf())
            .await
            .unwrap();
        let provider = CarrierGossipProvider::new(node.gossip_state.clone());
        let topic = "__test/tracker-handle-teardown";

        let first_join = provider
            .send_raw(&serde_json::json!({
                "op": "gossip_join",
                "topic": topic,
                "mode": "direct",
            }))
            .await
            .unwrap();
        assert_eq!(first_join["status"], "ok");
        let first_task = node
            .gossip_state
            .lock()
            .await
            .receiver_tasks
            .get(topic)
            .expect("joined topic has a receiver task")
            .abort_handle();

        let leave = provider
            .send_raw(&serde_json::json!({"op": "gossip_leave", "topic": topic}))
            .await
            .unwrap();
        assert_eq!(leave["status"], "ok");
        assert!(first_task.is_finished());
        {
            let state = node.gossip_state.lock().await;
            assert!(!state.joined_topics.contains(topic));
            assert!(!state.senders.contains_key(topic));
            assert!(!state.receiver_tasks.contains_key(topic));
        }

        let second_join = provider
            .send_raw(&serde_json::json!({
                "op": "gossip_join",
                "topic": topic,
                "mode": "direct",
            }))
            .await
            .unwrap();
        assert_eq!(second_join["status"], "ok");
        let second_task = node
            .gossip_state
            .lock()
            .await
            .receiver_tasks
            .get(topic)
            .expect("rejoined topic has a receiver task")
            .abort_handle();

        shutdown_test_carrier_node(node).await;
        assert!(second_task.is_finished());
    }

    #[tokio::test]
    async fn test_carrier_tracker_leave_rejoin_and_shutdown_finish_receiver_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let (sk, did) = elastos_identity::derive_did(&[74u8; 32]);
        let node = start_carrier_node(&sk, &did, dir.path().to_path_buf())
            .await
            .unwrap();
        let provider = CarrierGossipProvider::new(node.gossip_state.clone());
        let topic = "__test/tracker-auto-discovery-handle-teardown";

        let first_join = provider
            .send_raw(&serde_json::json!({"op": "gossip_join", "topic": topic}))
            .await
            .unwrap();
        assert_eq!(first_join["status"], "ok");
        let first_task = node
            .gossip_state
            .lock()
            .await
            .receiver_tasks
            .get(topic)
            .expect("tracked topic has a receiver task")
            .abort_handle();

        let leave = provider
            .send_raw(&serde_json::json!({"op": "gossip_leave", "topic": topic}))
            .await
            .unwrap();
        assert_eq!(leave["status"], "ok");
        assert!(first_task.is_finished());

        let second_join = provider
            .send_raw(&serde_json::json!({"op": "gossip_join", "topic": topic}))
            .await
            .unwrap();
        assert_eq!(second_join["status"], "ok");
        let second_task = node
            .gossip_state
            .lock()
            .await
            .receiver_tasks
            .get(topic)
            .expect("rejoined tracked topic has a receiver task")
            .abort_handle();

        shutdown_test_carrier_node(node).await;
        assert!(second_task.is_finished());
    }

    #[tokio::test]
    async fn test_gossip_state_drop_aborts_receiver_tasks() {
        struct DropSignal(Option<tokio::sync::oneshot::Sender<()>>);

        impl Drop for DropSignal {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }

        let endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
            .bind()
            .await
            .unwrap();
        let gossip = Gossip::builder().spawn(endpoint.clone());
        let memory_lookup = MemoryLookup::new();
        let mut state =
            GossipState::new(endpoint.clone(), gossip.clone(), memory_lookup, None, None);
        let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _signal = DropSignal(Some(dropped_tx));
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
        });
        started_rx.await.expect("receiver task did not start");
        let task_abort = task.abort_handle();
        state.receiver_tasks.insert("drop-test".into(), task);

        drop(state);

        tokio::time::timeout(Duration::from_secs(5), dropped_rx)
            .await
            .expect("receiver task was not dropped")
            .expect("receiver task drop signal was lost");
        tokio::time::timeout(Duration::from_secs(5), async {
            while !task_abort.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("aborted receiver task did not finish");
        gossip.shutdown().await.unwrap();
        endpoint.close().await;
    }

    struct MockCarrierIpfsProvider;

    #[async_trait::async_trait]
    impl Provider for MockCarrierIpfsProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "mock ipfs provider only supports raw operations".into(),
            ))
        }

        fn schemes(&self) -> Vec<&'static str> {
            Vec::new()
        }

        fn name(&self) -> &'static str {
            "mock-carrier-ipfs-provider"
        }

        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            if request.get("op").and_then(|value| value.as_str()) != Some("cat") {
                return Ok(serde_json::json!({
                    "status": "error",
                    "code": "unsupported",
                    "message": "unsupported mock ipfs operation"
                }));
            }
            Ok(serde_json::json!({
                "status": "ok",
                "data": {
                    "data": base64::engine::general_purpose::STANDARD.encode(b"carrier content")
                }
            }))
        }
    }

    struct MockCarrierExitProvider {
        relay_path: Option<String>,
    }

    #[async_trait::async_trait]
    impl Provider for MockCarrierExitProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "mock exit provider only supports raw operations".into(),
            ))
        }

        fn schemes(&self) -> Vec<&'static str> {
            Vec::new()
        }

        fn name(&self) -> &'static str {
            "mock-carrier-exit-provider"
        }

        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            assert_eq!(
                request.get("op").and_then(|value| value.as_str()),
                Some("open_stream")
            );
            assert_eq!(
                request.get("target").and_then(|value| value.as_str()),
                Some("tls://example.com:443")
            );
            Ok(serde_json::json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.exit.stream-session/v1",
                    "backend": "remote-local-exit",
                    "stream_id": "stream:remote-local:test",
                    "target": "tls://example.com:443",
                    "byte_transport": if self.relay_path.is_some() { "adapter_ipc" } else { "not_attached" },
                    "relay_ipc": self.relay_path.as_ref().map(|path| serde_json::json!({
                        "schema": "elastos.exit.relay-ipc/v1",
                        "kind": "unix_socket",
                        "path": path,
                        "stream_id": "stream:remote-local:test"
                    }))
                }
            }))
        }
    }

    struct MockCarrierContentProvider;

    struct RecordingCarrierContentProvider {
        requests: Arc<StdMutex<Vec<serde_json::Value>>>,
    }

    #[async_trait::async_trait]
    impl Provider for MockCarrierContentProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "mock content provider only supports raw operations".into(),
            ))
        }

        fn schemes(&self) -> Vec<&'static str> {
            Vec::new()
        }

        fn name(&self) -> &'static str {
            "mock-carrier-content-provider"
        }

        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            if request.get("op").and_then(|op| op.as_str()) == Some("fetch")
                && request
                    .get("local_only")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
            {
                return Ok(serde_json::json!({
                    "status": "ok",
                    "data": {
                        "cid": request
                            .get("cid")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "stream": {
                            "schema": "elastos.provider.stream/v1",
                            "encoding": "base64-chunks",
                            "total_bytes": 22,
                            "completed": true,
                            "chunks": [
                                {
                                    "index": 0,
                                    "offset": 0,
                                    "length": 22,
                                    "data": base64::engine::general_purpose::STANDARD.encode(
                                        b"carrier provider bytes",
                                    ),
                                }
                            ],
                        }
                    }
                }));
            }
            Ok(serde_json::json!({
                "status": "ok",
                "data": {
                    "op": request.get("op").cloned().unwrap_or(serde_json::Value::Null),
                    "runtime_invocation": request
                        .get("_runtime_invocation")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                }
            }))
        }
    }

    #[async_trait::async_trait]
    impl Provider for RecordingCarrierContentProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "recording content provider only supports raw operations".into(),
            ))
        }

        fn schemes(&self) -> Vec<&'static str> {
            Vec::new()
        }

        fn name(&self) -> &'static str {
            "recording-carrier-content-provider"
        }

        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            self.requests.lock().unwrap().push(request.clone());
            Ok(serde_json::json!({
                "status": "ok",
                "data": {
                    "op": request.get("op").cloned().unwrap_or(serde_json::Value::Null),
                    "runtime_invocation": request
                        .get("_runtime_invocation")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                }
            }))
        }
    }

    struct MockCarrierObjectContentProvider;
    struct MockCarrierBlockGraphProvider;

    #[async_trait::async_trait]
    impl Provider for MockCarrierObjectContentProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "mock content provider only supports raw operations".into(),
            ))
        }

        fn schemes(&self) -> Vec<&'static str> {
            Vec::new()
        }

        fn name(&self) -> &'static str {
            "mock-carrier-object-content-provider"
        }

        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> std::result::Result<serde_json::Value, ProviderError> {
            if request.get("op").and_then(|op| op.as_str()) != Some("fetch")
                || !request
                    .get("local_only")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
            {
                return Ok(serde_json::json!({
                    "status": "error",
                    "code": "unsupported",
                    "message": "mock object content provider only supports local fetch"
                }));
            }
            let path = request
                .get("path")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let bytes = match path {
                CONTENT_OBJECT_MANIFEST_PATH => carrier_test_object_manifest_bytes(),
                "index.md" => carrier_test_object_file_bytes(),
                _ => {
                    return Ok(serde_json::json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "mock object path not found"
                    }))
                }
            };
            Ok(serde_json::json!({
                "status": "ok",
                "data": {
                    "cid": request
                        .get("cid")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                    "stream": carrier_test_stream_payload(&bytes)
                }
            }))
        }
    }

    #[async_trait::async_trait]
    impl Provider for MockCarrierBlockGraphProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "mock block-graph provider only supports raw operations".into(),
            ))
        }

        fn schemes(&self) -> Vec<&'static str> {
            Vec::new()
        }

        fn name(&self) -> &'static str {
            "mock-block-graph-provider"
        }

        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> std::result::Result<serde_json::Value, ProviderError> {
            if request.get("op").and_then(|op| op.as_str()) != Some("export_graph") {
                return Ok(serde_json::json!({
                    "status": "error",
                    "code": "unsupported",
                    "message": "mock block-graph provider only supports export_graph"
                }));
            }
            let cid = request
                .get("cid")
                .and_then(|value| value.as_str())
                .unwrap_or("bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi");
            Ok(serde_json::json!({
                "status": "ok",
                "data": {
                    "graph": {
                        "schema": CONTENT_BLOCK_GRAPH_SCHEMA,
                        "root_cid": cid,
                        "kind": "ipld_dag",
                        "blocks": [
                            {
                                "cid": cid,
                                "codec": "dag-pb",
                                "size": 22,
                                "data": base64::engine::general_purpose::STANDARD.encode(
                                    b"carrier provider bytes",
                                )
                            }
                        ],
                        "links": [],
                        "bytes": 22
                    }
                }
            }))
        }
    }

    #[derive(Default)]
    struct MockCarrierProviderPlaneInvoker {
        requests: Mutex<Vec<serde_json::Value>>,
        fail_ensure: bool,
        reject_admission: bool,
        omit_admission_receipt: bool,
    }

    #[async_trait::async_trait]
    impl ProviderCarrierInvoker for MockCarrierProviderPlaneInvoker {
        async fn invoke_carrier_provider(
            &self,
            route: &ProviderCarrierRoute,
            invocation: &ProviderInvocation,
            request: serde_json::Value,
        ) -> std::result::Result<serde_json::Value, ProviderError> {
            let ticket = match route {
                ProviderCarrierRoute::ConnectTicket { connect_ticket, .. } => {
                    connect_ticket.as_str()
                }
                ProviderCarrierRoute::PeerDid { .. } => "",
            };
            self.requests.lock().await.push(serde_json::json!({
                "ticket": ticket,
                "source": invocation.source.as_str(),
                "target": invocation.target.as_str(),
                "op": invocation.op.as_str(),
                "request": request,
            }));
            if invocation.transfer == ProviderTransfer::Stream {
                return Ok(serde_json::json!({
                    "status": "ok",
                    "data": {
                        "cid": "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
                        "stream": {
                            "schema": "elastos.provider.stream/v1",
                            "encoding": "base64-chunks",
                            "total_bytes": 22,
                            "completed": true,
                            "chunks": [
                                {
                                    "index": 0,
                                    "offset": 0,
                                    "length": 22,
                                    "data": base64::engine::general_purpose::STANDARD.encode(
                                        b"carrier provider bytes",
                                    ),
                                }
                            ],
                        }
                    }
                }));
            }
            if invocation.transfer == ProviderTransfer::Json && invocation.op == "admission" {
                let admission = serde_json::json!({
                    "schema": "elastos.content.admission/v1",
                    "policy": "content_provider_principal_quota_preflight",
                    "scope": "content-availability",
                    "accepted": !self.reject_admission,
                    "status": if self.reject_admission { "rejected" } else { "accepted" },
                    "reason": if self.reject_admission {
                        Some("mock remote quota exceeded")
                    } else {
                        None
                    },
                    "cid": request
                        .get("cid")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                    "publisher_did": request
                        .get("publisher_did")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                    "estimated_content_bytes": request
                        .get("estimated_content_bytes")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                    "quota": {
                        "policy": "principal_storage_quota",
                        "status": if self.reject_admission {
                            "quota_exceeded"
                        } else {
                            "within_quota"
                        },
                        "enforced": true
                    },
                    "checked_at": 1_700_000_000,
                    "app_visible": false
                });
                let receipt = if self.omit_admission_receipt {
                    serde_json::Value::Null
                } else {
                    signed_remote_admission_receipt(&admission)
                };
                return Ok(serde_json::json!({
                    "status": "ok",
                    "data": {
                        "cid": request
                            .get("cid")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "admission": admission,
                        "receipt": receipt
                    }
                }));
            }
            if invocation.transfer == ProviderTransfer::Json && invocation.op == "ensure" {
                let ensure_attempts = self
                    .requests
                    .lock()
                    .await
                    .iter()
                    .filter(|request| request["op"] == "ensure")
                    .count();
                if self.fail_ensure && ensure_attempts == 1 {
                    return Ok(serde_json::json!({
                        "status": "error",
                        "code": "pin_failed",
                        "message": "mock remote pin failed"
                    }));
                }
                return Ok(serde_json::json!({
                    "status": "ok",
                    "data": {
                        "cid": request
                            .get("cid")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "receipt": signed_remote_content_receipt(
                            request
                                .get("cid")
                                .and_then(|value| value.as_str())
                                .unwrap_or("bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi")
                        ),
                        "availability": {
                            "status": "local_pinned",
                            "provider": "content-provider",
                            "policy": "carrier_replica",
                            "replicas": 1,
                            "peer_selection": {
                                "mode": "single_local",
                                "live_multi_peer_proof": false
                            },
                            "quota": {
                                "policy": "not_enforced"
                            },
                            "repair_worker": {
                                "scheduled": false,
                                "status": "not_scheduled"
                            }
                        }
                    }
                }));
            }
            if invocation.transfer == ProviderTransfer::Json && invocation.op == "import_exact" {
                return Ok(serde_json::json!({
                    "status": "ok",
                    "data": {
                        "cid": request
                            .get("cid")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "receipt": signed_remote_content_receipt(
                            request
                                .get("cid")
                                .and_then(|value| value.as_str())
                                .unwrap_or("bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi")
                        ),
                        "availability": {
                            "status": "local_pinned",
                            "provider": "content-provider",
                            "policy": "carrier_exact_import",
                            "replicas": 1,
                            "peer_selection": {
                                "mode": "single_local",
                                "live_multi_peer_proof": false
                            },
                            "quota": {
                                "policy": "not_enforced"
                            },
                            "repair_worker": {
                                "scheduled": false,
                                "status": "not_scheduled"
                            }
                        },
                        "import": {
                            "schema": "elastos.content.import-exact/v1",
                            "verified_cid": true,
                            "bytes": 22
                        }
                    }
                }));
            }
            if invocation.transfer == ProviderTransfer::Json && invocation.op == "import_object" {
                return Ok(serde_json::json!({
                    "status": "ok",
                    "data": {
                        "cid": request
                            .get("cid")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "receipt": signed_remote_content_receipt(
                            request
                                .get("cid")
                                .and_then(|value| value.as_str())
                                .unwrap_or("bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi")
                        ),
                        "availability": {
                            "status": "local_pinned",
                            "provider": "content-provider",
                            "policy": "carrier_object_import",
                            "replicas": 1,
                            "peer_selection": {
                                "mode": "single_local",
                                "live_multi_peer_proof": false
                            },
                            "quota": {
                                "policy": "not_enforced"
                            },
                            "repair_worker": {
                                "scheduled": false,
                                "status": "not_scheduled"
                            }
                        },
                        "import": {
                            "schema": "elastos.content.import-object/v1",
                            "verified_cid": true,
                            "files": request
                                .get("files")
                                .and_then(|value| value.as_array())
                                .map(|files| files.len())
                                .unwrap_or(0)
                        }
                    }
                }));
            }
            if invocation.transfer == ProviderTransfer::Json && invocation.op == "import_graph" {
                return Ok(serde_json::json!({
                    "status": "ok",
                    "data": {
                        "cid": request
                            .get("cid")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "receipt": signed_remote_content_receipt(
                            request
                                .get("cid")
                                .and_then(|value| value.as_str())
                                .unwrap_or("bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi")
                        ),
                        "availability": {
                            "status": "local_pinned",
                            "provider": CONTENT_BLOCK_GRAPH_PROVIDER,
                            "policy": "carrier_block_graph_import",
                            "replicas": 1,
                            "peer_selection": {
                                "mode": "single_local",
                                "live_multi_peer_proof": false
                            },
                            "quota": {
                                "policy": "not_enforced"
                            },
                            "repair_worker": {
                                "scheduled": false,
                                "status": "not_scheduled"
                            },
                            "repair_graph": {
                                "schema": CONTENT_REPAIR_GRAPH_SCHEMA,
                                "policy": "carrier_provider_bounded_graph_repair",
                                "requested_kind": "ipld_dag",
                                "status": "block_graph_provider_imported"
                            }
                        },
                        "import": {
                            "schema": CONTENT_BLOCK_GRAPH_SCHEMA,
                            "verified_cid": true,
                            "blocks": request
                                .get("graph")
                                .and_then(|graph| graph.get("blocks"))
                                .and_then(|value| value.as_array())
                                .map(|blocks| blocks.len())
                                .unwrap_or(0)
                        }
                    }
                }));
            }
            if invocation.transfer == ProviderTransfer::Json && invocation.op == "status" {
                return Ok(serde_json::json!({
                    "status": "ok",
                    "data": {
                        "cid": request
                            .get("cid")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "availability": {
                            "status": "local_pinned",
                            "provider": "content-provider",
                            "policy": "local_repair_pin",
                            "replicas": 1,
                            "peer_selection": {
                                "mode": "single_local",
                                "live_multi_peer_proof": false
                            },
                            "quota": {
                                "policy": "not_enforced"
                            },
                            "repair_worker": {
                                "scheduled": false,
                                "status": "not_scheduled"
                            }
                        }
                    }
                }));
            }
            Ok(serde_json::json!({
                "status": "ok",
                "data": {
                    "data": {
                        "data": base64::engine::general_purpose::STANDARD.encode(
                            b"carrier provider bytes",
                        )
                    }
                }
            }))
        }
    }

    #[test]
    fn test_topic_hash_deterministic() {
        let h1 = topic_hash("#general");
        let h2 = topic_hash("#general");
        assert_eq!(h1, h2, "same topic name must produce same hash");

        let h3 = topic_hash("#other");
        assert_ne!(h1, h3, "different topics must produce different hashes");
    }

    #[test]
    fn test_topic_hash_matches_distributed_topic_tracker_topic_id() {
        let topic_name = "__elastos_internal/carrier-test-v1/topic";
        let mut expected = [0u8; 32];
        expected.copy_from_slice(&Sha512::digest(topic_name.as_bytes())[..32]);

        assert_eq!(
            topic_hash(topic_name),
            iroh_gossip::proto::TopicId::from(expected)
        );
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
    fn test_content_availability_topic_is_deterministic_and_does_not_embed_raw_cid() {
        let cid = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";
        let topic = content_availability_topic_name(cid);
        let topic_again = content_availability_topic_name(cid);
        let uri = content_availability_topic_uri(cid);

        assert_eq!(topic, topic_again);
        assert!(topic.starts_with("__elastos_content/v1/"));
        assert!(uri.starts_with("elastos://carrier/content/"));
        assert!(uri.ends_with("/availability"));
        assert!(!topic.contains(cid));
        assert!(!uri.contains(cid));
    }

    #[test]
    fn test_content_availability_cid_validation_is_fail_closed() {
        assert!(validate_content_cid(
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"
        )
        .is_ok());
        assert!(validate_content_cid("QmRSEtAyq7Xgr5YCFVWuYsBdqbR5X9fJDsdpNQuvm9yaic").is_ok());
        assert!(validate_content_cid("short").is_err());
        assert!(validate_content_cid("cid/with/slashes").is_err());
        assert!(validate_content_cid("cid with spaces").is_err());
    }

    #[test]
    fn test_content_availability_fetch_tickets_require_signed_matching_announcement() {
        let cid = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";
        let (signed_message, _) = signed_content_availability_message(
            cid,
            [19u8; 32],
            "ticket:test",
            "endpoint:test",
            1_700_000_000,
        );
        let unsigned_message = GossipMessage {
            content: serde_json::json!({
                "payload": {
                    "schema": CONTENT_AVAILABILITY_ANNOUNCEMENT_SCHEMA,
                    "cid": cid,
                    "node_did": "did:key:z6Mkuntrusted",
                    "fetch": {"connect_ticket": "ticket:unsigned"}
                },
                "signature": "00",
                "signer_did": "did:key:z6Mkuntrusted"
            })
            .to_string(),
            ..signed_message.clone()
        };

        let tickets = content_availability_fetch_tickets(&[unsigned_message, signed_message], cid);

        assert_eq!(tickets, vec!["ticket:test".to_string()]);
    }

    #[test]
    fn test_content_availability_replicas_ignore_signed_repair_only_announcements() {
        let cid = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";
        let (sk, did) = elastos_identity::derive_did(&[23u8; 32]);
        let payload = serde_json::json!({
            "schema": CONTENT_AVAILABILITY_ANNOUNCEMENT_SCHEMA,
            "cid": cid,
            "uri": format!("elastos://{cid}"),
            "policy": "network_default",
            "provider": "carrier-availability",
            "node_did": did,
            "topic": content_availability_topic_uri(cid),
            "local": {
                "status": "local_unpinned",
                "provider": "ipfs-provider",
                "replicas": 0
            },
            "announced_at": 1_700_000_000u64
        });
        let canonical = serde_json::to_string(&payload).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &sk,
            CONTENT_AVAILABILITY_ANNOUNCEMENT_DOMAIN,
            canonical.as_bytes(),
        );
        let message = GossipMessage {
            sender_id: signer_did.clone(),
            sender_nick: "content-provider".to_string(),
            content: serde_json::json!({
                "payload": payload,
                "signature": signature,
                "signer_did": signer_did,
            })
            .to_string(),
            ts: 1_700_000_000,
            nonce: 1,
            signature: None,
            sender_session_id: None,
        };

        assert!(
            content_availability_replicas(&[message], cid).is_empty(),
            "repair-only announcements must not become fetch/replication candidates"
        );
    }

    #[test]
    fn test_content_availability_replicas_ignore_oversized_candidate_metadata() {
        let cid = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";
        let (oversized_ticket, _) = signed_content_availability_message(
            cid,
            [24u8; 32],
            &"x".repeat(MAX_CARRIER_AVAILABILITY_TICKET_LEN + 1),
            "remote-endpoint",
            1_700_000_000,
        );
        let (oversized_endpoint, _) = signed_content_availability_message(
            cid,
            [25u8; 32],
            "ticket:test",
            &"e".repeat(MAX_CARRIER_AVAILABILITY_ENDPOINT_ID_LEN + 1),
            1_700_000_000,
        );

        let replicas = content_availability_replicas(&[oversized_ticket, oversized_endpoint], cid);

        assert!(
            replicas.is_empty(),
            "oversized candidate metadata must not be used for Carrier provider invocation"
        );
    }

    #[test]
    fn test_content_availability_replicas_are_scored_and_sorted() {
        let cid = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";
        let (mut stale, stale_did) =
            signed_content_availability_message(cid, [26u8; 32], "ticket:stale", "", 10);
        stale.ts = 1_700_000_000;
        let (fresh, fresh_did) = signed_content_availability_message(
            cid,
            [27u8; 32],
            "ticket:fresh",
            "remote-endpoint",
            1_700_000_000,
        );

        let replicas = content_availability_replicas(&[stale, fresh], cid);

        assert_eq!(replicas.len(), 2);
        assert_eq!(replicas[0].node_did, fresh_did);
        assert_eq!(replicas[0].connect_ticket, "ticket:fresh");
        assert_eq!(replicas[0].score, 90);
        assert_eq!(
            replicas[0].selection_reason,
            "signed_announcement+endpoint_advertised+fresh+local_reputation_neutral"
        );
        assert_eq!(replicas[0].reputation_score, 0);
        assert_eq!(replicas[0].reputation_reason, "no_local_history");
        assert_eq!(replicas[1].node_did, stale_did);
        assert_eq!(replicas[1].score, 50);
        assert_eq!(
            replicas[1].selection_reason,
            "signed_announcement+stale+local_reputation_neutral"
        );
    }

    #[test]
    fn test_content_availability_replicas_apply_local_runtime_reputation() {
        let cid = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";
        let (preferred, preferred_did) = signed_content_availability_message(
            cid,
            [28u8; 32],
            "ticket:preferred",
            "remote-endpoint",
            1_700_000_000,
        );
        let (penalized, penalized_did) = signed_content_availability_message(
            cid,
            [29u8; 32],
            "ticket:penalized",
            "remote-endpoint",
            1_700_000_000,
        );
        let mut reputation = HashMap::new();
        reputation.insert(
            preferred_did.clone(),
            CarrierPeerReputation {
                successes: 2,
                failures: 0,
            },
        );
        reputation.insert(
            penalized_did.clone(),
            CarrierPeerReputation {
                successes: 0,
                failures: 2,
            },
        );

        let replicas = content_availability_replicas_with_reputation(
            &[penalized, preferred],
            cid,
            &reputation,
        );

        assert_eq!(replicas.len(), 2);
        assert_eq!(replicas[0].node_did, preferred_did);
        assert_eq!(replicas[0].score, 98);
        assert_eq!(replicas[0].reputation_score, 8);
        assert_eq!(
            replicas[0].reputation_reason,
            "local_runtime_successes:2;failures:0"
        );
        assert_eq!(replicas[1].node_did, penalized_did);
        assert_eq!(replicas[1].score, 74);
        assert_eq!(replicas[1].reputation_score, -16);
    }

    #[test]
    fn test_carrier_peer_reputation_persists_local_history() {
        let data_dir = tempfile::tempdir().unwrap();
        let mut reputation = HashMap::new();
        reputation.insert(
            "did:key:zDurablePeer".to_string(),
            CarrierPeerReputation {
                successes: 3,
                failures: 1,
            },
        );

        save_carrier_peer_reputation(data_dir.path(), &reputation).unwrap();
        let loaded = load_carrier_peer_reputation(data_dir.path());

        let peer = loaded.get("did:key:zDurablePeer").unwrap();
        assert_eq!(peer.successes, 3);
        assert_eq!(peer.failures, 1);
        assert!(carrier_peer_reputation_path(data_dir.path()).is_file());
    }

    fn signed_content_availability_message(
        cid: &str,
        key_seed: [u8; 32],
        connect_ticket: &str,
        endpoint_id: &str,
        announced_at: u64,
    ) -> (GossipMessage, String) {
        let (sk, did) = elastos_identity::derive_did(&key_seed);
        let payload = serde_json::json!({
            "schema": CONTENT_AVAILABILITY_ANNOUNCEMENT_SCHEMA,
            "cid": cid,
            "uri": format!("elastos://{cid}"),
            "policy": "network_default",
            "provider": "carrier-availability",
            "node_did": did,
            "topic": content_availability_topic_uri(cid),
            "fetch": {
                "transport": "carrier-file",
                "endpoint_id": endpoint_id,
                "connect_ticket": connect_ticket
            },
            "local": {
                "status": "local_pinned",
                "provider": "ipfs-provider",
                "replicas": 1
            },
            "announced_at": announced_at
        });
        let canonical = serde_json::to_string(&payload).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &sk,
            CONTENT_AVAILABILITY_ANNOUNCEMENT_DOMAIN,
            canonical.as_bytes(),
        );
        (
            GossipMessage {
                sender_id: signer_did.clone(),
                sender_nick: "content-provider".to_string(),
                content: serde_json::json!({
                    "payload": payload,
                    "signature": signature,
                    "signer_did": signer_did,
                })
                .to_string(),
                ts: announced_at,
                nonce: 1,
                signature: None,
                sender_session_id: None,
            },
            did,
        )
    }

    fn signed_remote_content_receipt(cid: &str) -> serde_json::Value {
        let (sk, _) = elastos_identity::derive_did(&[44u8; 32]);
        let checked_at = 1_700_000_123u64;
        let payload = serde_json::json!({
            "schema": "elastos.content.availability.receipt/v1",
            "cid": cid,
            "uri": format!("elastos://{cid}"),
            "provider": "content-provider",
            "policy": "carrier_exact_import",
            "status": "local_pinned",
            "replicas": 1,
            "peer_selection": {
                "mode": "single_local",
                "live_multi_peer_proof": false
            },
            "quota": {
                "policy": "carrier_provider_quota",
                "status": "within_quota",
                "enforced": true,
                "used_replicas": 1,
                "effective_max_replicas": 3
            },
            "repair_worker": {
                "scheduled": false,
                "status": "not_scheduled",
                "worker": "content-provider"
            },
            "repair_graph": {
                "schema": CONTENT_REPAIR_GRAPH_SCHEMA,
                "policy": "carrier_provider_bounded_graph_repair",
                "requested_kind": "auto",
                "status": "bounded_import_supported",
                "refuses_exact_fallback_for_arbitrary_dag": true
            },
            "storage_market": {
                "schema": "elastos.content.storage-market/v1",
                "mode": "carrier_provider_receipts",
                "status": "receipt_proven_no_market_settlement",
                "settlement": "not_configured",
                "quota_enforced": true
            },
            "accounting": {
                "schema": "elastos.content.accounting/v1",
                "observed": true,
                "files": 1,
                "content_bytes": 22,
                "replica_bytes_estimate": 22,
                "storage_quota": {
                    "status": "observed_not_enforced"
                }
            },
            "abuse_controls": {
                "schema": "elastos.content.abuse-controls/v1",
                "policy": "carrier_provider_invocation_guardrail",
                "enforced": true,
                "candidate_count": 1,
                "attempt_limit": 1,
                "attempted_operations": 1,
                "failed_operations": 0,
                "throttled": false
            },
            "checked_at": checked_at,
        });
        let canonical = serde_json::to_string(&payload).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &sk,
            "elastos.content.availability.receipt.v1",
            canonical.as_bytes(),
        );
        serde_json::json!({
            "payload": payload,
            "signature": signature,
            "signer_did": signer_did,
        })
    }

    fn signed_remote_admission_receipt(payload: &serde_json::Value) -> serde_json::Value {
        let (sk, _) = elastos_identity::derive_did(&[45u8; 32]);
        let canonical = serde_json::to_string(payload).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &sk,
            CONTENT_ADMISSION_DOMAIN,
            canonical.as_bytes(),
        );
        serde_json::json!({
            "payload": payload,
            "signature": signature,
            "signer_did": signer_did,
        })
    }

    fn signed_peer_attestation_exchange_receipt(payload: serde_json::Value) -> serde_json::Value {
        let (sk, _) = elastos_identity::derive_did(&[46u8; 32]);
        let canonical = serde_json::to_string(&payload).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &sk,
            CARRIER_PEER_ATTESTATION_EXCHANGE_RECEIPT_DOMAIN,
            canonical.as_bytes(),
        );
        serde_json::json!({
            "payload": payload,
            "signature": signature,
            "signer_did": signer_did,
        })
    }

    fn carrier_peer_attestation_test_proof() -> CarrierReplicationProof {
        CarrierReplicationProof {
            node_did: "did:key:zRemote".to_string(),
            endpoint_id: Some("remote-endpoint".to_string()),
            announced_at: 1_700_000_000,
            score: 90,
            selection_reason: "signed_announcement+endpoint_advertised+fresh".to_string(),
            reputation_score: 4,
            reputation_reason: "local_runtime_successes:1;failures:0".to_string(),
            ensure_status: "ok".to_string(),
            admission: Some(serde_json::json!({
                "accepted": true,
                "quota": {"status": "within_quota"}
            })),
            status_availability: serde_json::json!({
                "status": "local_pinned",
                "replicas": 1,
            }),
            remote_receipt: Some(serde_json::json!({
                "schema": "elastos.content.availability.receipt/v1",
                "cid": "bafyattest",
                "status": "network_available",
                "signer_did": "did:key:zRemoteContentProvider",
                "verified": true,
            })),
            transfer: Some(serde_json::json!({
                "transport": "carrier-provider-plane",
                "carrier": {
                    "route": "connect_ticket",
                    "connect_ticket": "ticket:internal-secret",
                }
            })),
            checked_at: 1_700_000_001,
        }
    }

    fn spawn_peer_attestation_exchange_endpoint(
        response: serde_json::Value,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let body = response.to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = std::io::Read::read(&mut stream, &mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if http_request_complete(&request) {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            std::io::Write::write_all(&mut stream, response.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (format!("http://{addr}/peer-attestation/exchange"), handle)
    }

    fn http_request_complete(request: &[u8]) -> bool {
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            return false;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.eq_ignore_ascii_case("content-length") {
                    value.trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0);
        request.len() >= header_end + 4 + content_length
    }

    fn carrier_test_stream_payload(bytes: &[u8]) -> serde_json::Value {
        serde_json::json!({
            "schema": "elastos.provider.stream/v1",
            "encoding": "base64-chunks",
            "total_bytes": bytes.len(),
            "completed": true,
            "chunks": [{
                "index": 0,
                "offset": 0,
                "length": bytes.len(),
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }],
        })
    }

    fn carrier_test_object_file_bytes() -> Vec<u8> {
        b"# Carrier Object\n".to_vec()
    }

    fn carrier_test_object_manifest_bytes() -> Vec<u8> {
        let bytes = carrier_test_object_file_bytes();
        let file_sha = format!("{:x}", Sha256::digest(&bytes));
        let mut hasher = Sha256::new();
        hasher.update(b"index.md");
        hasher.update(b"\0");
        hasher.update(file_sha.as_bytes());
        hasher.update(b"\0");
        hasher.update(bytes.len().to_string().as_bytes());
        hasher.update(b"\0");
        let manifest = serde_json::json!({
            "schema": "elastos.content.object.manifest/v1",
            "kind": "document",
            "content_digest": format!("sha256:{:x}", hasher.finalize()),
            "files": [{
                "path": "index.md",
                "sha256": file_sha,
                "size": bytes.len()
            }],
            "links": [],
            "object_did": "did:key:zObject",
            "publisher_did": "did:key:zPublisher"
        });
        serde_json::to_vec(&manifest).unwrap()
    }

    #[tokio::test]
    async fn test_carrier_provider_invoke_dispatches_runtime_enveloped_request() {
        let registry = ProviderRegistry::new();
        registry
            .register_sub_provider("content", Arc::new(MockCarrierContentProvider))
            .await
            .unwrap();
        let request = serde_json::json!({
            "source": "carrier-availability",
            "target": "content",
            "operation": "fetch",
            "transfer": "bytes",
            "request": {
                "op": "fetch",
                "_runtime_invocation": {
                    "schema": "elastos.provider.invocation/v1",
                    "source": "carrier-availability",
                    "target": "content",
                    "op": "fetch",
                    "capability": "provider:carrier-availability->content:fetch",
                    "transport": "carrier-provider-plane",
                    "carrier": null,
                    "transfer": "bytes",
                    "range": null,
                    "progress": null
                }
            }
        });

        let source_endpoint = authenticated_test_source_endpoint();
        let response = carrier_provider_invoke_registry(&registry, &request, &source_endpoint)
            .await
            .unwrap();

        assert_eq!(response["ok"], true);
        assert_eq!(response["result"]["status"], "ok");
        assert_eq!(response["result"]["data"]["op"], "fetch");
        assert_eq!(
            response["result"]["data"]["runtime_invocation"]["transport"],
            "carrier-provider-plane"
        );
        assert_eq!(
            response["result"]["data"]["runtime_invocation"]["carrier"]["source_endpoint_did"],
            public_key_to_did(&source_endpoint)
        );
        assert!(!response.to_string().contains("\"connect_ticket\":"));
    }

    struct PeerDidRouteFixture {
        local_registry: Arc<ProviderRegistry>,
        remote_requests: Arc<StdMutex<Vec<serde_json::Value>>>,
        remote_did: String,
        remote_addr: iroh::EndpointAddr,
        local_node: CarrierNode,
        remote_node: CarrierNode,
        _remote_registry: Arc<ProviderRegistry>,
        _local_dir: tempfile::TempDir,
        _remote_dir: tempfile::TempDir,
    }

    /// Deterministic peer-DID resolver setup.
    ///
    /// Runtime keeps one long-lived Carrier endpoint and owns peer resolution.
    /// Seeding the remote node's verified address into that endpoint's address
    /// book makes resolution in-process, so a routed invocation does not depend
    /// on public discovery timing. Nothing here is visible to apps: the route
    /// stays a peer DID and the address never leaves Runtime.
    async fn peer_did_route_fixture(local_seed: u8, remote_seed: u8) -> PeerDidRouteFixture {
        let local_dir = tempfile::tempdir().unwrap();
        let local_registry = Arc::new(ProviderRegistry::new());
        let (local_sk, local_did) = elastos_identity::derive_did(&[local_seed; 32]);
        let local_node = start_carrier_node_with_registry(
            &local_sk,
            &local_did,
            local_dir.path().to_path_buf(),
            Some(Arc::downgrade(&local_registry)),
        )
        .await
        .unwrap();
        local_registry
            .set_carrier_invoker(Arc::new(CarrierProviderInvoker::with_carrier_endpoint(
                local_node.endpoint.clone(),
            )))
            .await;

        let remote_requests = Arc::new(StdMutex::new(Vec::new()));
        let remote_registry = Arc::new(ProviderRegistry::new());
        remote_registry
            .register_sub_provider(
                "content",
                Arc::new(RecordingCarrierContentProvider {
                    requests: remote_requests.clone(),
                }),
            )
            .await
            .unwrap();
        let remote_dir = tempfile::tempdir().unwrap();
        let (remote_sk, remote_did) = elastos_identity::derive_did(&[remote_seed; 32]);
        let remote_node = start_carrier_node_with_registry(
            &remote_sk,
            &remote_did,
            remote_dir.path().to_path_buf(),
            Some(Arc::downgrade(&remote_registry)),
        )
        .await
        .unwrap();

        let mut remote_watch = remote_node.endpoint.watch_addr();
        let remote_addr = remote_watch.get();
        local_node
            .memory_lookup
            .add_endpoint_info(remote_addr.clone());

        PeerDidRouteFixture {
            local_registry,
            remote_requests,
            remote_did,
            remote_addr,
            local_node,
            remote_node,
            _remote_registry: remote_registry,
            _local_dir: local_dir,
            _remote_dir: remote_dir,
        }
    }

    fn peer_did_invocation(peer_did: &str) -> ProviderInvocation {
        ProviderInvocation {
            source: "content-provider".to_string(),
            target: "content".to_string(),
            op: "fetch".to_string(),
            request: serde_json::json!({ "op": "fetch" }),
            transfer: ProviderTransfer::Json,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Carrier(ProviderCarrierRoute::PeerDid {
                peer_did: peer_did.to_string(),
                timeout_ms: Some(5_000),
            }),
        }
    }

    fn connect_ticket_invocation(
        remote_addr: iroh::EndpointAddr,
        remote_did: &str,
    ) -> ProviderInvocation {
        ProviderInvocation {
            source: "content-provider".to_string(),
            target: "content".to_string(),
            op: "fetch".to_string(),
            request: serde_json::json!({ "op": "fetch" }),
            transfer: ProviderTransfer::Json,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Carrier(ProviderCarrierRoute::ConnectTicket {
                connect_ticket: encode_ticket_for(remote_addr),
                peer_did: Some(remote_did.to_string()),
                timeout_ms: Some(5_000),
            }),
        }
    }

    #[tokio::test]
    async fn test_peer_did_route_reaches_resolved_provider_without_exposing_topology() {
        let fixture = peer_did_route_fixture(60, 61).await;

        let response = fixture
            .local_registry
            .invoke_provider(peer_did_invocation(&fixture.remote_did))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["op"], "fetch");
        assert_eq!(
            response["data"]["runtime_invocation"]["transport"],
            "carrier-provider-plane"
        );
        assert_eq!(
            response["data"]["runtime_invocation"]["carrier"],
            serde_json::Value::Null
        );

        let rendered = response.to_string();
        assert!(!rendered.contains("connect_ticket"));
        assert!(!rendered.contains("did:key:"));
        assert!(!rendered.contains("127.0.0.1") && !rendered.contains("localhost"));

        let requests = fixture.remote_requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0]["_runtime_invocation"]["carrier"]["source_endpoint_did"],
            public_key_to_did(&fixture.local_node.endpoint.id())
        );
        assert_eq!(
            requests[0]["_runtime_invocation"]["source"],
            "content-provider"
        );
        assert_eq!(requests[0]["_runtime_invocation"]["target"], "content");
        drop(requests);

        shutdown_test_carrier_node(fixture.remote_node).await;
        shutdown_test_carrier_node(fixture.local_node).await;
    }

    #[tokio::test]
    async fn test_connect_ticket_route_reuses_runtime_endpoint_identity() {
        let fixture = peer_did_route_fixture(72, 73).await;

        for expected in 1..=2usize {
            let response = fixture
                .local_registry
                .invoke_provider(connect_ticket_invocation(
                    fixture.remote_addr.clone(),
                    &fixture.remote_did,
                ))
                .await
                .unwrap();

            assert_eq!(response["status"], "ok");
            assert_eq!(fixture.remote_requests.lock().unwrap().len(), expected);
            assert_eq!(
                fixture.remote_requests.lock().unwrap()[expected - 1]["_runtime_invocation"]
                    ["carrier"]["source_endpoint_did"],
                public_key_to_did(&fixture.local_node.endpoint.id())
            );
            assert!(!fixture.local_node.endpoint.is_closed());
        }

        shutdown_test_carrier_node(fixture.remote_node).await;
        shutdown_test_carrier_node(fixture.local_node).await;
    }

    #[tokio::test]
    async fn test_connect_ticket_route_without_runtime_endpoint_fails_before_provider_effect() {
        let fixture = peer_did_route_fixture(74, 75).await;
        fixture
            .local_registry
            .set_carrier_invoker(Arc::new(CarrierProviderInvoker::new()))
            .await;

        let err = fixture
            .local_registry
            .invoke_provider(connect_ticket_invocation(
                fixture.remote_addr.clone(),
                &fixture.remote_did,
            ))
            .await
            .expect_err("ticket route without the Runtime endpoint must fail closed");

        assert!(
            matches!(err, ProviderError::Unavailable(_)),
            "expected typed peer-unavailable, got {err:?}"
        );
        assert!(fixture.remote_requests.lock().unwrap().is_empty());

        shutdown_test_carrier_node(fixture.remote_node).await;
        shutdown_test_carrier_node(fixture.local_node).await;
    }

    #[tokio::test]
    async fn test_peer_did_route_without_runtime_endpoint_is_unavailable_before_provider_effect() {
        let fixture = peer_did_route_fixture(62, 63).await;
        // Rebind the registry to an invoker that owns no Carrier endpoint, so
        // Runtime has no verified route to any peer.
        fixture
            .local_registry
            .set_carrier_invoker(Arc::new(CarrierProviderInvoker::new()))
            .await;

        let err = fixture
            .local_registry
            .invoke_provider(peer_did_invocation(&fixture.remote_did))
            .await
            .expect_err("no verified route must fail closed");

        assert!(
            matches!(err, ProviderError::Unavailable(_)),
            "expected typed peer-unavailable, got {err:?}"
        );
        let rendered = err.to_string();
        assert!(!rendered.contains("did:key:"));
        assert!(!rendered.contains("connect_ticket"));
        assert!(!rendered.contains("127.0.0.1") && !rendered.contains("localhost"));
        assert!(fixture.remote_requests.lock().unwrap().is_empty());

        shutdown_test_carrier_node(fixture.remote_node).await;
        shutdown_test_carrier_node(fixture.local_node).await;
    }

    #[tokio::test]
    async fn test_peer_did_route_rejects_substituted_identity_before_provider_effect() {
        let fixture = peer_did_route_fixture(64, 65).await;

        // A DID Runtime cannot resolve at all.
        let (_, unknown_did) = elastos_identity::derive_did(&[66u8; 32]);
        let err = fixture
            .local_registry
            .invoke_provider(peer_did_invocation(&unknown_did))
            .await
            .expect_err("unresolvable peer must fail closed");
        assert!(
            matches!(err, ProviderError::Unavailable(_)),
            "expected typed peer-unavailable, got {err:?}"
        );

        // A DID whose address book entry points at another peer's transport
        // address. The resolved endpoint identity must still have to match the
        // requested DID.
        let (_, substituted_did) = elastos_identity::derive_did(&[67u8; 32]);
        let substituted_key = did_to_public_key(&substituted_did).unwrap();
        let mut substituted_addr = iroh::EndpointAddr::from(substituted_key);
        substituted_addr
            .addrs
            .extend(fixture.remote_addr.addrs.iter().cloned());
        fixture
            .local_node
            .memory_lookup
            .add_endpoint_info(substituted_addr);

        let err = fixture
            .local_registry
            .invoke_provider(peer_did_invocation(&substituted_did))
            .await
            .expect_err("substituted peer identity must fail closed");
        assert!(
            matches!(err, ProviderError::Unavailable(_)),
            "expected typed peer-unavailable, got {err:?}"
        );

        assert!(
            fixture.remote_requests.lock().unwrap().is_empty(),
            "a rejected route must not reach the remote provider"
        );

        shutdown_test_carrier_node(fixture.remote_node).await;
        shutdown_test_carrier_node(fixture.local_node).await;
    }

    #[tokio::test]
    async fn test_peer_did_route_executes_each_invocation_exactly_once() {
        let fixture = peer_did_route_fixture(68, 69).await;

        for expected in 1..=3usize {
            fixture
                .local_registry
                .invoke_provider(peer_did_invocation(&fixture.remote_did))
                .await
                .unwrap();
            assert_eq!(fixture.remote_requests.lock().unwrap().len(), expected);
        }

        shutdown_test_carrier_node(fixture.remote_node).await;
        shutdown_test_carrier_node(fixture.local_node).await;
    }

    #[tokio::test]
    async fn test_peer_did_route_serves_concurrent_invocations_exactly_once_each() {
        let fixture = peer_did_route_fixture(70, 71).await;

        let mut handles = Vec::new();
        for _ in 0..8 {
            let registry = fixture.local_registry.clone();
            let peer_did = fixture.remote_did.clone();
            handles.push(tokio::spawn(async move {
                registry
                    .invoke_provider(peer_did_invocation(&peer_did))
                    .await
            }));
        }

        let mut ok = 0;
        for handle in handles {
            let response = handle.await.unwrap().unwrap();
            assert_eq!(response["status"], "ok");
            ok += 1;
        }
        assert_eq!(ok, 8);
        assert_eq!(fixture.remote_requests.lock().unwrap().len(), 8);

        shutdown_test_carrier_node(fixture.remote_node).await;
        shutdown_test_carrier_node(fixture.local_node).await;
    }

    #[tokio::test]
    async fn test_browser_carrier_exit_stream_requires_remote_exit_relay_ipc() {
        let request = serde_json::json!({
            "schema": BROWSER_CARRIER_STREAM_SCHEMA,
            "carrier_service": "elastos://exit/open_stream",
            "grant_id": "operator-grant:server-exit:alice",
            "stream_id": "remote-carrier:server-exit:test",
            "target": "tls://example.com:443",
            "principal_id": "person:local:alice",
            "reason": "browser test"
        });

        let registry = ProviderRegistry::new();
        registry
            .register_sub_provider(
                "exit",
                Arc::new(MockCarrierExitProvider {
                    relay_path: Some("/tmp/elastos-remote-exit.sock".to_string()),
                }),
            )
            .await
            .unwrap();
        let relay_path = browser_carrier_exit_relay_path(&registry, &request)
            .await
            .unwrap();
        assert_eq!(relay_path, PathBuf::from("/tmp/elastos-remote-exit.sock"));

        let registry = ProviderRegistry::new();
        registry
            .register_sub_provider(
                "exit",
                Arc::new(MockCarrierExitProvider { relay_path: None }),
            )
            .await
            .unwrap();
        let err = browser_carrier_exit_relay_path(&registry, &request)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("relay_ipc"),
            "unexpected error for missing relay_ipc: {err}"
        );
    }

    #[tokio::test]
    async fn test_remote_carrier_browser_exit_stream_relays_bytes_between_runtimes() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let remote_dir = tempfile::tempdir().unwrap();
        let relay_path = remote_dir.path().join("remote-exit.sock");
        let relay_listener = tokio::net::UnixListener::bind(&relay_path).unwrap();
        let relay_task = tokio::spawn(async move {
            let (mut relay, _addr) = relay_listener.accept().await.unwrap();
            let mut request = [0_u8; 4];
            relay.read_exact(&mut request).await.unwrap();
            assert_eq!(&request, b"ping");
            relay.write_all(b"pong").await.unwrap();
        });

        let registry = Arc::new(ProviderRegistry::new());
        registry
            .register_sub_provider(
                "exit",
                Arc::new(MockCarrierExitProvider {
                    relay_path: Some(relay_path.to_string_lossy().to_string()),
                }),
            )
            .await
            .unwrap();
        let (remote_sk, remote_did) = elastos_identity::derive_did(&[55u8; 32]);
        let remote_node = start_carrier_node_with_registry(
            &remote_sk,
            &remote_did,
            remote_dir.path().to_path_buf(),
            Some(Arc::downgrade(&registry)),
        )
        .await
        .unwrap();
        let ticket = carrier_connect_ticket(&remote_node.endpoint);
        let request = BrowserCarrierStreamRequest {
            connect_ticket: ticket,
            peer_did: Some(remote_did),
            carrier_service: "elastos://exit/open_stream".to_string(),
            grant_id: "operator-grant:server-exit:alice".to_string(),
            stream_id: "remote-carrier:server-exit:test".to_string(),
            target: "tls://example.com:443".to_string(),
            principal_id: Some("person:local:alice".to_string()),
            reason: Some("browser byte bridge test".to_string()),
            timeout_ms: Some(5_000),
        };

        let mut stream = open_browser_carrier_stream(&request).await.unwrap();
        stream.send.write_all(b"ping").await.unwrap();
        stream.send.finish().unwrap();
        let mut response = [0_u8; 4];
        stream.recv.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"pong");

        relay_task.await.unwrap();
        shutdown_test_carrier_node(remote_node).await;
    }

    #[tokio::test]
    async fn test_carrier_provider_invoke_accepts_stream_contract_metadata() {
        let registry = ProviderRegistry::new();
        registry
            .register_sub_provider("content", Arc::new(MockCarrierContentProvider))
            .await
            .unwrap();
        let request = serde_json::json!({
            "source": "carrier-availability",
            "target": "content",
            "operation": "fetch",
            "transfer": "stream",
            "request": {
                "op": "fetch",
                "_runtime_invocation": {
                    "schema": "elastos.provider.invocation/v1",
                    "source": "carrier-availability",
                    "target": "content",
                    "op": "fetch",
                    "capability": "provider:carrier-availability->content:fetch",
                    "transport": "carrier-provider-plane",
                    "carrier": null,
                    "transfer": "stream",
                    "stream": {
                        "schema": "elastos.provider.stream/v1",
                        "encoding": "base64-chunks",
                        "chunk_size": 65536
                    },
                    "range": null,
                    "progress": null
                }
            }
        });

        let response = carrier_provider_invoke_registry(
            &registry,
            &request,
            &authenticated_test_source_endpoint(),
        )
        .await
        .unwrap();

        assert_eq!(response["ok"], true);
        assert_eq!(
            response["result"]["data"]["runtime_invocation"]["stream"]["schema"],
            "elastos.provider.stream/v1"
        );
        assert_eq!(
            response["result"]["data"]["runtime_invocation"]["stream"]["encoding"],
            "base64-chunks"
        );
        assert_eq!(
            response["result"]["data"]["runtime_invocation"]["carrier"]["source_endpoint_did"],
            public_key_to_did(&authenticated_test_source_endpoint())
        );
        assert!(!response.to_string().contains("\"connect_ticket\":"));
    }

    #[tokio::test]
    async fn test_carrier_provider_invoke_rejects_non_null_carrier_metadata() {
        let registry = ProviderRegistry::new();
        registry
            .register_sub_provider("content", Arc::new(MockCarrierContentProvider))
            .await
            .unwrap();
        let request = serde_json::json!({
            "source": "carrier-availability",
            "target": "content",
            "operation": "fetch",
            "transfer": "bytes",
            "request": {
                "op": "fetch",
                "_runtime_invocation": {
                    "schema": "elastos.provider.invocation/v1",
                    "source": "carrier-availability",
                    "target": "content",
                    "op": "fetch",
                    "capability": "provider:carrier-availability->content:fetch",
                    "transport": "carrier-provider-plane",
                    "carrier": {
                        "route": "peer_did",
                        "peer_did": "did:key:zInjected",
                        "timeout_ms": 5000
                    },
                    "transfer": "bytes",
                    "range": null,
                    "progress": null
                }
            }
        });

        let response = carrier_provider_invoke_registry(
            &registry,
            &request,
            &authenticated_test_source_endpoint(),
        )
        .await
        .unwrap();

        assert_eq!(response["ok"], false);
        assert_eq!(response["code"], "invalid_provider_invocation");
        assert!(response["error"]
            .as_str()
            .unwrap()
            .contains("carrier metadata must be null"));
    }

    #[tokio::test]
    async fn test_carrier_provider_invoke_rejects_stream_without_contract_metadata() {
        let registry = ProviderRegistry::new();
        registry
            .register_sub_provider("content", Arc::new(MockCarrierContentProvider))
            .await
            .unwrap();
        let request = serde_json::json!({
            "source": "carrier-availability",
            "target": "content",
            "operation": "fetch",
            "transfer": "stream",
            "request": {
                "op": "fetch",
                "_runtime_invocation": {
                    "schema": "elastos.provider.invocation/v1",
                    "source": "carrier-availability",
                    "target": "content",
                    "op": "fetch",
                    "capability": "provider:carrier-availability->content:fetch",
                    "transport": "carrier-provider-plane",
                    "carrier": null,
                    "transfer": "stream"
                }
            }
        });

        let response = carrier_provider_invoke_registry(
            &registry,
            &request,
            &authenticated_test_source_endpoint(),
        )
        .await
        .unwrap();

        assert_eq!(response["ok"], false);
        assert_eq!(response["code"], "invalid_provider_invocation");
        assert!(response["error"]
            .as_str()
            .unwrap()
            .contains("stream transfer requires stream metadata"));
    }

    #[tokio::test]
    async fn test_carrier_provider_invoke_rejects_raw_backend_target() {
        let registry = ProviderRegistry::new();
        registry
            .register_sub_provider("ipfs", Arc::new(MockCarrierIpfsProvider))
            .await
            .unwrap();
        let request = serde_json::json!({
            "source": "content-provider",
            "target": "ipfs",
            "operation": "cat",
            "transfer": "bytes",
            "request": {
                "op": "cat",
                "_runtime_invocation": {
                    "schema": "elastos.provider.invocation/v1",
                    "source": "content-provider",
                    "target": "ipfs",
                    "op": "cat",
                    "capability": "provider:content-provider->ipfs:cat",
                    "transport": "carrier-provider-plane",
                    "carrier": null,
                    "transfer": "bytes"
                }
            }
        });

        let response = carrier_provider_invoke_registry(
            &registry,
            &request,
            &authenticated_test_source_endpoint(),
        )
        .await
        .unwrap();

        assert_eq!(response["ok"], false);
        assert_eq!(response["code"], "unauthorized_provider_target");
    }

    #[tokio::test]
    async fn test_carrier_availability_fetch_uses_provider_invocation_transport() {
        let registry = ProviderRegistry::new();
        let invoker = Arc::new(MockCarrierProviderPlaneInvoker::default());
        registry.set_carrier_invoker(invoker.clone()).await;

        let (bytes, remote_transfer) = fetch_content_via_carrier_provider_invocation(
            &registry,
            "ticket:internal-secret",
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
            "docs/readme.md",
        )
        .await
        .unwrap();

        assert_eq!(bytes, b"carrier provider bytes");
        let remote_transfer = remote_transfer.expect("Carrier invocation must emit transfer");
        assert_eq!(remote_transfer["transport"], "carrier-provider-plane");
        assert_eq!(remote_transfer["source"], "carrier-availability");
        assert_eq!(remote_transfer["target"], "content");
        assert_eq!(remote_transfer["op"], "fetch");
        assert_eq!(remote_transfer["transfer"], "stream");
        assert_eq!(
            remote_transfer["stream"]["schema"],
            "elastos.provider.stream/v1"
        );
        assert!(!remote_transfer
            .to_string()
            .contains("ticket:internal-secret"));

        let requests = invoker.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["ticket"], "ticket:internal-secret");
        assert_eq!(requests[0]["target"], "content");
        assert_eq!(requests[0]["request"]["local_only"], true);
        assert_eq!(requests[0]["request"]["transfer"], "stream");
        assert_eq!(requests[0]["request"]["path"], "docs/readme.md");
        assert_eq!(
            requests[0]["request"]["_runtime_invocation"]["transport"],
            "carrier-provider-plane"
        );
        assert_eq!(
            requests[0]["request"]["_runtime_invocation"]["transfer"],
            "stream"
        );
    }

    #[tokio::test]
    async fn test_carrier_replication_proof_uses_remote_content_provider_invocation() {
        let registry = ProviderRegistry::new();
        let invoker = Arc::new(MockCarrierProviderPlaneInvoker::default());
        registry.set_carrier_invoker(invoker.clone()).await;
        let replica = CarrierAvailabilityReplica {
            node_did: "did:key:zRemote".to_string(),
            endpoint_id: Some("remote-endpoint".to_string()),
            connect_ticket: "ticket:internal-secret".to_string(),
            announced_at: 1_700_000_000,
            score: 90,
            selection_reason: "signed_announcement+endpoint_advertised+fresh".to_string(),
            reputation_score: 0,
            reputation_reason: "no_local_history".to_string(),
        };

        let proof = ensure_content_via_carrier_provider_invocation(
            &registry,
            &replica,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
            &serde_json::json!({
                "object_did": "did:key:zObject",
                "publisher_did": "did:key:zPublisher",
                "accounting": {
                    "schema": "elastos.content.accounting/v1",
                    "content_bytes": 22
                },
                "requirements": {
                    "max_storage_bytes_per_principal": 1024
                }
            }),
        )
        .await
        .unwrap();

        assert_eq!(proof.node_did, "did:key:zRemote");
        assert_eq!(proof.endpoint_id.as_deref(), Some("remote-endpoint"));
        assert_eq!(proof.ensure_status, "local_pinned");
        assert_eq!(proof.status_availability["status"], "local_pinned");
        assert_eq!(proof.announced_at, 1_700_000_000);
        assert_eq!(proof.admission.as_ref().unwrap()["accepted"], true);
        assert_eq!(
            proof.admission.as_ref().unwrap()["estimated_content_bytes"],
            22
        );
        assert_eq!(proof.remote_receipt.as_ref().unwrap()["verified"], true);
        assert_eq!(
            proof.remote_receipt.as_ref().unwrap()["status"],
            "local_pinned"
        );
        assert_eq!(
            proof.remote_receipt.as_ref().unwrap()["policy"],
            "carrier_exact_import"
        );
        assert_eq!(
            proof.remote_receipt.as_ref().unwrap()["quota"]["status"],
            "within_quota"
        );
        assert_eq!(
            proof.remote_receipt.as_ref().unwrap()["quota"]["enforced"],
            true
        );
        assert_eq!(
            proof.remote_receipt.as_ref().unwrap()["repair_worker"]["worker"],
            "content-provider"
        );
        assert_eq!(
            proof.remote_receipt.as_ref().unwrap()["accounting"]["content_bytes"],
            22
        );
        assert_eq!(
            proof.remote_receipt.as_ref().unwrap()["accounting"]["storage_quota_status"],
            "observed_not_enforced"
        );

        let requests = invoker.requests.lock().await;
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0]["target"], "content");
        assert_eq!(requests[0]["op"], "admission");
        assert_eq!(
            requests[0]["request"]["availability_requirements"]["max_storage_bytes_per_principal"],
            1024
        );
        assert_eq!(requests[0]["request"]["estimated_content_bytes"], 22);
        assert_eq!(requests[1]["op"], "ensure");
        assert_eq!(
            requests[1]["request"]["availability_policy"],
            "carrier_replica"
        );
        assert_eq!(requests[1]["request"]["object_did"], "did:key:zObject");
        assert_eq!(requests[2]["op"], "status");
        assert_eq!(
            requests[0]["request"]["_runtime_invocation"]["transport"],
            "carrier-provider-plane"
        );
    }

    #[tokio::test]
    async fn test_carrier_replication_falls_back_to_exact_import_when_remote_pin_fails() {
        let registry = ProviderRegistry::new();
        let invoker = Arc::new(MockCarrierProviderPlaneInvoker {
            requests: Mutex::new(Vec::new()),
            fail_ensure: true,
            reject_admission: false,
            omit_admission_receipt: false,
        });
        registry.set_carrier_invoker(invoker.clone()).await;
        registry
            .register_sub_provider("content", Arc::new(MockCarrierContentProvider))
            .await
            .unwrap();
        let replica = CarrierAvailabilityReplica {
            node_did: "did:key:zRemote".to_string(),
            endpoint_id: Some("remote-endpoint".to_string()),
            connect_ticket: "ticket:internal-secret".to_string(),
            announced_at: 1_700_000_000,
            score: 90,
            selection_reason: "signed_announcement+endpoint_advertised+fresh".to_string(),
            reputation_score: 0,
            reputation_reason: "no_local_history".to_string(),
        };

        let proof = ensure_content_via_carrier_provider_invocation(
            &registry,
            &replica,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
            &serde_json::json!({
                "object_did": "did:key:zObject",
                "publisher_did": "did:key:zPublisher"
            }),
        )
        .await
        .unwrap();

        assert_eq!(proof.ensure_status, "local_pinned");
        assert_eq!(proof.status_availability["status"], "local_pinned");
        assert_eq!(proof.remote_receipt.as_ref().unwrap()["verified"], true);

        let requests = invoker.requests.lock().await;
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[0]["op"], "admission");
        assert_eq!(requests[1]["op"], "ensure");
        assert_eq!(requests[2]["op"], "import_exact");
        assert_eq!(requests[2]["request"]["object_did"], "did:key:zObject");
        assert_eq!(
            requests[2]["request"]["stream"]["schema"],
            "elastos.provider.stream/v1"
        );
        assert_eq!(requests[3]["op"], "status");
        assert!(!requests[2]["request"]
            .to_string()
            .contains("ticket:internal-secret"));
    }

    #[tokio::test]
    async fn test_carrier_replication_refuses_object_exact_fallback_without_block_graph_provider() {
        let registry = ProviderRegistry::new();
        let invoker = Arc::new(MockCarrierProviderPlaneInvoker {
            requests: Mutex::new(Vec::new()),
            fail_ensure: true,
            reject_admission: false,
            omit_admission_receipt: false,
        });
        registry.set_carrier_invoker(invoker.clone()).await;
        registry
            .register_sub_provider("content", Arc::new(MockCarrierContentProvider))
            .await
            .unwrap();
        let replica = CarrierAvailabilityReplica {
            node_did: "did:key:zRemote".to_string(),
            endpoint_id: Some("remote-endpoint".to_string()),
            connect_ticket: "ticket:internal-secret".to_string(),
            announced_at: 1_700_000_000,
            score: 90,
            selection_reason: "signed_announcement+endpoint_advertised+fresh".to_string(),
            reputation_score: 0,
            reputation_reason: "no_local_history".to_string(),
        };

        let err = ensure_content_via_carrier_provider_invocation(
            &registry,
            &replica,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
            &serde_json::json!({
                "availability_requirements": {
                    "repair_graph_kind": "ipld_dag"
                }
            }),
        )
        .await
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("local block-graph export failed for arbitrary DAG repair"));
        assert!(err.to_string().contains("refused object/exact fallback"));

        let requests = invoker.requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["op"], "admission");
        assert_eq!(requests[1]["op"], "ensure");
        assert!(!requests
            .iter()
            .any(|request| request["op"] == "import_exact"));
        assert!(!requests
            .iter()
            .any(|request| request["op"] == "import_object"));
    }

    #[tokio::test]
    async fn test_carrier_replication_uses_block_graph_provider_for_arbitrary_dag() {
        let registry = ProviderRegistry::new();
        let invoker = Arc::new(MockCarrierProviderPlaneInvoker {
            requests: Mutex::new(Vec::new()),
            fail_ensure: true,
            reject_admission: false,
            omit_admission_receipt: false,
        });
        registry.set_carrier_invoker(invoker.clone()).await;
        registry
            .register_sub_provider("content", Arc::new(MockCarrierContentProvider))
            .await
            .unwrap();
        registry
            .register_sub_provider("block-graph", Arc::new(MockCarrierBlockGraphProvider))
            .await
            .unwrap();
        let replica = CarrierAvailabilityReplica {
            node_did: "did:key:zRemote".to_string(),
            endpoint_id: Some("remote-endpoint".to_string()),
            connect_ticket: "ticket:internal-secret".to_string(),
            announced_at: 1_700_000_000,
            score: 90,
            selection_reason: "signed_announcement+endpoint_advertised+fresh".to_string(),
            reputation_score: 0,
            reputation_reason: "no_local_history".to_string(),
        };

        let proof = ensure_content_via_carrier_provider_invocation(
            &registry,
            &replica,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
            &serde_json::json!({
                "object_did": "did:key:zObject",
                "publisher_did": "did:key:zPublisher",
                "availability_requirements": {
                    "repair_graph_kind": "ipld_dag"
                }
            }),
        )
        .await
        .unwrap();

        assert_eq!(proof.ensure_status, "local_pinned");
        assert_eq!(
            proof.remote_receipt.as_ref().unwrap()["verified"],
            serde_json::Value::Bool(true)
        );

        let requests = invoker.requests.lock().await;
        assert_eq!(requests.len(), 5);
        assert_eq!(requests[0]["op"], "admission");
        assert_eq!(requests[1]["op"], "ensure");
        assert_eq!(requests[2]["target"], CONTENT_BLOCK_GRAPH_TARGET);
        assert_eq!(requests[2]["op"], "import_graph");
        assert_eq!(
            requests[2]["request"]["graph"]["schema"],
            CONTENT_BLOCK_GRAPH_SCHEMA
        );
        assert_eq!(requests[2]["request"]["object_did"], "did:key:zObject");
        assert_eq!(
            requests[2]["request"]["publisher_did"],
            "did:key:zPublisher"
        );
        assert_eq!(requests[3]["target"], "content");
        assert_eq!(requests[3]["op"], "ensure");
        assert_eq!(
            requests[3]["request"]["availability_policy"],
            "carrier_block_graph_import"
        );
        assert_eq!(requests[4]["op"], "status");
        assert!(!requests
            .iter()
            .any(|request| request["op"] == "import_exact"));
        assert!(!requests
            .iter()
            .any(|request| request["op"] == "import_object"));
    }

    #[tokio::test]
    async fn test_carrier_replication_prefers_object_import_when_manifest_exists() {
        let registry = ProviderRegistry::new();
        let invoker = Arc::new(MockCarrierProviderPlaneInvoker {
            requests: Mutex::new(Vec::new()),
            fail_ensure: true,
            reject_admission: false,
            omit_admission_receipt: false,
        });
        registry.set_carrier_invoker(invoker.clone()).await;
        registry
            .register_sub_provider("content", Arc::new(MockCarrierObjectContentProvider))
            .await
            .unwrap();
        let replica = CarrierAvailabilityReplica {
            node_did: "did:key:zRemote".to_string(),
            endpoint_id: Some("remote-endpoint".to_string()),
            connect_ticket: "ticket:internal-secret".to_string(),
            announced_at: 1_700_000_000,
            score: 90,
            selection_reason: "signed_announcement+endpoint_advertised+fresh".to_string(),
            reputation_score: 0,
            reputation_reason: "no_local_history".to_string(),
        };

        let proof = ensure_content_via_carrier_provider_invocation(
            &registry,
            &replica,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
            &serde_json::json!({
                "object_did": "did:key:zIgnoredSourceObject",
                "publisher_did": "did:key:zIgnoredSourcePublisher"
            }),
        )
        .await
        .unwrap();

        assert_eq!(proof.ensure_status, "local_pinned");
        assert_eq!(proof.status_availability["status"], "local_pinned");

        let requests = invoker.requests.lock().await;
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[0]["op"], "admission");
        assert_eq!(requests[1]["op"], "ensure");
        assert_eq!(requests[2]["op"], "import_object");
        assert_eq!(requests[2]["request"]["object_kind"], "document");
        assert_eq!(requests[2]["request"]["object_did"], "did:key:zObject");
        assert_eq!(
            requests[2]["request"]["publisher_did"],
            "did:key:zPublisher"
        );
        assert_eq!(requests[2]["request"]["files"].as_array().unwrap().len(), 1);
        assert_eq!(
            requests[2]["request"]["files"][0]["path"],
            serde_json::Value::String("index.md".to_string())
        );
        assert!(requests[2]["request"].get("stream").is_none());
        assert_eq!(requests[3]["op"], "status");
        assert!(!requests[2]["request"]
            .to_string()
            .contains("ticket:internal-secret"));
    }

    #[tokio::test]
    async fn test_carrier_replication_stops_when_remote_admission_rejects() {
        let registry = ProviderRegistry::new();
        let invoker = Arc::new(MockCarrierProviderPlaneInvoker {
            requests: Mutex::new(Vec::new()),
            fail_ensure: false,
            reject_admission: true,
            omit_admission_receipt: false,
        });
        registry.set_carrier_invoker(invoker.clone()).await;
        let replica = CarrierAvailabilityReplica {
            node_did: "did:key:zRemote".to_string(),
            endpoint_id: Some("remote-endpoint".to_string()),
            connect_ticket: "ticket:internal-secret".to_string(),
            announced_at: 1_700_000_000,
            score: 90,
            selection_reason: "signed_announcement+endpoint_advertised+fresh".to_string(),
            reputation_score: 0,
            reputation_reason: "no_local_history".to_string(),
        };

        let err = ensure_content_via_carrier_provider_invocation(
            &registry,
            &replica,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
            &serde_json::json!({
                "publisher_did": "did:key:zPublisher",
                "estimated_content_bytes": 22,
                "requirements": {
                    "max_storage_bytes_per_principal": 1
                }
            }),
        )
        .await
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("remote content admission rejected"));
        assert!(err.to_string().contains("mock remote quota exceeded"));

        let requests = invoker.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["op"], "admission");
        assert_eq!(requests[0]["request"]["estimated_content_bytes"], 22);
        assert!(!requests.iter().any(|request| request["op"] == "ensure"));
        assert!(!requests
            .iter()
            .any(|request| request["op"] == "import_exact"));
        assert!(!requests
            .iter()
            .any(|request| request["op"] == "import_object"));
        assert!(!requests
            .iter()
            .any(|request| request["op"] == "import_graph"));
    }

    #[tokio::test]
    async fn test_carrier_replication_rejects_unsigned_remote_admission() {
        let registry = ProviderRegistry::new();
        let invoker = Arc::new(MockCarrierProviderPlaneInvoker {
            requests: Mutex::new(Vec::new()),
            fail_ensure: false,
            reject_admission: false,
            omit_admission_receipt: true,
        });
        registry.set_carrier_invoker(invoker.clone()).await;
        let replica = CarrierAvailabilityReplica {
            node_did: "did:key:zRemote".to_string(),
            endpoint_id: Some("remote-endpoint".to_string()),
            connect_ticket: "ticket:internal-secret".to_string(),
            announced_at: 1_700_000_000,
            score: 90,
            selection_reason: "signed_announcement+endpoint_advertised+fresh".to_string(),
            reputation_score: 0,
            reputation_reason: "no_local_history".to_string(),
        };

        let err = ensure_content_via_carrier_provider_invocation(
            &registry,
            &replica,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
            &serde_json::json!({
                "publisher_did": "did:key:zPublisher",
                "estimated_content_bytes": 22,
                "requirements": {
                    "max_storage_bytes_per_principal": 1024
                }
            }),
        )
        .await
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("remote content admission missing signed receipt"));

        let requests = invoker.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["op"], "admission");
        assert!(!requests.iter().any(|request| request["op"] == "ensure"));
        assert!(!requests
            .iter()
            .any(|request| request["op"] == "import_exact"));
        assert!(!requests
            .iter()
            .any(|request| request["op"] == "import_object"));
        assert!(!requests
            .iter()
            .any(|request| request["op"] == "import_graph"));
    }

    #[test]
    fn test_carrier_peer_selection_proof_redacts_connect_tickets() {
        let proof = CarrierReplicationProof {
            node_did: "did:key:zRemote".to_string(),
            endpoint_id: Some("remote-endpoint".to_string()),
            announced_at: 1_700_000_000,
            score: 90,
            selection_reason: "signed_announcement+endpoint_advertised+fresh".to_string(),
            reputation_score: 4,
            reputation_reason: "local_runtime_successes:1;failures:0".to_string(),
            ensure_status: "local_pinned".to_string(),
            admission: Some(serde_json::json!({
                "schema": "elastos.content.admission/v1",
                "accepted": true,
                "status": "accepted",
                "quota": {
                    "status": "within_quota",
                    "enforced": true
                }
            })),
            status_availability: serde_json::json!({"status": "local_pinned"}),
            remote_receipt: Some(serde_json::json!({
                "schema": "elastos.content.availability.receipt/v1",
                "cid": "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
                "status": "local_pinned",
                "replicas": 1,
                "quota": {
                    "status": "within_quota",
                    "enforced": true
                },
                "accounting": {
                    "observed": true,
                    "content_bytes": 22,
                    "storage_quota_status": "observed_not_enforced"
                },
                "signer_did": "did:key:zRemoteContentProvider",
                "verified": true
            })),
            transfer: Some(serde_json::json!({
                "transport": "carrier-provider-plane",
                "carrier": {
                    "route": "connect_ticket",
                    "peer_did": "did:key:zRemote"
                }
            })),
            checked_at: 1_700_000_001,
        };

        let peer_selection = carrier_peer_selection_json(
            "elastos://carrier/content/test/availability",
            "did:key:zLocal",
            1,
            &[proof],
            true,
            CarrierPeerAttestationExchangeView {
                configured: false,
                receipt: None,
            },
        );

        assert_eq!(peer_selection["mode"], "carrier_provider_replication");
        assert_eq!(peer_selection["live_multi_peer_proof"], true);
        assert_eq!(
            peer_selection["peer_reputation_policy"]["schema"],
            CARRIER_PEER_REPUTATION_SCHEMA
        );
        assert_eq!(
            peer_selection["peer_reputation_policy"]["status"],
            "local_history_applied"
        );
        assert_eq!(
            peer_selection["peer_reputation_policy"]["federation"]["configured"],
            false
        );
        assert_eq!(
            peer_selection["peer_attestation_exchange_policy"]["schema"],
            CARRIER_PEER_ATTESTATION_EXCHANGE_POLICY_SCHEMA
        );
        assert_eq!(
            peer_selection["peer_attestation_exchange_policy"]["status"],
            "live_peer_proof_without_attestation_exchange"
        );
        assert_eq!(
            peer_selection["peer_attestation_exchange_policy"]["local_proof"]
                ["verified_remote_content_receipts"],
            1
        );
        assert_eq!(
            peer_selection["peer_attestation_exchange_policy"]["attestation_exchange"]
                ["configured"],
            false
        );
        assert_eq!(peer_selection["replicas"].as_array().unwrap().len(), 2);
        assert_eq!(peer_selection["replicas"][1]["score"], 90);
        assert_eq!(
            peer_selection["replicas"][1]["selection_reason"],
            "signed_announcement+endpoint_advertised+fresh"
        );
        assert_eq!(
            peer_selection["replicas"][1]["local_reputation"]["scope"],
            "local_runtime"
        );
        assert_eq!(
            peer_selection["replicas"][1]["local_reputation"]["score_delta"],
            4
        );
        assert_eq!(
            peer_selection["replicas"][1]["local_reputation"]["reason"],
            "local_runtime_successes:1;failures:0"
        );
        assert_eq!(
            peer_selection["replicas"][1]["remote_receipt"]["quota"]["status"],
            "within_quota"
        );
        assert_eq!(
            peer_selection["replicas"][1]["remote_receipt"]["accounting"]["content_bytes"],
            22
        );
        assert_eq!(
            peer_selection["replicas"][1]["remote_receipt"]["accounting"]["storage_quota_status"],
            "observed_not_enforced"
        );
        assert_eq!(peer_selection["replicas"][1]["admission"]["accepted"], true);
        assert_eq!(
            peer_selection["replicas"][1]["admission"]["quota"]["status"],
            "within_quota"
        );
        assert!(!peer_selection
            .to_string()
            .contains("ticket:internal-secret"));
        assert!(!peer_selection
            .to_string()
            .contains("connect_ticket\": \"ticket"));
    }

    #[tokio::test]
    async fn test_carrier_peer_attestation_exchange_posts_signed_request_and_verifies_receipt() {
        let signed_receipt = signed_peer_attestation_exchange_receipt(serde_json::json!({
            "schema": CARRIER_PEER_ATTESTATION_EXCHANGE_RECEIPT_SCHEMA,
            "exchange_id": "peer-attestation:test",
            "receipt_id": "peer-attestation-receipt:123",
            "accepted": true,
        }));
        let (url, handle) = spawn_peer_attestation_exchange_endpoint(serde_json::json!({
            "accepted": true,
            "status": "accepted",
            "exchange_id": "peer-attestation:test",
            "receipt_id": "peer-attestation-receipt:123",
            "receipt": signed_receipt,
        }));
        let client = CarrierPeerAttestationExchangeClient::from_config(serde_json::json!({
            "url": url,
            "authorization": "Bearer peer-attestation-test",
            "timeout_secs": 5,
        }))
        .unwrap();
        let (signing_key, _) = elastos_identity::derive_did(&[47u8; 32]);
        let proof = carrier_peer_attestation_test_proof();
        let request = carrier_peer_attestation_exchange_request(
            &signing_key,
            "bafyattest",
            "elastos://carrier/content/test/availability",
            "did:key:zLocal",
            std::slice::from_ref(&proof),
            true,
            1_700_000_002,
        )
        .unwrap();

        let receipt = client.exchange(&request).await.unwrap();

        assert_eq!(
            receipt["schema"],
            CARRIER_PEER_ATTESTATION_EXCHANGE_RECEIPT_SCHEMA
        );
        assert_eq!(receipt["status"], "accepted");
        assert_eq!(receipt["accepted"], true);
        assert_eq!(receipt["signed_receipt"]["verified"], true);
        assert_eq!(
            receipt["signed_receipt"]["payload_schema"],
            CARRIER_PEER_ATTESTATION_EXCHANGE_RECEIPT_SCHEMA
        );
        assert_eq!(receipt["exchange"]["credential_exposed"], false);
        let peer_selection = carrier_peer_selection_json(
            "elastos://carrier/content/test/availability",
            "did:key:zLocal",
            1,
            &[proof],
            true,
            CarrierPeerAttestationExchangeView {
                configured: true,
                receipt: Some(&receipt),
            },
        );
        assert_eq!(
            peer_selection["peer_attestation_exchange_policy"]["status"],
            "attestation_exchange_accepted"
        );
        assert_eq!(
            peer_selection["peer_attestation_exchange_policy"]["attestation_exchange"]
                ["configured"],
            true
        );
        assert_eq!(
            peer_selection["peer_attestation_exchange_policy"]["attestation_exchange"]
                ["signed_reputation_receipts"],
            true
        );

        let request_text = handle.join().unwrap();
        assert!(request_text.starts_with("POST /peer-attestation/exchange HTTP/1.1"));
        assert!(request_text
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer peer-attestation-test")));
        assert!(request_text.contains(CARRIER_PEER_ATTESTATION_EXCHANGE_REQUEST_SCHEMA));
        assert!(request_text.contains("\"signature\""));
        assert!(request_text.contains("\"signer_did\""));
        assert!(!request_text
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("")
            .contains("peer-attestation-test"));
        assert!(!request_text.contains("ticket:internal-secret"));
    }

    #[tokio::test]
    async fn test_carrier_peer_attestation_exchange_accepts_endpoint_quorum() {
        let signed_receipt_a = signed_peer_attestation_exchange_receipt(serde_json::json!({
            "schema": CARRIER_PEER_ATTESTATION_EXCHANGE_RECEIPT_SCHEMA,
            "exchange_id": "peer-attestation:a",
            "receipt_id": "peer-attestation-receipt:a",
            "accepted": true,
        }));
        let (url_a, handle_a) = spawn_peer_attestation_exchange_endpoint(serde_json::json!({
            "accepted": true,
            "status": "accepted",
            "exchange_id": "peer-attestation:a",
            "receipt_id": "peer-attestation-receipt:a",
            "receipt": signed_receipt_a,
        }));
        let signed_receipt_b = signed_peer_attestation_exchange_receipt(serde_json::json!({
            "schema": CARRIER_PEER_ATTESTATION_EXCHANGE_RECEIPT_SCHEMA,
            "exchange_id": "peer-attestation:b",
            "receipt_id": "peer-attestation-receipt:b",
            "accepted": true,
        }));
        let (url_b, handle_b) = spawn_peer_attestation_exchange_endpoint(serde_json::json!({
            "accepted": true,
            "status": "accepted",
            "exchange_id": "peer-attestation:b",
            "receipt_id": "peer-attestation-receipt:b",
            "receipt": signed_receipt_b,
        }));
        let client = CarrierPeerAttestationExchangeClient::from_config(serde_json::json!({
            "quorum": 2,
            "endpoints": [
                {
                    "id": "peer-attestation-a",
                    "url": url_a,
                    "authorization": "Bearer peer-attestation-secret-a",
                    "timeout_secs": 5
                },
                {
                    "id": "peer-attestation-b",
                    "url": url_b,
                    "authorization": "Bearer peer-attestation-secret-b",
                    "timeout_secs": 5
                }
            ]
        }))
        .unwrap();
        let (signing_key, _) = elastos_identity::derive_did(&[47u8; 32]);
        let proof = carrier_peer_attestation_test_proof();
        let request = carrier_peer_attestation_exchange_request(
            &signing_key,
            "bafyattest",
            "elastos://carrier/content/test/availability",
            "did:key:zLocal",
            std::slice::from_ref(&proof),
            true,
            1_700_000_002,
        )
        .unwrap();

        let receipt = client.exchange(&request).await.unwrap();

        assert_eq!(receipt["status"], "accepted");
        assert_eq!(receipt["accepted"], true);
        assert_eq!(receipt["quorum"]["required"], 2);
        assert_eq!(receipt["quorum"]["endpoint_count"], 2);
        assert_eq!(receipt["quorum"]["accepted"], 2);
        assert_eq!(receipt["signed_receipt"]["verified"], true);
        assert_eq!(receipt["exchange"]["multi_endpoint"], true);
        assert_eq!(receipt["exchange"]["endpoint_count"], 2);
        assert!(!receipt.to_string().contains("peer-attestation-secret-a"));
        assert!(!receipt.to_string().contains("peer-attestation-secret-b"));

        let request_a = handle_a.join().unwrap();
        let request_b = handle_b.join().unwrap();
        assert!(request_a.lines().any(
            |line| line.eq_ignore_ascii_case("authorization: Bearer peer-attestation-secret-a")
        ));
        assert!(request_b.lines().any(
            |line| line.eq_ignore_ascii_case("authorization: Bearer peer-attestation-secret-b")
        ));
        assert!(!request_a
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("")
            .contains("peer-attestation-secret-a"));
        assert!(!request_b
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("")
            .contains("peer-attestation-secret-b"));
        assert!(!request_a.contains("ticket:internal-secret"));
        assert!(!request_b.contains("ticket:internal-secret"));
    }

    #[tokio::test]
    async fn test_carrier_peer_attestation_exchange_rejects_endpoint_quorum_failure() {
        let signed_receipt = signed_peer_attestation_exchange_receipt(serde_json::json!({
            "schema": CARRIER_PEER_ATTESTATION_EXCHANGE_RECEIPT_SCHEMA,
            "exchange_id": "peer-attestation:a",
            "receipt_id": "peer-attestation-receipt:a",
            "accepted": true,
        }));
        let (accepted_url, accepted_handle) =
            spawn_peer_attestation_exchange_endpoint(serde_json::json!({
                "accepted": true,
                "status": "accepted",
                "receipt": signed_receipt,
            }));
        let (rejected_url, rejected_handle) =
            spawn_peer_attestation_exchange_endpoint(serde_json::json!({
                "accepted": false,
                "status": "rejected",
                "reason": "reputation trust policy rejected peer",
            }));
        let client = CarrierPeerAttestationExchangeClient::from_config(serde_json::json!({
            "quorum": 2,
            "endpoints": [
                {"id": "peer-attestation-a", "url": accepted_url, "timeout_secs": 5},
                {"id": "peer-attestation-b", "url": rejected_url, "timeout_secs": 5}
            ]
        }))
        .unwrap();
        let (signing_key, _) = elastos_identity::derive_did(&[47u8; 32]);
        let proof = carrier_peer_attestation_test_proof();
        let request = carrier_peer_attestation_exchange_request(
            &signing_key,
            "bafyattest",
            "elastos://carrier/content/test/availability",
            "did:key:zLocal",
            std::slice::from_ref(&proof),
            true,
            1_700_000_002,
        )
        .unwrap();

        let receipt = client.exchange(&request).await.unwrap();

        assert_eq!(receipt["status"], "rejected");
        assert_eq!(receipt["accepted"], false);
        assert_eq!(receipt["quorum"]["required"], 2);
        assert_eq!(receipt["quorum"]["accepted"], 1);
        assert_eq!(receipt["quorum"]["rejected"], 1);
        assert_eq!(receipt["signed_receipt"]["verified"], true);
        assert!(receipt["reason"]
            .as_str()
            .unwrap()
            .contains("reputation trust policy rejected peer"));

        let accepted_request = accepted_handle.join().unwrap();
        let rejected_request = rejected_handle.join().unwrap();
        assert!(accepted_request.contains(CARRIER_PEER_ATTESTATION_EXCHANGE_REQUEST_SCHEMA));
        assert!(rejected_request.contains(CARRIER_PEER_ATTESTATION_EXCHANGE_REQUEST_SCHEMA));
    }

    #[test]
    fn test_remote_content_receipt_peer_selection_summary_redacts_replica_rows() {
        let summary = remote_content_receipt_peer_selection_summary(Some(&serde_json::json!({
            "mode": "carrier_provider_replication",
            "strategy": "signed_announcement_then_provider_invoke",
            "live_multi_peer_proof": true,
            "peer_reputation_policy": {
                "schema": CARRIER_PEER_REPUTATION_SCHEMA,
                "policy": "local_runtime_reputation",
                "status": "local_history_applied",
                "federation": {
                    "configured": false,
                    "cross_runtime_reputation": false
                }
            },
            "peer_attestation_exchange_policy": {
                "schema": CARRIER_PEER_ATTESTATION_EXCHANGE_POLICY_SCHEMA,
                "policy": "no_cross_runtime_attestation_exchange",
                "status": "live_peer_proof_without_attestation_exchange",
                "attestation_exchange": {
                    "configured": false,
                    "signed_reputation_receipts": false
                }
            },
            "replicas": [
                {
                    "role": "local",
                    "node_did": "did:key:zLocal",
                    "status": "local_pinned"
                },
                {
                    "role": "remote",
                    "node_did": "did:key:zRemote",
                    "endpoint_id": "remote-endpoint",
                    "score": 94,
                    "selection_reason": "signed_announcement+endpoint_advertised+fresh+local_reputation_positive",
                    "local_reputation": {
                        "scope": "local_runtime",
                        "score_delta": 4,
                        "reason": "local_runtime_successes:1;failures:0"
                    },
                    "status": "local_pinned",
                    "transfer": {
                        "transport": "carrier-provider-plane",
                        "carrier": {
                            "route": "connect_ticket",
                            "connect_ticket": "ticket:internal-secret"
                        }
                    },
                    "remote_receipt": {
                        "signer_did": "did:key:zRemoteContentProvider"
                    }
                }
            ]
        })));

        assert_eq!(summary["mode"], "carrier_provider_replication");
        assert_eq!(
            summary["peer_reputation_policy"]["status"],
            "local_history_applied"
        );
        assert_eq!(
            summary["peer_reputation_policy"]["federation"]["configured"],
            false
        );
        assert_eq!(
            summary["peer_attestation_exchange_policy"]["schema"],
            CARRIER_PEER_ATTESTATION_EXCHANGE_POLICY_SCHEMA
        );
        assert_eq!(
            summary["peer_attestation_exchange_policy"]["attestation_exchange"]["configured"],
            false
        );
        assert_eq!(summary["replica_count"], 2);
        assert_eq!(summary["remote_replicas"], 1);
        assert_eq!(summary["replica_summary_limit"], 5);
        assert_eq!(summary["replicas_truncated"], false);
        assert_eq!(summary["replicas"].as_array().unwrap().len(), 2);
        assert_eq!(summary["replicas"][1]["node_did"], "did:key:zRemote");
        assert_eq!(summary["replicas"][1]["score"], 94);
        assert_eq!(
            summary["replicas"][1]["local_reputation"]["scope"],
            "local_runtime"
        );
        assert!(!summary.to_string().contains("ticket:internal-secret"));
        assert!(!summary.to_string().contains("connect_ticket"));
        assert!(!summary.to_string().contains("remote_receipt"));
    }

    #[test]
    fn test_remote_content_receipt_peer_selection_summary_marks_truncated_rows() {
        let replicas = (0..6)
            .map(|index| {
                serde_json::json!({
                    "role": "remote",
                    "node_did": format!("did:key:zRemote{index}"),
                    "score": 80 + index,
                    "transfer": {
                        "carrier": {
                            "route": "connect_ticket",
                            "connect_ticket": format!("ticket:secret-{index}")
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        let summary = remote_content_receipt_peer_selection_summary(Some(&serde_json::json!({
            "mode": "carrier_provider_replication",
            "live_multi_peer_proof": true,
            "replicas": replicas
        })));

        assert_eq!(summary["replica_count"], 6);
        assert_eq!(summary["remote_replicas"], 6);
        assert_eq!(summary["replica_summary_limit"], 5);
        assert_eq!(summary["replicas_truncated"], true);
        assert_eq!(summary["replicas"].as_array().unwrap().len(), 5);
        assert!(!summary.to_string().contains("ticket:secret"));
        assert!(!summary.to_string().contains("connect_ticket"));
    }

    #[test]
    fn test_carrier_quota_marks_impossible_replica_requirements() {
        let requirements = CarrierAvailabilityRequirements {
            min_replicas: 4,
            max_replicas: Some(2),
            require_live_multi_peer_proof: true,
            repair_graph_kind: CarrierRepairGraphKind::Auto,
        };

        let quota = carrier_quota_json(requirements, 2, 2);

        assert_eq!(quota["policy"], "carrier_provider_quota");
        assert_eq!(quota["enforced"], true);
        assert_eq!(quota["status"], "requirements_exceed_quota");
        assert_eq!(quota["effective_max_replicas"], 2);
        assert_eq!(quota["requirements_exceed_quota"], true);
    }

    #[test]
    fn test_carrier_remote_candidate_limit_keeps_live_multi_peer_requirement() {
        let requirements = CarrierAvailabilityRequirements {
            min_replicas: 2,
            max_replicas: None,
            require_live_multi_peer_proof: true,
            repair_graph_kind: CarrierRepairGraphKind::Auto,
        };

        assert_eq!(carrier_remote_candidate_limit(requirements, 2, 2), 1);

        let quota_blocked = CarrierAvailabilityRequirements {
            min_replicas: 2,
            max_replicas: Some(2),
            require_live_multi_peer_proof: true,
            repair_graph_kind: CarrierRepairGraphKind::Auto,
        };
        assert_eq!(carrier_remote_candidate_limit(quota_blocked, 2, 2), 0);
    }

    #[tokio::test]
    async fn test_carrier_availability_ensure_proves_remote_replica_via_provider_plane() {
        let cid = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";
        let topic_name = content_availability_topic_name(cid);
        let (remote_message, remote_did) = signed_content_availability_message(
            cid,
            [22u8; 32],
            "ticket:remote-secret",
            "remote-endpoint",
            1_700_000_000,
        );
        let (local_sk, local_did) = elastos_identity::derive_did(&[21u8; 32]);
        let endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(iroh::SecretKey::from_bytes(&local_sk.to_bytes()))
            .bind()
            .await
            .unwrap();
        let memory_lookup = MemoryLookup::new();
        endpoint
            .address_lookup()
            .unwrap()
            .add(memory_lookup.clone());
        let gossip = spawn_carrier_gossip(&endpoint);
        let state = Arc::new(Mutex::new(GossipState::new(
            endpoint.clone(),
            gossip,
            memory_lookup,
            Some(local_sk),
            Some(local_did),
        )));
        {
            let mut guard = state.lock().await;
            guard.joined_topics.insert(topic_name.clone());
            guard
                .buffers
                .lock()
                .await
                .insert(topic_name, test_topic_buffer([remote_message], 0));
        }

        let registry = Arc::new(ProviderRegistry::new());
        let invoker = Arc::new(MockCarrierProviderPlaneInvoker::default());
        registry.set_carrier_invoker(invoker.clone()).await;
        let provider =
            CarrierAvailabilityProvider::with_provider_registry(state, Arc::downgrade(&registry));
        let response = provider
            .send_raw(&serde_json::json!({
                "op": "ensure",
                "cid": cid,
                "uri": format!("elastos://{cid}"),
                "policy": "network_default",
                "local": {
                    "status": "local_pinned",
                    "provider": "ipfs-provider",
                    "replicas": 1
                },
                "requirements": {
                    "min_replicas": 2,
                    "max_replicas": 2,
                    "require_live_multi_peer_proof": true
                },
                "object_did": "did:key:zObject",
                "publisher_did": "did:key:zPublisher"
            }))
            .await
            .unwrap();

        let availability = &response["data"]["availability"];
        assert_eq!(response["status"], "ok");
        assert_eq!(availability["status"], "network_available");
        assert_eq!(availability["replicas"], 2);
        assert_eq!(
            availability["peer_selection"]["mode"],
            "carrier_provider_replication"
        );
        assert_eq!(
            availability["peer_selection"]["live_multi_peer_proof"],
            true
        );
        assert_eq!(availability["quota"]["policy"], "carrier_provider_quota");
        assert_eq!(availability["quota"]["enforced"], true);
        assert_eq!(availability["quota"]["status"], "at_quota");
        assert_eq!(availability["quota"]["effective_max_replicas"], 2);
        assert_eq!(availability["quota"]["requirements_exceed_quota"], false);
        assert_eq!(availability["quota"]["used_replicas"], 2);
        assert_eq!(
            availability["abuse_controls"]["policy"],
            "carrier_provider_invocation_guardrail"
        );
        assert_eq!(availability["abuse_controls"]["enforced"], true);
        assert_eq!(availability["abuse_controls"]["candidate_count"], 1);
        assert_eq!(availability["abuse_controls"]["attempt_limit"], 1);
        assert_eq!(availability["abuse_controls"]["attempted_operations"], 1);
        assert_eq!(availability["abuse_controls"]["failed_operations"], 0);
        assert_eq!(availability["abuse_controls"]["throttled"], false);
        assert_eq!(
            availability["peer_selection"]["replicas"][1]["remote_receipt"]["abuse_controls"]
                ["policy"],
            "carrier_provider_invocation_guardrail"
        );
        assert_eq!(
            availability["peer_selection"]["replicas"][1]["remote_receipt"]["abuse_controls"]
                ["attempted_operations"],
            1
        );
        assert_eq!(availability["repair_worker"]["status"], "healthy");
        assert_eq!(
            availability["repair_graph"]["schema"],
            CONTENT_REPAIR_GRAPH_SCHEMA
        );
        assert_eq!(
            availability["repair_graph"]["status"],
            "bounded_import_supported"
        );
        assert_eq!(
            availability["repair_graph"]["refuses_exact_fallback_for_arbitrary_dag"],
            true
        );
        assert_eq!(
            availability["peer_selection"]["replicas"][1]["remote_receipt"]["repair_graph"]
                ["status"],
            "bounded_import_supported"
        );
        assert_eq!(
            availability["storage_market"]["mode"],
            "carrier_provider_receipts"
        );
        assert_eq!(
            availability["quota"]["federated_quota_ledger_policy"]["schema"],
            CONTENT_FEDERATED_QUOTA_LEDGER_POLICY_SCHEMA
        );
        assert_eq!(
            availability["quota"]["federated_quota_ledger_policy"]["remote"]["admission_preflight"],
            true
        );
        assert_eq!(
            availability["quota"]["federated_quota_ledger_policy"]["remote"]
                ["signed_admission_receipts"],
            true
        );
        assert_eq!(
            availability["quota"]["federated_quota_ledger_policy"]["federation"]["configured"],
            false
        );
        assert_eq!(
            availability["quota"]["federated_quota_ledger_policy"]["federation"]
                ["signed_admission_receipt_exchange"],
            true
        );
        assert_eq!(
            availability["peer_selection"]["replicas"][1]["admission"]["receipt"]["verified"],
            true
        );
        assert_eq!(
            availability["storage_market"]["settlement"],
            "not_configured"
        );
        assert_eq!(availability["storage_market"]["escrow"], "not_configured");
        assert_eq!(
            availability["storage_market"]["status"],
            "receipt_proven_no_market_settlement"
        );
        assert_eq!(
            availability["storage_market"]["settlement_policy"]["schema"],
            "elastos.content.storage-settlement-policy/v1"
        );
        assert_eq!(
            availability["storage_market"]["settlement_policy"]["production_federation"]
                ["configured"],
            false
        );
        assert_eq!(
            availability["storage_market"]["admission_policy"]["schema"],
            CONTENT_STORAGE_MARKET_ADMISSION_POLICY_SCHEMA
        );
        assert_eq!(
            availability["storage_market"]["admission_policy"]["status"],
            "remote_admission_preflight_no_market_admission"
        );
        assert_eq!(
            availability["storage_market"]["admission_policy"]["current_admission"]
                ["remote_content_admission_preflight"],
            true
        );
        assert_eq!(
            availability["storage_market"]["admission_policy"]["current_admission"]
                ["signed_admission_receipts"],
            true
        );
        assert_eq!(
            availability["storage_market"]["admission_policy"]["production_market"]["configured"],
            false
        );
        assert!(availability
            .to_string()
            .contains(&format!("\"node_did\":\"{remote_did}\"")));
        assert!(!availability.to_string().contains("ticket:remote-secret"));

        let requests = invoker.requests.lock().await;
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0]["ticket"], "ticket:remote-secret");
        assert_eq!(requests[0]["op"], "admission");
        assert_eq!(requests[0]["request"]["object_did"], "did:key:zObject");
        assert_eq!(requests[1]["op"], "ensure");
        assert_eq!(requests[1]["request"]["object_did"], "did:key:zObject");
        assert_eq!(requests[2]["op"], "status");
        endpoint.close().await;
    }

    #[tokio::test]
    async fn test_carrier_availability_requires_remote_attempt_for_live_proof_when_min_met() {
        let cid = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";
        let topic_name = content_availability_topic_name(cid);
        let (remote_message, _remote_did) = signed_content_availability_message(
            cid,
            [23u8; 32],
            "ticket:remote-secret",
            "remote-endpoint",
            1_700_000_000,
        );
        let (local_sk, local_did) = elastos_identity::derive_did(&[24u8; 32]);
        let endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(iroh::SecretKey::from_bytes(&local_sk.to_bytes()))
            .bind()
            .await
            .unwrap();
        let memory_lookup = MemoryLookup::new();
        endpoint
            .address_lookup()
            .unwrap()
            .add(memory_lookup.clone());
        let gossip = spawn_carrier_gossip(&endpoint);
        let state = Arc::new(Mutex::new(GossipState::new(
            endpoint.clone(),
            gossip,
            memory_lookup,
            Some(local_sk),
            Some(local_did),
        )));
        {
            let mut guard = state.lock().await;
            guard.joined_topics.insert(topic_name.clone());
            guard
                .buffers
                .lock()
                .await
                .insert(topic_name, test_topic_buffer([remote_message], 0));
        }

        let registry = Arc::new(ProviderRegistry::new());
        let invoker = Arc::new(MockCarrierProviderPlaneInvoker::default());
        registry.set_carrier_invoker(invoker.clone()).await;
        let provider =
            CarrierAvailabilityProvider::with_provider_registry(state, Arc::downgrade(&registry));
        let response = provider
            .send_raw(&serde_json::json!({
                "op": "ensure",
                "cid": cid,
                "uri": format!("elastos://{cid}"),
                "policy": "network_default",
                "local": {
                    "status": "local_pinned",
                    "provider": "ipfs-provider",
                    "replicas": 2
                },
                "requirements": {
                    "min_replicas": 2,
                    "require_live_multi_peer_proof": true
                }
            }))
            .await
            .unwrap();

        let availability = &response["data"]["availability"];
        assert_eq!(availability["status"], "network_available");
        assert_eq!(availability["replicas"], 3);
        assert_eq!(
            availability["peer_selection"]["live_multi_peer_proof"],
            true
        );
        assert_eq!(availability["abuse_controls"]["attempt_limit"], 1);
        assert_eq!(availability["abuse_controls"]["attempted_operations"], 1);

        let requests = invoker.requests.lock().await;
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0]["op"], "admission");
        assert_eq!(requests[1]["op"], "ensure");
        assert_eq!(requests[2]["op"], "status");
        endpoint.close().await;
    }

    #[tokio::test]
    async fn test_carrier_content_fetch_reads_from_internal_ipfs_provider() {
        let registry = ProviderRegistry::new();
        registry
            .register_sub_provider("ipfs", Arc::new(MockCarrierIpfsProvider))
            .await
            .unwrap();

        let bytes = carrier_content_fetch_bytes(
            &registry,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
            "docs/readme.md",
        )
        .await
        .unwrap();

        assert_eq!(bytes, b"carrier content");
        let err = carrier_content_fetch_bytes(
            &registry,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
            "../secret",
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("invalid segment"));
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

    #[tokio::test]
    async fn test_gossip_join_exact_rejects_ambiguous_peer_sets_without_state_changes() {
        let dir = tempfile::tempdir().unwrap();
        let (signing_key, did) = elastos_identity::derive_did(&[13u8; 32]);
        let node = start_carrier_node(&signing_key, &did, dir.path().to_path_buf())
            .await
            .unwrap();
        let provider = CarrierGossipProvider::new(node.gossip_state.clone());
        let configured_peer = iroh::SecretKey::from_bytes(&[14u8; 32]).public();
        node.gossip_state
            .lock()
            .await
            .bootstrap_peers
            .push(configured_peer);

        let canonical_peer = iroh::SecretKey::from_bytes(&[15u8; 32])
            .public()
            .to_string();
        let noncanonical_peer = canonical_peer.to_ascii_uppercase();
        assert_ne!(noncanonical_peer, canonical_peer);
        let overflow_peers = (0..=GOSSIP_JOIN_EXACT_MAX_PEERS)
            .map(|index| {
                iroh::SecretKey::from_bytes(&[32 + index as u8; 32])
                    .public()
                    .to_string()
            })
            .collect::<Vec<_>>();
        let requests = [
            serde_json::json!({
                "op": "gossip_join_exact",
                "topic": "__test/exact-rejected-missing"
            }),
            serde_json::json!({
                "op": "gossip_join_exact",
                "topic": "__test/exact-rejected-not-array",
                "peers": "not-an-array"
            }),
            serde_json::json!({
                "op": "gossip_join_exact",
                "topic": "__test/exact-rejected-non-string",
                "peers": [7]
            }),
            serde_json::json!({
                "op": "gossip_join_exact",
                "topic": "__test/exact-rejected-invalid",
                "peers": ["not-an-endpoint-id"]
            }),
            serde_json::json!({
                "op": "gossip_join_exact",
                "topic": "__test/exact-rejected-noncanonical",
                "peers": [noncanonical_peer]
            }),
            serde_json::json!({
                "op": "gossip_join_exact",
                "topic": "__test/exact-rejected-duplicate",
                "peers": [canonical_peer.clone(), canonical_peer]
            }),
            serde_json::json!({
                "op": "gossip_join_exact",
                "topic": "__test/exact-rejected-overflow",
                "peers": overflow_peers
            }),
            serde_json::json!({
                "op": "gossip_join_exact",
                "topic": "__test/exact-rejected-unknown-field",
                "peers": [],
                "mode": "direct"
            }),
        ];

        for request in requests {
            let response = provider.send_raw(&request).await.unwrap();
            assert_eq!(response["status"], "error", "request: {request}");
        }

        let state = node.gossip_state.lock().await;
        assert_eq!(state.bootstrap_peers, vec![configured_peer]);
        assert!(state.joined_topics.is_empty());
        assert!(state.last_direct_join_peers.is_none());
        drop(state);
        shutdown_test_carrier_node(node).await;
    }

    #[tokio::test]
    async fn test_gossip_join_exact_uses_only_explicit_peers_while_direct_join_uses_global_peers() {
        let dir = tempfile::tempdir().unwrap();
        let (signing_key, did) = elastos_identity::derive_did(&[16u8; 32]);
        let node = start_carrier_node(&signing_key, &did, dir.path().to_path_buf())
            .await
            .unwrap();
        let provider = CarrierGossipProvider::new(node.gossip_state.clone());
        let configured_peer = iroh::SecretKey::from_bytes(&[17u8; 32]).public();
        let exact_peer = iroh::SecretKey::from_bytes(&[18u8; 32]).public();
        node.gossip_state
            .lock()
            .await
            .bootstrap_peers
            .push(configured_peer);

        let exact_response = provider
            .send_raw(&serde_json::json!({
                "op": "gossip_join_exact",
                "topic": "__test/exact-local-only",
                "peers": []
            }))
            .await
            .unwrap();
        assert_eq!(exact_response["status"], "ok");
        {
            let state = node.gossip_state.lock().await;
            assert_eq!(state.bootstrap_peers, vec![configured_peer]);
            assert_eq!(state.last_direct_join_peers, Some(Vec::new()));
        }

        let populated_exact_response = provider
            .send_raw(&serde_json::json!({
                "op": "gossip_join_exact",
                "topic": "__test/exact-populated",
                "peers": [exact_peer.to_string()]
            }))
            .await
            .unwrap();
        assert_eq!(populated_exact_response["status"], "ok");
        {
            let state = node.gossip_state.lock().await;
            assert_eq!(state.bootstrap_peers, vec![configured_peer]);
            assert_eq!(state.last_direct_join_peers, Some(vec![exact_peer]));
        }

        let direct_response = provider
            .send_raw(&serde_json::json!({
                "op": "gossip_join",
                "topic": "__test/existing-direct",
                "mode": "direct"
            }))
            .await
            .unwrap();
        assert_eq!(direct_response["status"], "ok");
        {
            let state = node.gossip_state.lock().await;
            assert_eq!(state.bootstrap_peers, vec![configured_peer]);
            assert_eq!(state.last_direct_join_peers, Some(vec![configured_peer]));
        }

        shutdown_test_carrier_node(node).await;
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
    fn test_carrier_mdns_env_can_disable_lan_discovery() {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("ELASTOS_CARRIER_MDNS");
        assert!(carrier_mdns_enabled());
        std::env::set_var("ELASTOS_CARRIER_MDNS", "0");
        assert!(!carrier_mdns_enabled());
        std::env::set_var("ELASTOS_CARRIER_MDNS", "false");
        assert!(!carrier_mdns_enabled());
        std::env::set_var("ELASTOS_CARRIER_MDNS", "1");
        assert!(carrier_mdns_enabled());
        std::env::remove_var("ELASTOS_CARRIER_MDNS");
    }

    #[tokio::test]
    async fn test_trusted_source_runtime_joins_discovery_topic() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(elastos_common::localhost::publisher_root_path(dir.path()))
            .unwrap();
        std::fs::write(publisher_install_script_path(dir.path()), b"#!/bin/sh\n").unwrap();

        let key = [7u8; 32];
        let (signing_key, did) = elastos_identity::derive_did(&key);
        let node = start_carrier_node(&signing_key, &did, dir.path().to_path_buf())
            .await
            .unwrap();
        let state = node.gossip_state.lock().await;

        assert!(state.joined_topics.contains(CHAT_DISCOVERY_TOPIC_GENERAL));
        drop(state);
        node.endpoint.close().await;
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

        let opaque_envelope =
            vec![0x5a; elastos_common::collaboration_protocol::MAX_COLLABORATION_ENVELOPE_BYTES];
        let near_max = GossipMessage {
            sender_id: did2.clone(),
            sender_nick: "collaboration-transport".to_string(),
            content: base64::engine::general_purpose::STANDARD.encode(opaque_envelope),
            ts: 1_700_000_001,
            nonce: 100,
            signature: None,
            sender_session_id: None,
        };
        let near_max_bytes = serialize_gossip_wire_message(&near_max).unwrap();
        assert!(
            near_max_bytes.len() > iroh_gossip::proto::DEFAULT_MAX_MESSAGE_SIZE,
            "near-maximum collaboration transport frame must exceed iroh's old 4 KiB default"
        );
        assert!(near_max_bytes.len() <= GOSSIP_WIRE_MESSAGE_MAX_BYTES);
        assert_eq!(
            node1.gossip.max_message_size(),
            GOSSIP_WIRE_MESSAGE_MAX_BYTES
        );
        assert_eq!(
            node2.gossip.max_message_size(),
            GOSSIP_WIRE_MESSAGE_MAX_BYTES
        );
        sender2
            .broadcast(near_max_bytes.clone().into())
            .await
            .unwrap();

        let near_max_event = tokio::time::timeout(std::time::Duration::from_secs(10), async {
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
        assert_eq!(near_max_event.content.as_ref(), near_max_bytes.as_slice());

        // Cleanup
        shutdown_test_carrier_node(node1).await;
        shutdown_test_carrier_node(node2).await;
    }

    #[tokio::test]
    async fn test_carrier_gossip_provider_chat_bootstrap_sequence_delivers() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();

        let (sk1, did1) = elastos_identity::derive_did(&[11u8; 32]);
        let (sk2, did2) = elastos_identity::derive_did(&[22u8; 32]);
        let node1 = start_carrier_node(&sk1, &did1, dir1.path().to_path_buf())
            .await
            .unwrap();
        let node2 = start_carrier_node(&sk2, &did2, dir2.path().to_path_buf())
            .await
            .unwrap();
        let provider1 = CarrierGossipProvider::new(node1.gossip_state.clone());
        let provider2 = CarrierGossipProvider::new(node2.gossip_state.clone());
        let topic = "__test/chat-bootstrap-provider-delivery";

        provider1
            .send_raw(&serde_json::json!({"op": "gossip_join", "topic": topic, "mode": "direct"}))
            .await
            .unwrap();
        let ticket = provider1
            .send_raw(&serde_json::json!({"op": "get_ticket"}))
            .await
            .unwrap()["data"]["ticket"]
            .as_str()
            .unwrap()
            .to_string();
        let remembered = provider2
            .send_raw(&serde_json::json!({"op": "remember_peer", "ticket": ticket}))
            .await
            .unwrap();
        let peers = remembered["data"]["added"].as_array().unwrap();
        provider2
            .send_raw(&serde_json::json!({"op": "gossip_join", "topic": topic, "mode": "direct"}))
            .await
            .unwrap();
        provider2
            .send_raw(
                &serde_json::json!({"op": "gossip_join_peers", "topic": topic, "peers": peers}),
            )
            .await
            .unwrap();

        let marker = format!("provider marker {}", now_secs());
        for _ in 0..20 {
            provider2
                .send_raw(&serde_json::json!({
                    "op": "gossip_send",
                    "topic": topic,
                    "message": marker,
                    "sender": "provider2",
                    "sender_id": did2,
                }))
                .await
                .unwrap();
            let recv = provider1
                .send_raw(&serde_json::json!({
                    "op": "gossip_recv",
                    "topic": topic,
                    "consumer_id": "provider-test",
                    "limit": 100,
                }))
                .await
                .unwrap();
            let seen = recv["data"]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|msg| msg["content"].as_str() == Some(marker.as_str()));
            if seen {
                shutdown_test_carrier_node(node1).await;
                shutdown_test_carrier_node(node2).await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        shutdown_test_carrier_node(node1).await;
        shutdown_test_carrier_node(node2).await;
        panic!("provider gossip bootstrap sequence did not deliver marker");
    }

    #[tokio::test]
    async fn test_gossip_wire_bound_rejects_send_push_and_ingress_before_effect() {
        let dir = tempfile::tempdir().unwrap();
        let (signing_key, did) = elastos_identity::derive_did(&[34u8; 32]);
        let node = start_carrier_node(&signing_key, &did, dir.path().to_path_buf())
            .await
            .unwrap();
        let provider = CarrierGossipProvider::new(node.gossip_state.clone());
        let topic = "__test/gossip-wire-bound";
        assert_eq!(
            provider
                .send_raw(&serde_json::json!({
                    "op":"gossip_join", "topic":topic, "mode":"direct"
                }))
                .await
                .unwrap()["status"],
            "ok"
        );

        let oversized = gossip_test_message_with_encoded_len(1, GOSSIP_WIRE_MESSAGE_MAX_BYTES + 1);
        let oversized_bytes = serde_json::to_vec(&oversized).unwrap();
        assert_eq!(oversized_bytes.len(), GOSSIP_WIRE_MESSAGE_MAX_BYTES + 1);
        let rejected_send = provider
            .send_raw(&serde_json::json!({
                "op":"gossip_send",
                "topic":topic,
                "message":oversized.content.clone(),
                "sender":oversized.sender_nick.clone(),
                "sender_id":oversized.sender_id.clone(),
                "ts":oversized.ts,
                "nonce":oversized.nonce,
                "signature":oversized.signature.clone(),
                "sender_session_id":oversized.sender_session_id.clone(),
            }))
            .await
            .unwrap();
        assert_eq!(rejected_send["status"], "error");
        assert_eq!(rejected_send["code"], "message_too_large");

        let rejected_push = carrier_gossip_push(
            node.gossip_state.clone(),
            &serde_json::json!({"topic":topic, "message":oversized}),
        )
        .await;
        assert_eq!(rejected_push["ok"], false);
        assert_eq!(rejected_push["code"], "message_too_large");

        let (buffers, peers, topic_peers) = {
            let state = node.gossip_state.lock().await;
            (
                state.buffers.clone(),
                state.peers.clone(),
                state.topic_peers.clone(),
            )
        };
        handle_gossip_event(
            iroh_gossip::api::Event::Received(iroh_gossip::api::Message {
                content: oversized_bytes.into(),
                scope: iroh_gossip::proto::DeliveryScope::Neighbors,
                delivered_from: node.endpoint.id(),
            }),
            &buffers,
            &peers,
            &topic_peers,
            topic,
        )
        .await;
        {
            let buffers = buffers.lock().await;
            let buffer = buffers.get(topic).unwrap();
            assert!(buffer.messages.is_empty());
            assert_eq!(buffer.retained_bytes, 0);
            assert_eq!(buffer.base_index, 0);
        }

        let pushed = gossip_test_message_with_encoded_len(2, GOSSIP_WIRE_MESSAGE_MAX_BYTES);
        let pushed_response = carrier_gossip_push(
            node.gossip_state.clone(),
            &serde_json::json!({"topic":topic, "message":pushed}),
        )
        .await;
        assert_eq!(pushed_response["ok"], true);
        assert_eq!(pushed_response["inserted"], true);

        let received = gossip_test_message_with_encoded_len(3, GOSSIP_WIRE_MESSAGE_MAX_BYTES);
        let received_bytes = serialize_gossip_wire_message(&received).unwrap();
        handle_gossip_event(
            iroh_gossip::api::Event::Received(iroh_gossip::api::Message {
                content: received_bytes.into(),
                scope: iroh_gossip::proto::DeliveryScope::Neighbors,
                delivered_from: node.endpoint.id(),
            }),
            &buffers,
            &peers,
            &topic_peers,
            topic,
        )
        .await;
        let buffers = buffers.lock().await;
        let buffer = buffers.get(topic).unwrap();
        assert_eq!(buffer.messages.len(), 2);
        assert_eq!(buffer.retained_bytes, 2 * GOSSIP_WIRE_MESSAGE_MAX_BYTES);
        drop(buffers);
        shutdown_test_carrier_node(node).await;
    }

    #[tokio::test]
    async fn test_gossip_buffer_byte_eviction_preserves_cursor_and_ack_replay() {
        let dir = tempfile::tempdir().unwrap();
        let (signing_key, did) = elastos_identity::derive_did(&[35u8; 32]);
        let node = start_carrier_node(&signing_key, &did, dir.path().to_path_buf())
            .await
            .unwrap();
        let provider = CarrierGossipProvider::new(node.gossip_state.clone());
        let topic = "__test/gossip-byte-eviction";
        let small_message_bytes = GOSSIP_WIRE_MESSAGE_MAX_BYTES / 3;
        let mut messages = (0..3)
            .map(|index| gossip_test_message_with_encoded_len(index, small_message_bytes))
            .collect::<Vec<_>>();
        messages.extend((3..66).map(|index| {
            gossip_test_message_with_encoded_len(index, GOSSIP_WIRE_MESSAGE_MAX_BYTES)
        }));
        let buffer = test_topic_buffer(messages, 0);
        assert_eq!(buffer.retained_bytes, GOSSIP_TOPIC_BUFFER_MAX_BYTES);
        {
            let mut state = node.gossip_state.lock().await;
            state.joined_topics.insert(topic.to_string());
            state.buffers.lock().await.insert(topic.to_string(), buffer);
        }

        let peek = provider
            .send_raw(&serde_json::json!({
                "op":"gossip_peek", "topic":topic,
                "consumer_id":"byte-eviction", "limit":1
            }))
            .await
            .unwrap();
        assert_eq!(peek["data"]["cursor"], 0);
        assert_eq!(peek["data"]["next_cursor"], 1);
        let ack_request = serde_json::json!({
            "op":"gossip_ack", "topic":topic,
            "consumer_id":"byte-eviction", "cursor":0, "next_cursor":1
        });
        assert_eq!(
            provider.send_raw(&ack_request).await.unwrap()["data"]["advanced"],
            true
        );

        {
            let state = node.gossip_state.lock().await;
            let mut buffers = state.buffers.lock().await;
            let buffer = buffers.get_mut(topic).unwrap();
            let duplicate = buffer.messages.back().unwrap().clone();
            assert!(!push_gossip_buffer_message(buffer, duplicate));
            assert_eq!(buffer.base_index, 0);
            assert_eq!(buffer.retained_bytes, GOSSIP_TOPIC_BUFFER_MAX_BYTES);

            let next = gossip_test_message_with_encoded_len(66, GOSSIP_WIRE_MESSAGE_MAX_BYTES);
            assert!(push_gossip_buffer_message(buffer, next));
            assert_eq!(buffer.base_index, 3);
            assert_eq!(buffer.messages.len(), 64);
            assert_eq!(buffer.retained_bytes, GOSSIP_TOPIC_BUFFER_MAX_BYTES);
        }

        let replay = provider.send_raw(&ack_request).await.unwrap();
        assert_eq!(replay["status"], "ok");
        assert_eq!(replay["data"]["advanced"], false);
        let normalized = provider
            .send_raw(&serde_json::json!({
                "op":"gossip_peek", "topic":topic,
                "consumer_id":"byte-eviction", "limit":1
            }))
            .await
            .unwrap();
        assert_eq!(normalized["data"]["cursor"], 3);
        assert_eq!(normalized["data"]["next_cursor"], 4);
        assert_eq!(normalized["data"]["messages"][0]["nonce"], 4);
        shutdown_test_carrier_node(node).await;
    }

    #[tokio::test]
    async fn test_carrier_gossip_push_pull_over_carrier_client() {
        let dir = tempfile::tempdir().unwrap();
        let (sk, did) = elastos_identity::derive_did(&[33u8; 32]);
        let node = start_carrier_node(&sk, &did, dir.path().to_path_buf())
            .await
            .unwrap();
        let provider = CarrierGossipProvider::new(node.gossip_state.clone());
        let topic = "__test/chat-bootstrap-carrier-push-pull";

        provider
            .send_raw(&serde_json::json!({"op": "gossip_join", "topic": topic, "mode": "direct"}))
            .await
            .unwrap();
        provider
            .send_raw(&serde_json::json!({
                "op": "gossip_send",
                "topic": topic,
                "message": "source-local-message",
                "sender": "source",
                "sender_id": did,
                "ts": 1_700_000_001u64,
                "nonce": 101u64,
            }))
            .await
            .unwrap();

        let mut watcher = node.endpoint.watch_addr();
        let client = CarrierClient::connect_endpoint_addr(watcher.get(), 5)
            .await
            .unwrap();
        let pulled = client.pull_gossip_messages(topic, 10, None).await.unwrap();
        assert!(
            pulled
                .iter()
                .any(|message| message.content == "source-local-message"),
            "Carrier gossip pull must expose the source runtime room buffer"
        );

        let pushed = GossipMessage {
            sender_id: "did:key:zRemote".to_string(),
            sender_nick: "remote".to_string(),
            content: "remote-pushed-message".to_string(),
            ts: 1_700_000_002,
            nonce: 202,
            signature: Some("sig-remote".to_string()),
            sender_session_id: None,
        };
        client.push_gossip_message(topic, &pushed).await.unwrap();
        let recv = provider
            .send_raw(&serde_json::json!({
                "op": "gossip_recv",
                "topic": topic,
                "consumer_id": "push-pull-test",
                "limit": 100,
            }))
            .await
            .unwrap();
        assert!(
            recv["data"]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["content"].as_str() == Some("remote-pushed-message")),
            "Carrier gossip push must insert into the source runtime room buffer"
        );

        shutdown_test_carrier_node(node).await;
    }

    fn cursor_test_message(index: u64) -> GossipMessage {
        GossipMessage {
            sender_id: format!("transport-sender-{index}"),
            sender_nick: format!("transport-nick-{index}"),
            content: format!("opaque-frame-{index}"),
            ts: 1_700_000_000 + index,
            nonce: index + 1,
            signature: Some(format!("untrusted-signature-{index}")),
            sender_session_id: Some(format!("untrusted-session-{index}")),
        }
    }

    async fn install_cursor_test_topic(
        node: &CarrierNode,
        topic: &str,
        base_index: u64,
        message_indexes: impl IntoIterator<Item = u64>,
    ) {
        let mut state = node.gossip_state.lock().await;
        state.joined_topics.insert(topic.to_string());
        state.buffers.lock().await.insert(
            topic.to_string(),
            test_topic_buffer(
                message_indexes.into_iter().map(cursor_test_message),
                base_index,
            ),
        );
    }

    #[tokio::test]
    async fn test_gossip_peek_is_repeatable_and_ack_advances_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        let (signing_key, did) = elastos_identity::derive_did(&[61u8; 32]);
        let node = start_carrier_node(&signing_key, &did, dir.path().to_path_buf())
            .await
            .unwrap();
        let provider = CarrierGossipProvider::new(node.gossip_state.clone());
        let topic = "__test/gossip-peek-repeatable";
        install_cursor_test_topic(&node, topic, 0, 0..3).await;

        let peek_request = serde_json::json!({
            "op": "gossip_peek",
            "topic": topic,
            "consumer_id": "durable-consumer",
            "limit": 1,
        });
        let first = provider.send_raw(&peek_request).await.unwrap();
        {
            let state = node.gossip_state.lock().await;
            let message = cursor_test_message(3);
            push_gossip_buffer_message(state.buffers.lock().await.get_mut(topic).unwrap(), message);
        }
        let repeated = provider.send_raw(&peek_request).await.unwrap();
        assert_eq!(
            first, repeated,
            "peek must retain its exact range when frames are appended"
        );
        assert_eq!(first["status"], "ok");
        assert_eq!(first["data"]["cursor"], 0);
        assert_eq!(first["data"]["next_cursor"], 1);
        assert_eq!(first["data"]["scanned"], 1);
        assert_eq!(
            first["data"]["messages"][0]["sender_id"],
            "transport-sender-0"
        );
        assert_eq!(
            first["data"]["messages"][0]["sender_session_id"], "untrusted-session-0",
            "peek must not filter or reinterpret outer transport metadata"
        );
        {
            let state = node.gossip_state.lock().await;
            let cursors = state.cursors.lock().await;
            assert_eq!(
                cursors
                    .get(&(topic.to_string(), "durable-consumer".to_string()))
                    .unwrap()
                    .position,
                0
            );
        }

        for unobserved_ack in [(0, 3), (0, 4)] {
            let rejected = provider
                .send_raw(&serde_json::json!({
                    "op":"gossip_ack", "topic":topic, "consumer_id":"durable-consumer",
                    "cursor":unobserved_ack.0, "next_cursor":unobserved_ack.1
                }))
                .await
                .unwrap();
            assert_eq!(rejected["status"], "error");
            assert_eq!(rejected["code"], "unobserved_ack_range");
        }

        let ack_request = serde_json::json!({
            "op": "gossip_ack",
            "topic": topic,
            "consumer_id": "durable-consumer",
            "cursor": 0,
            "next_cursor": 1,
        });
        let ack = provider.send_raw(&ack_request).await.unwrap();
        assert_eq!(ack["status"], "ok");
        assert_eq!(ack["data"]["advanced"], true);
        let replay = provider.send_raw(&ack_request).await.unwrap();
        assert_eq!(replay["status"], "ok");
        assert_eq!(replay["data"]["advanced"], false);

        {
            let state = node.gossip_state.lock().await;
            let mut buffers = state.buffers.lock().await;
            let buffer = buffers.get_mut(topic).unwrap();
            pop_gossip_buffer_front(buffer);
            pop_gossip_buffer_front(buffer);
        }
        let replay_after_eviction = provider.send_raw(&ack_request).await.unwrap();
        assert_eq!(replay_after_eviction["status"], "ok");
        assert_eq!(replay_after_eviction["data"]["advanced"], false);
        let conflicting_replay = provider
            .send_raw(&serde_json::json!({
                "op":"gossip_ack", "topic":topic, "consumer_id":"durable-consumer",
                "cursor":0, "next_cursor":2
            }))
            .await
            .unwrap();
        assert_eq!(conflicting_replay["status"], "error");

        let next = provider.send_raw(&peek_request).await.unwrap();
        assert_eq!(next["data"]["cursor"], 2);
        assert_eq!(next["data"]["next_cursor"], 3);
        assert_eq!(next["data"]["messages"][0]["content"], "opaque-frame-2");

        for invalid_ack in [
            serde_json::json!({
                "op":"gossip_ack", "topic":topic, "consumer_id":"durable-consumer",
                "cursor":1, "next_cursor":3
            }),
            serde_json::json!({
                "op":"gossip_ack", "topic":topic, "consumer_id":"durable-consumer",
                "cursor":2, "next_cursor":1
            }),
            serde_json::json!({
                "op":"gossip_ack", "topic":topic, "consumer_id":"durable-consumer",
                "cursor":2, "next_cursor":5
            }),
        ] {
            assert_eq!(
                provider.send_raw(&invalid_ack).await.unwrap()["status"],
                "error",
                "invalid acknowledgement must fail: {invalid_ack}"
            );
        }

        let final_ack = provider
            .send_raw(&serde_json::json!({
                "op":"gossip_ack", "topic":topic, "consumer_id":"durable-consumer",
                "cursor":2, "next_cursor":3
            }))
            .await
            .unwrap();
        assert_eq!(final_ack["data"]["advanced"], true);
        assert_eq!(
            provider.send_raw(&ack_request).await.unwrap()["status"],
            "error",
            "a later acknowledgement must retire the older replay boundary"
        );
        shutdown_test_carrier_node(node).await;
    }

    #[tokio::test]
    async fn test_gossip_peek_enforces_aggregate_encoded_batch_bound() {
        let dir = tempfile::tempdir().unwrap();
        let (signing_key, did) = elastos_identity::derive_did(&[64u8; 32]);
        let node = start_carrier_node(&signing_key, &did, dir.path().to_path_buf())
            .await
            .unwrap();
        let provider = CarrierGossipProvider::new(node.gossip_state.clone());
        let bounded_topic = "__test/gossip-peek-byte-bound";
        let oversized_topic = "__test/gossip-peek-oversized-first";
        let mut valid_messages = Vec::new();
        let mut retained_bytes = 0;
        while retained_bytes < GOSSIP_TOPIC_BUFFER_MAX_BYTES {
            let encoded_len =
                (GOSSIP_TOPIC_BUFFER_MAX_BYTES - retained_bytes).min(GOSSIP_WIRE_MESSAGE_MAX_BYTES);
            let message =
                gossip_test_message_with_encoded_len(valid_messages.len() as u64, encoded_len);
            retained_bytes += encoded_len;
            valid_messages.push(message);
        }
        assert_eq!(retained_bytes, GOSSIP_TOPIC_BUFFER_MAX_BYTES);
        assert!(valid_messages.len() <= GOSSIP_PEEK_MAX_MESSAGES);
        let oversized = gossip_test_message_with_encoded_len(
            valid_messages.len() as u64,
            GOSSIP_PEEK_MAX_BATCH_BYTES,
        );
        {
            let mut state = node.gossip_state.lock().await;
            state.joined_topics.insert(bounded_topic.to_string());
            state.joined_topics.insert(oversized_topic.to_string());
            let mut buffers = state.buffers.lock().await;
            buffers.insert(
                bounded_topic.to_string(),
                test_topic_buffer(valid_messages, 0),
            );
            buffers.insert(
                oversized_topic.to_string(),
                corrupt_test_topic_buffer([oversized], 0),
            );
        }

        let bounded = provider
            .send_raw(&serde_json::json!({
                "op":"gossip_peek", "topic":bounded_topic,
                "consumer_id":"byte-bounded", "limit":GOSSIP_PEEK_MAX_MESSAGES
            }))
            .await
            .unwrap();
        assert_eq!(bounded["status"], "ok");
        assert_eq!(bounded["data"]["cursor"], 0);
        let next_cursor = bounded["data"]["next_cursor"].as_u64().unwrap();
        assert!(next_cursor > 0);
        assert_eq!(
            bounded["data"]["messages"].as_array().unwrap().len() as u64,
            next_cursor
        );
        let encoded_response_len = serde_json::to_vec(&bounded).unwrap().len();
        assert!(
            encoded_response_len <= GOSSIP_PEEK_MAX_BATCH_BYTES,
            "the complete encoded provider response must fit the batch boundary"
        );
        assert!(encoded_response_len > GOSSIP_PEEK_MAX_BATCH_BYTES - 256 * 1024);
        let changed_limit = provider
            .send_raw(&serde_json::json!({
                "op":"gossip_peek", "topic":bounded_topic,
                "consumer_id":"byte-bounded", "limit":1
            }))
            .await
            .unwrap();
        assert_eq!(changed_limit["status"], "error");
        assert_eq!(changed_limit["code"], "peek_limit_conflict");
        let skipped_prefix = provider
            .send_raw(&serde_json::json!({
                "op":"gossip_ack", "topic":bounded_topic,
                "consumer_id":"byte-bounded", "cursor":0,
                "next_cursor":next_cursor + 1
            }))
            .await
            .unwrap();
        assert_eq!(skipped_prefix["status"], "error");
        assert_eq!(skipped_prefix["code"], "unobserved_ack_range");
        assert_eq!(
            provider
                .send_raw(&serde_json::json!({
                    "op":"gossip_ack", "topic":bounded_topic,
                    "consumer_id":"byte-bounded", "cursor":0,
                    "next_cursor":next_cursor
                }))
                .await
                .unwrap()["status"],
            "ok"
        );

        let oversized_first = provider
            .send_raw(&serde_json::json!({
                "op":"gossip_peek", "topic":oversized_topic,
                "consumer_id":"oversized-first", "limit":1
            }))
            .await
            .unwrap();
        assert_eq!(oversized_first["status"], "error");
        assert_eq!(oversized_first["code"], "peek_frame_too_large");
        let state = node.gossip_state.lock().await;
        let cursors = state.cursors.lock().await;
        let cursor = cursors
            .get(&(oversized_topic.to_string(), "oversized-first".to_string()))
            .unwrap();
        assert_eq!(cursor.position, 0);
        assert_eq!(cursor.outstanding_peek, None);
        drop(cursors);
        drop(state);
        shutdown_test_carrier_node(node).await;
    }

    #[tokio::test]
    async fn test_gossip_empty_peek_requires_exact_empty_ack() {
        let dir = tempfile::tempdir().unwrap();
        let (signing_key, did) = elastos_identity::derive_did(&[65u8; 32]);
        let node = start_carrier_node(&signing_key, &did, dir.path().to_path_buf())
            .await
            .unwrap();
        let provider = CarrierGossipProvider::new(node.gossip_state.clone());
        let topic = "__test/gossip-empty-peek";
        install_cursor_test_topic(&node, topic, 0, std::iter::empty()).await;

        let peek_request = serde_json::json!({
            "op":"gossip_peek", "topic":topic, "consumer_id":"empty", "limit":1
        });
        let empty = provider.send_raw(&peek_request).await.unwrap();
        assert_eq!(empty["status"], "ok");
        assert_eq!(empty["data"]["cursor"], 0);
        assert_eq!(empty["data"]["next_cursor"], 0);
        assert_eq!(empty["data"]["messages"].as_array().unwrap().len(), 0);
        let empty_ack = serde_json::json!({
            "op":"gossip_ack", "topic":topic, "consumer_id":"empty",
            "cursor":0, "next_cursor":0
        });
        let accepted = provider.send_raw(&empty_ack).await.unwrap();
        assert_eq!(accepted["status"], "ok");
        assert_eq!(accepted["data"]["advanced"], false);
        assert_eq!(provider.send_raw(&peek_request).await.unwrap(), empty);
        assert_eq!(
            provider.send_raw(&empty_ack).await.unwrap(),
            accepted,
            "a second current empty range must be consumed, not mistaken for replay"
        );

        {
            let state = node.gossip_state.lock().await;
            state
                .buffers
                .lock()
                .await
                .get_mut(topic)
                .unwrap()
                .messages
                .push_back(cursor_test_message(0));
        }
        let appended = provider.send_raw(&peek_request).await.unwrap();
        assert_eq!(appended["data"]["cursor"], 0);
        assert_eq!(appended["data"]["next_cursor"], 1);
        assert_eq!(appended["data"]["messages"].as_array().unwrap().len(), 1);
        assert_eq!(provider.send_raw(&empty_ack).await.unwrap(), accepted);
        assert_eq!(
            provider
                .send_raw(&serde_json::json!({
                    "op":"gossip_ack", "topic":topic, "consumer_id":"empty",
                    "cursor":0, "next_cursor":1
                }))
                .await
                .unwrap()["data"]["advanced"],
            true
        );
        shutdown_test_carrier_node(node).await;
    }

    #[tokio::test]
    async fn test_gossip_peek_normalizes_evicted_prefix_without_crossing_consumers_or_topics() {
        let dir = tempfile::tempdir().unwrap();
        let (signing_key, did) = elastos_identity::derive_did(&[62u8; 32]);
        let node = start_carrier_node(&signing_key, &did, dir.path().to_path_buf())
            .await
            .unwrap();
        let provider = CarrierGossipProvider::new(node.gossip_state.clone());
        let topic_a = "__test/gossip-peek-isolation-a";
        let topic_b = "__test/gossip-peek-isolation-b";
        install_cursor_test_topic(&node, topic_a, 0, 0..3).await;
        install_cursor_test_topic(&node, topic_b, 10, 10..12).await;

        let peek_a = serde_json::json!({
            "op":"gossip_peek", "topic":topic_a, "consumer_id":"consumer-a", "limit":1
        });
        assert_eq!(
            provider.send_raw(&peek_a).await.unwrap()["data"]["cursor"],
            0
        );
        {
            let state = node.gossip_state.lock().await;
            let mut buffers = state.buffers.lock().await;
            let buffer = buffers.get_mut(topic_a).unwrap();
            pop_gossip_buffer_front(buffer);
        }
        let after_eviction = provider.send_raw(&peek_a).await.unwrap();
        assert_eq!(after_eviction["data"]["cursor"], 1);
        assert_eq!(after_eviction["data"]["next_cursor"], 2);
        assert_eq!(
            after_eviction["data"]["messages"][0]["content"],
            "opaque-frame-1"
        );

        let other_consumer = provider
            .send_raw(&serde_json::json!({
                "op":"gossip_peek", "topic":topic_a, "consumer_id":"consumer-b", "limit":1
            }))
            .await
            .unwrap();
        assert_eq!(other_consumer["data"]["cursor"], 1);
        let other_topic = provider
            .send_raw(&serde_json::json!({
                "op":"gossip_peek", "topic":topic_b, "consumer_id":"consumer-a", "limit":1
            }))
            .await
            .unwrap();
        assert_eq!(other_topic["data"]["cursor"], 10);
        assert_eq!(
            other_topic["data"]["messages"][0]["content"],
            "opaque-frame-10"
        );

        let foreign_topic_ack = provider
            .send_raw(&serde_json::json!({
                "op":"gossip_ack", "topic":"__test/not-joined", "consumer_id":"consumer-a",
                "cursor":0, "next_cursor":0
            }))
            .await
            .unwrap();
        assert_eq!(foreign_topic_ack["status"], "error");

        let state = node.gossip_state.lock().await;
        let cursors = state.cursors.lock().await;
        assert_eq!(cursors.len(), 3);
        assert_eq!(
            cursors
                .get(&(topic_a.to_string(), "consumer-a".to_string()))
                .unwrap()
                .position,
            1
        );
        assert_eq!(
            cursors
                .get(&(topic_a.to_string(), "consumer-b".to_string()))
                .unwrap()
                .position,
            1
        );
        assert_eq!(
            cursors
                .get(&(topic_b.to_string(), "consumer-a".to_string()))
                .unwrap()
                .position,
            10
        );
        drop(cursors);
        drop(state);
        shutdown_test_carrier_node(node).await;
    }

    #[tokio::test]
    async fn test_gossip_peek_ack_inputs_and_cursor_capacity_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let (signing_key, did) = elastos_identity::derive_did(&[63u8; 32]);
        let node = start_carrier_node(&signing_key, &did, dir.path().to_path_buf())
            .await
            .unwrap();
        let provider = CarrierGossipProvider::new(node.gossip_state.clone());
        let topic = "__test/gossip-peek-bounds";
        install_cursor_test_topic(&node, topic, 0, 0..1).await;
        node.gossip_state
            .lock()
            .await
            .joined_topics
            .insert("__test/joined-without-buffer".to_string());

        let oversized_topic = "x".repeat(GOSSIP_CARRIER_PUSH_MAX_TOPIC_LEN + 1);
        let oversized_consumer = "x".repeat(GOSSIP_CURSOR_MAX_CONSUMER_ID_BYTES + 1);
        let invalid_peeks = [
            serde_json::json!({"op":"gossip_peek","consumer_id":"consumer","limit":1}),
            serde_json::json!({"op":"gossip_peek","topic":7,"consumer_id":"consumer","limit":1}),
            serde_json::json!({"op":"gossip_peek","topic":"bad\ntopic","consumer_id":"consumer","limit":1}),
            serde_json::json!({"op":"gossip_peek","topic":oversized_topic,"consumer_id":"consumer","limit":1}),
            serde_json::json!({"op":"gossip_peek","topic":topic,"limit":1}),
            serde_json::json!({"op":"gossip_peek","topic":topic,"consumer_id":"","limit":1}),
            serde_json::json!({"op":"gossip_peek","topic":topic,"consumer_id":oversized_consumer,"limit":1}),
            serde_json::json!({"op":"gossip_peek","topic":topic,"consumer_id":"consumer"}),
            serde_json::json!({"op":"gossip_peek","topic":topic,"consumer_id":"consumer","limit":0}),
            serde_json::json!({"op":"gossip_peek","topic":topic,"consumer_id":"consumer","limit":GOSSIP_PEEK_MAX_MESSAGES + 1}),
            serde_json::json!({"op":"gossip_peek","topic":topic,"consumer_id":"consumer","limit":"1"}),
            serde_json::json!({"op":"gossip_peek","topic":"__test/not-joined","consumer_id":"consumer","limit":1}),
            serde_json::json!({"op":"gossip_peek","topic":"__test/joined-without-buffer","consumer_id":"consumer","limit":1}),
        ];
        for request in invalid_peeks {
            assert_eq!(
                provider.send_raw(&request).await.unwrap()["status"],
                "error",
                "invalid peek must fail: {request}"
            );
        }

        provider
            .send_raw(&serde_json::json!({
                "op":"gossip_peek", "topic":topic, "consumer_id":"ack-input", "limit":1
            }))
            .await
            .unwrap();
        for request in [
            serde_json::json!({
                "op":"gossip_ack", "topic":topic, "consumer_id":"ack-input", "next_cursor":1
            }),
            serde_json::json!({
                "op":"gossip_ack", "topic":topic, "consumer_id":"ack-input", "cursor":0
            }),
            serde_json::json!({
                "op":"gossip_ack", "topic":topic, "consumer_id":"ack-input",
                "cursor":"0", "next_cursor":1
            }),
            serde_json::json!({
                "op":"gossip_ack", "topic":topic, "consumer_id":"never-peeked",
                "cursor":0, "next_cursor":1
            }),
        ] {
            assert_eq!(
                provider.send_raw(&request).await.unwrap()["status"],
                "error",
                "invalid ack must fail: {request}"
            );
        }

        let existing_consumers = {
            let state = node.gossip_state.lock().await;
            let mut cursors = state.cursors.lock().await;
            cursors.clear();
            let consumers = (0..MAX_CURSORS)
                .map(|index| format!("bounded-{index}"))
                .collect::<Vec<_>>();
            for consumer in &consumers {
                cursors.insert(
                    (topic.to_string(), consumer.clone()),
                    GossipConsumerCursor::at(0),
                );
            }
            consumers
        };
        let bounded_peek = provider
            .send_raw(&serde_json::json!({
                "op":"gossip_peek", "topic":topic, "consumer_id":"new-consumer", "limit":1
            }))
            .await
            .unwrap();
        assert_eq!(bounded_peek["status"], "ok");
        let evicted_consumer = {
            let state = node.gossip_state.lock().await;
            assert!(state.bootstrap_peers.is_empty());
            assert_eq!(state.joined_topics.len(), 2);
            let cursors = state.cursors.lock().await;
            assert!(cursors.len() <= MAX_CURSORS);
            assert!(cursors.contains_key(&(topic.to_string(), "new-consumer".to_string())));
            existing_consumers
                .into_iter()
                .find(|consumer| !cursors.contains_key(&(topic.to_string(), consumer.clone())))
                .expect("cursor pressure must evict at least one old consumer")
        };
        let evicted_ack = provider
            .send_raw(&serde_json::json!({
                "op":"gossip_ack", "topic":topic, "consumer_id":evicted_consumer,
                "cursor":0, "next_cursor":1
            }))
            .await
            .unwrap();
        assert_eq!(evicted_ack["status"], "error");
        assert_eq!(evicted_ack["code"], "unknown_cursor");

        shutdown_test_carrier_node(node).await;
    }

    /// Prove that two consumers sharing the same gossip buffer see each
    /// other's messages. This is the core invariant for same-runtime
    /// native↔WASM chat interop: both capsules use the shared buffer
    /// with different consumer_ids.
    #[tokio::test]
    async fn test_shared_buffer_cross_consumer_delivery() {
        let buffers: Arc<Mutex<HashMap<String, TopicBuffer>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let cursors: Arc<Mutex<HashMap<(String, String), GossipConsumerCursor>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let topic = "#general".to_string();

        // Create buffer
        {
            let mut bufs = buffers.lock().await;
            bufs.insert(topic.clone(), TopicBuffer::default());
        }

        // Consumer A (native chat) writes a message
        {
            let mut bufs = buffers.lock().await;
            let buf = bufs.get_mut(&topic).unwrap();
            let message = GossipMessage {
                sender_id: "did:key:zAlice".to_string(),
                sender_nick: "alice".to_string(),
                content: "hello from native".to_string(),
                ts: 1000,
                nonce: 1,
                signature: Some("sig_alice".to_string()),
                sender_session_id: None,
            };
            push_gossip_buffer_message(buf, message);
        }

        // Consumer B (WASM chat) reads with different consumer_id
        {
            let bufs = buffers.lock().await;
            let buf = bufs.get(&topic).unwrap();
            let mut curs = cursors.lock().await;
            let cursor = curs
                .entry((topic.clone(), "chat-wasm".to_string()))
                .or_insert_with(|| GossipConsumerCursor::at(buf.base_index));
            let start = (cursor.position - buf.base_index) as usize;
            let messages: Vec<&GossipMessage> = buf.messages.iter().skip(start).take(50).collect();

            assert_eq!(messages.len(), 1, "WASM consumer must see native's message");
            assert_eq!(messages[0].content, "hello from native");
            assert_eq!(messages[0].sender_nick, "alice");
            cursor.position = buf.base_index + start as u64 + messages.len() as u64;
        }

        // Consumer A (native chat) reads — should also see its own message
        {
            let bufs = buffers.lock().await;
            let buf = bufs.get(&topic).unwrap();
            let mut curs = cursors.lock().await;
            let cursor = curs
                .entry((topic.clone(), "chat-native".to_string()))
                .or_insert_with(|| GossipConsumerCursor::at(buf.base_index));
            let start = (cursor.position - buf.base_index) as usize;
            let messages: Vec<&GossipMessage> = buf.messages.iter().skip(start).take(50).collect();

            assert_eq!(
                messages.len(),
                1,
                "native consumer must see its own message"
            );
            cursor.position = buf.base_index + start as u64 + messages.len() as u64;
        }

        // Consumer B writes a reply
        {
            let mut bufs = buffers.lock().await;
            let buf = bufs.get_mut(&topic).unwrap();
            let message = GossipMessage {
                sender_id: "did:key:zBob".to_string(),
                sender_nick: "bob".to_string(),
                content: "hello from wasm".to_string(),
                ts: 1001,
                nonce: 2,
                signature: Some("sig_bob".to_string()),
                sender_session_id: None,
            };
            push_gossip_buffer_message(buf, message);
        }

        // Consumer A reads again — should see only the new message (cursor advanced)
        {
            let bufs = buffers.lock().await;
            let buf = bufs.get(&topic).unwrap();
            let mut curs = cursors.lock().await;
            let cursor = curs
                .entry((topic.clone(), "chat-native".to_string()))
                .or_insert_with(|| GossipConsumerCursor::at(buf.base_index));
            let start = (cursor.position - buf.base_index) as usize;
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

    #[test]
    fn test_gossip_buffer_deduplicates_replayed_delivery() {
        let mut buffer = TopicBuffer::default();
        let message = GossipMessage {
            sender_id: "did:key:zAlice".to_string(),
            sender_nick: "alice".to_string(),
            content: "same signed payload".to_string(),
            ts: 1000,
            nonce: 1,
            signature: Some("sig_alice".to_string()),
            sender_session_id: None,
        };
        assert!(push_gossip_buffer_message(&mut buffer, message.clone()));

        let mut replay = message.clone();
        replay.nonce = 2;
        assert!(!push_gossip_buffer_message(&mut buffer, replay));
        assert_eq!(buffer.messages.len(), 1);

        let mut next_message = message;
        next_message.content = "different payload".to_string();
        next_message.nonce = 3;
        assert!(push_gossip_buffer_message(&mut buffer, next_message));
        assert_eq!(buffer.messages.len(), 2);
    }
}
