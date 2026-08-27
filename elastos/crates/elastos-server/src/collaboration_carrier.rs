//! Internal Carrier transport for one verified collaboration network.

use std::sync::Arc;

use anyhow::Context;
use elastos_runtime::provider::Provider;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::collaboration_network::VerifiedCollaborationNetworkProfile;
use crate::collaboration_protocol::{
    verify_collaboration_transport_frame, MAX_COLLABORATION_TRANSPORT_FRAME_BYTES,
};

const COLLABORATION_TOPIC_DOMAIN: &[u8] = b"elastos.collaboration.carrier.topic.v1";
const COLLABORATION_TOPIC_PREFIX: &str = "__elastos_internal/collaboration-v1/";
const COLLABORATION_RECEIVE_BATCH_SIZE: usize = 32;
const MAX_RECEIVE_RESPONSE_BYTES: usize =
    MAX_COLLABORATION_TRANSPORT_FRAME_BYTES * COLLABORATION_RECEIVE_BATCH_SIZE + 64 * 1024;

/// An exact collaboration-network Carrier subscription.
///
/// Its topology, topic, and consumer remain private so send and peek/ack cannot
/// be redirected by a caller after the verified-profile join succeeds.
pub struct JoinedCollaborationNetwork {
    carrier: Arc<dyn Provider>,
    topic: String,
    consumer_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollaborationCarrierSendOutcome {
    RemoteBroadcast { peer_count: u32 },
    LocalOnlyBuffered,
}

/// Decoded payloads are read-only; the private Carrier range cannot be changed
/// or acknowledged independently of the batch that produced them.
///
/// ```compile_fail
/// use elastos_server::collaboration_carrier::CollaborationCarrierReceiveBatch;
///
/// fn discard_before_ack(batch: &mut CollaborationCarrierReceiveBatch) {
///     batch.envelopes.clear();
/// }
/// ```
#[derive(Clone)]
pub struct CollaborationCarrierReceiveBatch {
    envelopes: Vec<Vec<u8>>,
    rejected_frames: usize,
    consumer_id: String,
    cursor: u64,
    next_cursor: u64,
    limit: usize,
}

impl CollaborationCarrierReceiveBatch {
    pub fn envelopes(&self) -> &[Vec<u8>] {
        &self.envelopes
    }

    pub fn rejected_frames(&self) -> usize {
        self.rejected_frames
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CarrierResponse<T> {
    status: String,
    data: T,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RememberPeerData {
    added: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JoinExactData {
    topic: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum CarrierBroadcast {
    LocalOnly,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendResponse {
    status: String,
    broadcast: Option<CarrierBroadcast>,
    data: SendData,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendData {
    remote_peer_count: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiveData {
    messages: Vec<serde_json::Value>,
    scanned: usize,
    limit: usize,
    cursor: u64,
    next_cursor: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AckData {
    cursor: u64,
    next_cursor: u64,
    advanced: bool,
}

/// Raw collaboration-network profiles cannot call this API:
///
/// ```compile_fail
/// use std::sync::Arc;
/// use elastos_runtime::provider::Provider;
/// use elastos_server::collaboration_carrier::join_collaboration_network;
/// use elastos_server::collaboration_network::CollaborationNetworkProfile;
///
/// async fn raw_profile_is_not_authority(
///     carrier: Arc<dyn Provider>,
///     raw_profile: &CollaborationNetworkProfile,
/// ) {
///     join_collaboration_network(carrier, raw_profile).await.unwrap();
/// }
/// ```
pub async fn join_collaboration_network(
    carrier: Arc<dyn Provider>,
    profile: &VerifiedCollaborationNetworkProfile,
) -> anyhow::Result<JoinedCollaborationNetwork> {
    let consumer_id = random_collaboration_consumer_id()?;
    let topic = collaboration_topic(profile);
    let mut peers = Vec::with_capacity(profile.profile().bootstrap_peers.len());

    for bootstrap in &profile.profile().bootstrap_peers {
        let response = carrier
            .send_raw(&serde_json::json!({
                "op": "remember_peer",
                "ticket": bootstrap.connect_ticket,
            }))
            .await
            .context("Carrier remember_peer failed for collaboration bootstrap")?;
        let data: RememberPeerData = require_ok_response(response, "remember_peer")?;
        if data.added.is_empty()
            || data
                .added
                .iter()
                .any(|node_id| node_id != &bootstrap.node_id)
        {
            anyhow::bail!("Carrier remember_peer returned a mismatched collaboration bootstrap");
        }
        peers.push(bootstrap.node_id.clone());
    }

    let response = carrier
        .send_raw(&serde_json::json!({
            "op": "gossip_join_exact",
            "topic": topic,
            "peers": peers,
        }))
        .await
        .context("Carrier gossip_join_exact failed for collaboration network")?;
    let data: JoinExactData = require_ok_response(response, "gossip_join_exact")?;
    if data.topic != topic {
        anyhow::bail!("Carrier gossip_join_exact returned a mismatched collaboration topic");
    }

    Ok(JoinedCollaborationNetwork {
        carrier,
        topic,
        consumer_id,
    })
}

impl JoinedCollaborationNetwork {
    pub async fn send(&self, frame: &[u8]) -> anyhow::Result<CollaborationCarrierSendOutcome> {
        let message =
            std::str::from_utf8(frame).context("collaboration Carrier frame is not UTF-8")?;
        if frame.len() > MAX_COLLABORATION_TRANSPORT_FRAME_BYTES {
            anyhow::bail!("collaboration Carrier frame exceeds the byte limit");
        }
        let response = self
            .carrier
            .send_raw(&serde_json::json!({
                "op": "gossip_send",
                "topic": self.topic,
                "message": message,
            }))
            .await
            .context("Carrier gossip_send failed for collaboration network")?;
        let response: SendResponse = serde_json::from_value(response)
            .context("malformed Carrier gossip_send response for collaboration network")?;
        if response.status != "ok" {
            anyhow::bail!("Carrier gossip_send rejected the collaboration frame");
        }
        match (response.data.remote_peer_count, response.broadcast) {
            (peer_count, None) if peer_count > 0 => {
                Ok(CollaborationCarrierSendOutcome::RemoteBroadcast { peer_count })
            }
            (_, Some(CarrierBroadcast::LocalOnly)) => {
                Ok(CollaborationCarrierSendOutcome::LocalOnlyBuffered)
            }
            _ => anyhow::bail!("inconsistent Carrier gossip_send collaboration result"),
        }
    }

    pub async fn peek(&self) -> anyhow::Result<CollaborationCarrierReceiveBatch> {
        let response = self
            .carrier
            .send_raw(&serde_json::json!({
                "op": "gossip_peek",
                "topic": self.topic,
                "consumer_id": self.consumer_id,
                "limit": COLLABORATION_RECEIVE_BATCH_SIZE,
            }))
            .await
            .context("Carrier gossip_peek failed for collaboration network")?;
        require_bounded_response(&response)?;
        let data: ReceiveData = require_ok_response(response, "gossip_peek")?;
        let scanned = u64::try_from(data.scanned)
            .context("Carrier gossip_peek scanned count exceeds the cursor range")?;
        if data.limit != COLLABORATION_RECEIVE_BATCH_SIZE
            || data.messages.len() > data.limit
            || data.scanned != data.messages.len()
            || data.next_cursor.checked_sub(data.cursor) != Some(scanned)
        {
            anyhow::bail!("Carrier gossip_peek returned an invalid collaboration batch");
        }

        let mut envelopes = Vec::with_capacity(data.messages.len());
        let mut rejected_frames = 0;
        for frame in data.messages {
            match decode_frame(&frame) {
                Some(envelope) => envelopes.push(envelope),
                None => rejected_frames += 1,
            }
        }
        Ok(CollaborationCarrierReceiveBatch {
            envelopes,
            rejected_frames,
            consumer_id: self.consumer_id.clone(),
            cursor: data.cursor,
            next_cursor: data.next_cursor,
            limit: data.limit,
        })
    }

    pub async fn ack(&self, batch: &CollaborationCarrierReceiveBatch) -> anyhow::Result<()> {
        if batch.consumer_id != self.consumer_id || batch.limit != COLLABORATION_RECEIVE_BATCH_SIZE
        {
            anyhow::bail!("collaboration Carrier batch belongs to another subscription");
        }
        let response = self
            .carrier
            .send_raw(&serde_json::json!({
                "op": "gossip_ack",
                "topic": self.topic,
                "consumer_id": self.consumer_id,
                "cursor": batch.cursor,
                "next_cursor": batch.next_cursor,
            }))
            .await
            .context("Carrier gossip_ack failed for collaboration network")?;
        let data: AckData = require_ok_response(response, "gossip_ack")?;
        if data.cursor != batch.cursor
            || data.next_cursor != batch.next_cursor
            || (data.cursor == data.next_cursor && data.advanced)
        {
            anyhow::bail!("Carrier gossip_ack returned an invalid collaboration range");
        }
        Ok(())
    }
}

fn random_collaboration_consumer_id() -> anyhow::Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|error| {
        anyhow::anyhow!("OS entropy unavailable for collaboration consumer: {error}")
    })?;
    Ok(hex::encode(bytes))
}

struct BoundedResponseCounter {
    bytes: usize,
}

impl std::io::Write for BoundedResponseCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let Some(next) = self.bytes.checked_add(bytes.len()) else {
            return Err(std::io::Error::other(
                "collaboration response byte count overflow",
            ));
        };
        if next > MAX_RECEIVE_RESPONSE_BYTES {
            return Err(std::io::Error::other(
                "collaboration response byte limit exceeded",
            ));
        }
        self.bytes = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn require_bounded_response(response: &serde_json::Value) -> anyhow::Result<()> {
    let mut counter = BoundedResponseCounter { bytes: 0 };
    serde_json::to_writer(&mut counter, response)
        .context("Carrier gossip_peek collaboration response exceeds the byte limit")
}

fn collaboration_topic(profile: &VerifiedCollaborationNetworkProfile) -> String {
    // The digest keeps the plaintext network_id out of Carrier topic names. It
    // provides neither secrecy against guessing nor authorization.
    let mut hasher = Sha256::new();
    hasher.update(COLLABORATION_TOPIC_DOMAIN);
    hasher.update(b"\0");
    hasher.update(profile.profile().network_id.as_bytes());
    format!(
        "{COLLABORATION_TOPIC_PREFIX}{}",
        hex::encode(hasher.finalize())
    )
}

fn require_ok_response<T: DeserializeOwned>(
    response: serde_json::Value,
    operation: &str,
) -> anyhow::Result<T> {
    let response: CarrierResponse<T> = serde_json::from_value(response)
        .with_context(|| format!("malformed Carrier {operation} collaboration response"))?;
    if response.status != "ok" {
        anyhow::bail!("Carrier {operation} rejected the collaboration operation");
    }
    Ok(response.data)
}

fn decode_frame(frame: &serde_json::Value) -> Option<Vec<u8>> {
    let encoded = frame.as_object()?.get("content")?.as_str()?;
    if encoded.is_empty() || encoded.len() > MAX_COLLABORATION_TRANSPORT_FRAME_BYTES {
        return None;
    }
    let frame = encoded.as_bytes().to_vec();
    verify_collaboration_transport_frame(&frame).ok()?;
    Some(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use elastos_runtime::provider::{ProviderError, ResourceRequest, ResourceResponse};
    use elastos_runtime::signature::{generate_keypair, SigningKey};

    use crate::collaboration_network::{
        canonical_collaboration_network_profile_payload_bytes,
        validate_collaboration_network_profile, CollaborationBootstrapPeer,
        CollaborationNetworkProfile, CollaborationNetworkProfileMode,
        SignedCollaborationNetworkProfile, COLLABORATION_NETWORK_PROFILE_SCHEMA,
        COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
    };
    use crate::collaboration_protocol::sign_collaboration_transport_frame;

    enum FakeReply {
        Value(serde_json::Value),
        Error(&'static str),
    }

    struct FakeCarrier {
        requests: Mutex<Vec<serde_json::Value>>,
        replies: Mutex<VecDeque<FakeReply>>,
    }

    impl FakeCarrier {
        fn new(replies: impl IntoIterator<Item = FakeReply>) -> Arc<Self> {
            Arc::new(Self {
                requests: Mutex::new(Vec::new()),
                replies: Mutex::new(replies.into_iter().collect()),
            })
        }

        fn requests(&self) -> Vec<serde_json::Value> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl Provider for FakeCarrier {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "fake Carrier supports raw operations only".to_string(),
            ))
        }

        fn schemes(&self) -> Vec<&'static str> {
            Vec::new()
        }

        fn name(&self) -> &'static str {
            "fake-collaboration-carrier"
        }

        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            self.requests.lock().unwrap().push(request.clone());
            match self.replies.lock().unwrap().pop_front() {
                Some(FakeReply::Value(value)) => Ok(value),
                Some(FakeReply::Error(message)) => {
                    Err(ProviderError::Provider(message.to_string()))
                }
                None => Err(ProviderError::Provider(
                    "fake Carrier has no queued response".to_string(),
                )),
            }
        }
    }

    fn ticket_for(secret_byte: u8) -> CollaborationBootstrapPeer {
        let secret = iroh::SecretKey::from_bytes(&[secret_byte; 32]);
        let endpoint = iroh::EndpointAddr::from(secret.public());
        let ticket_json = serde_json::json!({
            "topic": null,
            "endpoints": [endpoint],
        });
        let mut connect_ticket =
            data_encoding::BASE32_NOPAD.encode(&serde_json::to_vec(&ticket_json).unwrap());
        connect_ticket.make_ascii_lowercase();
        CollaborationBootstrapPeer {
            node_id: secret.public().to_string(),
            connect_ticket,
        }
    }

    fn verified_profile(
        signing_key: &SigningKey,
        network_id: &str,
        revision: u64,
        previous: Option<&VerifiedCollaborationNetworkProfile>,
        bootstrap_peers: Vec<CollaborationBootstrapPeer>,
    ) -> VerifiedCollaborationNetworkProfile {
        let signer_did = crate::crypto::encode_did_key(&signing_key.verifying_key());
        let payload = CollaborationNetworkProfile {
            schema: COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
            network_id: network_id.to_string(),
            revision,
            previous_profile_sha256: previous.map(|profile| profile.profile_sha256().to_string()),
            signer_did: signer_did.clone(),
            bootstrap_peers,
            default_conversation: None,
        };
        let payload_bytes =
            canonical_collaboration_network_profile_payload_bytes(&payload).unwrap();
        let (signature, envelope_signer) = crate::crypto::domain_separated_sign(
            signing_key,
            COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
            &payload_bytes,
        );
        let envelope = SignedCollaborationNetworkProfile {
            payload,
            signature,
            signer_did: envelope_signer,
        };
        let bytes = serde_json::to_vec(&serde_json::to_value(envelope).unwrap()).unwrap();
        match validate_collaboration_network_profile(
            Some(&bytes),
            network_id,
            &[signer_did],
            previous,
        )
        .unwrap()
        {
            CollaborationNetworkProfileMode::Configured(profile) => profile,
            CollaborationNetworkProfileMode::Isolated => panic!("expected configured profile"),
        }
    }

    fn remember_ok(node_id: &str) -> FakeReply {
        FakeReply::Value(serde_json::json!({
            "status": "ok",
            "data": {"added": [node_id]},
        }))
    }

    fn join_ok(topic: &str) -> FakeReply {
        FakeReply::Value(serde_json::json!({
            "status": "ok",
            "data": {"topic": topic},
        }))
    }

    fn signed_frame(envelope: &[u8]) -> Vec<u8> {
        let signing_key = SigningKey::from_bytes(&[0x42; 32]);
        sign_collaboration_transport_frame(&signing_key, envelope).unwrap()
    }

    fn gossip_frame(envelope: &[u8]) -> serde_json::Value {
        let signed = signed_frame(envelope);
        serde_json::json!({
            "content": std::str::from_utf8(&signed).unwrap(),
        })
    }

    fn peek_ok(cursor: u64, next_cursor: u64, messages: Vec<serde_json::Value>) -> FakeReply {
        let scanned = messages.len();
        FakeReply::Value(serde_json::json!({
            "status": "ok",
            "data": {
                "cursor": cursor,
                "next_cursor": next_cursor,
                "messages": messages,
                "scanned": scanned,
                "limit": COLLABORATION_RECEIVE_BATCH_SIZE,
            },
        }))
    }

    fn ack_ok(cursor: u64, next_cursor: u64, advanced: bool) -> FakeReply {
        FakeReply::Value(serde_json::json!({
            "status": "ok",
            "data": {
                "cursor": cursor,
                "next_cursor": next_cursor,
                "advanced": advanced,
            },
        }))
    }

    #[tokio::test]
    async fn collaboration_carrier_joins_only_verified_profile_peers_in_exact_sequence() {
        let (signing_key, _) = generate_keypair();
        let peer_one = ticket_for(21);
        let peer_two = ticket_for(22);
        let profile = verified_profile(
            &signing_key,
            "collaboration-join-test",
            1,
            None,
            vec![peer_one.clone(), peer_two.clone()],
        );
        let topic = collaboration_topic(&profile);
        let carrier = FakeCarrier::new([
            remember_ok(&peer_one.node_id),
            remember_ok(&peer_two.node_id),
            join_ok(&topic),
        ]);

        let _joined = join_collaboration_network(carrier.clone(), &profile)
            .await
            .unwrap();

        let requests = carrier.requests();
        assert_eq!(
            requests,
            vec![
                serde_json::json!({
                    "op": "remember_peer",
                    "ticket": peer_one.connect_ticket,
                }),
                serde_json::json!({
                    "op": "remember_peer",
                    "ticket": peer_two.connect_ticket,
                }),
                serde_json::json!({
                    "op": "gossip_join_exact",
                    "topic": topic,
                    "peers": [peer_one.node_id, peer_two.node_id],
                }),
            ]
        );
        assert!(requests.iter().all(|request| {
            matches!(
                request["op"].as_str(),
                Some("remember_peer" | "gossip_join_exact")
            )
        }));
    }

    #[tokio::test]
    async fn collaboration_carrier_empty_profile_joins_exact_local_only() {
        let (signing_key, _) = generate_keypair();
        let profile = verified_profile(
            &signing_key,
            "collaboration-empty-test",
            1,
            None,
            Vec::new(),
        );
        let topic = collaboration_topic(&profile);
        let carrier = FakeCarrier::new([join_ok(&topic)]);

        let _joined = join_collaboration_network(carrier.clone(), &profile)
            .await
            .unwrap();

        assert_eq!(
            carrier.requests(),
            vec![serde_json::json!({
                "op": "gossip_join_exact",
                "topic": topic,
                "peers": [],
            })]
        );
    }

    #[test]
    fn collaboration_carrier_topic_is_stable_across_revisions_and_network_separated() {
        let (signing_key, _) = generate_keypair();
        let initial = verified_profile(
            &signing_key,
            "collaboration-topic-test",
            1,
            None,
            Vec::new(),
        );
        let updated = verified_profile(
            &signing_key,
            "collaboration-topic-test",
            2,
            Some(&initial),
            vec![ticket_for(23)],
        );
        let other = verified_profile(
            &signing_key,
            "collaboration-other-test",
            1,
            None,
            Vec::new(),
        );

        let topic = collaboration_topic(&initial);
        assert_eq!(topic, collaboration_topic(&updated));
        assert_ne!(topic, collaboration_topic(&other));
        assert_eq!(topic.len(), COLLABORATION_TOPIC_PREFIX.len() + 64);
        assert!(!topic.contains("collaboration-topic-test"));
        assert!(!topic.contains(initial.profile_sha256()));
    }

    #[tokio::test]
    async fn collaboration_carrier_bootstrap_failures_prevent_exact_join() {
        let (signing_key, _) = generate_keypair();
        let peer = ticket_for(24);
        let profile = verified_profile(
            &signing_key,
            "collaboration-bootstrap-failure",
            1,
            None,
            vec![peer.clone()],
        );
        let cases = [
            vec![FakeReply::Error("remember failed")],
            vec![FakeReply::Value(serde_json::json!({
                "status": "ok",
                "data": {"added": []},
            }))],
            vec![FakeReply::Value(serde_json::json!({
                "status": "ok",
                "data": {"added": [ticket_for(25).node_id]},
            }))],
            vec![FakeReply::Value(serde_json::json!({
                "status": "ok",
                "data": {"added": [peer.node_id], "unexpected": true},
            }))],
        ];

        for replies in cases {
            let carrier = FakeCarrier::new(replies);
            assert!(join_collaboration_network(carrier.clone(), &profile)
                .await
                .is_err());
            let requests = carrier.requests();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0]["op"], "remember_peer");
        }
    }

    #[tokio::test]
    async fn collaboration_carrier_rejects_existing_or_mismatched_exact_join() {
        let (signing_key, _) = generate_keypair();
        let profile = verified_profile(
            &signing_key,
            "collaboration-join-failure",
            1,
            None,
            Vec::new(),
        );
        let topic = collaboration_topic(&profile);
        let responses = [
            FakeReply::Value(serde_json::json!({
                "status": "error",
                "code": "already_joined",
                "message": "already joined",
            })),
            FakeReply::Value(serde_json::json!({
                "status": "ok",
                "data": {"topic": "__elastos_internal/foreign"},
            })),
            FakeReply::Value(serde_json::json!({
                "status": "ok",
                "data": {"topic": topic, "unexpected": true},
            })),
        ];

        for response in responses {
            let carrier = FakeCarrier::new([response]);
            assert!(join_collaboration_network(carrier.clone(), &profile)
                .await
                .is_err());
            assert_eq!(carrier.requests()[0]["op"], "gossip_join_exact");
        }
    }

    #[tokio::test]
    async fn collaboration_carrier_send_preserves_bytes_bounds_and_transport_outcomes() {
        let (signing_key, _) = generate_keypair();
        let profile =
            verified_profile(&signing_key, "collaboration-send-test", 1, None, Vec::new());
        let topic = collaboration_topic(&profile);
        let carrier = FakeCarrier::new([
            join_ok(&topic),
            FakeReply::Value(serde_json::json!({
                "status": "ok",
                "data": {"remote_peer_count": 2},
            })),
            FakeReply::Value(serde_json::json!({
                "status": "ok",
                "broadcast": "local_only",
                "data": {"remote_peer_count": 3},
            })),
            FakeReply::Value(serde_json::json!({
                "status": "ok",
                "data": {"remote_peer_count": 0},
            })),
        ]);
        let joined = join_collaboration_network(carrier.clone(), &profile)
            .await
            .unwrap();
        let envelope = br#"{"frame":"ok"}"#;

        assert_eq!(
            joined.send(envelope).await.unwrap(),
            CollaborationCarrierSendOutcome::RemoteBroadcast { peer_count: 2 }
        );
        assert_eq!(
            joined.send(envelope).await.unwrap(),
            CollaborationCarrierSendOutcome::LocalOnlyBuffered
        );
        let request_count = carrier.requests().len();
        assert!(joined
            .send(&vec![b'x'; MAX_COLLABORATION_TRANSPORT_FRAME_BYTES + 1])
            .await
            .is_err());
        assert_eq!(carrier.requests().len(), request_count);
        assert!(joined.send(envelope).await.is_err());

        for request in &carrier.requests()[1..3] {
            assert_eq!(request["op"], "gossip_send");
            assert_eq!(request["topic"], topic);
            assert_eq!(request["message"].as_str().unwrap().as_bytes(), envelope);
            assert_eq!(request.as_object().unwrap().len(), 3);
        }
    }

    #[tokio::test]
    async fn collaboration_carrier_peek_ack_preserves_exact_ranges() {
        let (signing_key, _) = generate_keypair();
        let profile = verified_profile(&signing_key, "peek-ack-test", 1, None, Vec::new());
        let topic = collaboration_topic(&profile);
        let first_envelope = br#"{"frame":1}"#.to_vec();
        let next_envelope = br#"{"frame":2}"#.to_vec();
        let first_frame = signed_frame(&first_envelope);
        let next_frame = signed_frame(&next_envelope);
        let first_peek = peek_ok(0, 1, vec![gossip_frame(&first_envelope)]);
        let carrier = FakeCarrier::new([
            join_ok(&topic),
            first_peek,
            peek_ok(0, 1, vec![gossip_frame(&first_envelope)]),
            peek_ok(0, 1, vec![gossip_frame(&first_envelope)]),
            ack_ok(0, 1, true),
            ack_ok(0, 1, false),
            peek_ok(1, 2, vec![gossip_frame(&next_envelope)]),
        ]);
        let joined = join_collaboration_network(carrier.clone(), &profile)
            .await
            .unwrap();

        let batch = joined.peek().await.unwrap();
        let repeated = joined.peek().await.unwrap();
        assert_eq!(batch.envelopes(), std::slice::from_ref(&first_frame));
        assert_eq!(repeated.envelopes(), batch.envelopes());
        assert_eq!(repeated.cursor, batch.cursor);
        assert_eq!(repeated.next_cursor, batch.next_cursor);
        assert_eq!(repeated.consumer_id, batch.consumer_id);
        drop(repeated); // Simulated caller crash before durable acceptance.
        let after_append = joined.peek().await.unwrap();
        assert_eq!(after_append.envelopes(), &[first_frame]);
        assert_eq!((after_append.cursor, after_append.next_cursor), (0, 1));

        joined.ack(&batch).await.unwrap();
        joined.ack(&batch).await.unwrap();
        let next = joined.peek().await.unwrap();
        assert_eq!(next.envelopes(), &[next_frame]);
        assert_eq!((next.cursor, next.next_cursor), (1, 2));

        let requests = carrier.requests();
        assert_eq!(
            requests[1..]
                .iter()
                .map(|request| request["op"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "gossip_peek",
                "gossip_peek",
                "gossip_peek",
                "gossip_ack",
                "gossip_ack",
                "gossip_peek",
            ]
        );
        assert!(requests[1..].iter().all(|request| {
            request["consumer_id"].as_str() == Some(joined.consumer_id.as_str())
        }));
    }

    #[tokio::test]
    async fn collaboration_carrier_empty_peek_and_ack_are_exact() {
        let (signing_key, _) = generate_keypair();
        let profile = verified_profile(&signing_key, "empty-peek-test", 1, None, Vec::new());
        let topic = collaboration_topic(&profile);
        let carrier = FakeCarrier::new([
            join_ok(&topic),
            peek_ok(7, 7, Vec::new()),
            ack_ok(7, 7, false),
            peek_ok(7, 7, Vec::new()),
            ack_ok(7, 7, false),
        ]);
        let joined = join_collaboration_network(carrier, &profile).await.unwrap();

        let first = joined.peek().await.unwrap();
        assert!(first.envelopes().is_empty());
        assert_eq!((first.cursor, first.next_cursor), (7, 7));
        joined.ack(&first).await.unwrap();
        let second = joined.peek().await.unwrap();
        assert_eq!((second.cursor, second.next_cursor), (7, 7));
        joined.ack(&second).await.unwrap();
    }

    #[tokio::test]
    async fn collaboration_carrier_consumers_are_random_and_cross_ack_fails_pre_effect() {
        let (signing_key, _) = generate_keypair();
        let profile = verified_profile(&signing_key, "consumer-binding-test", 1, None, Vec::new());
        let topic = collaboration_topic(&profile);
        let first_carrier =
            FakeCarrier::new([join_ok(&topic), peek_ok(0, 1, vec![gossip_frame(b"one")])]);
        let second_carrier = FakeCarrier::new([join_ok(&topic)]);
        let first = join_collaboration_network(first_carrier, &profile)
            .await
            .unwrap();
        let second = join_collaboration_network(second_carrier.clone(), &profile)
            .await
            .unwrap();

        assert_ne!(first.consumer_id, second.consumer_id);
        for consumer_id in [&first.consumer_id, &second.consumer_id] {
            assert_eq!(consumer_id.len(), 32);
            assert!(consumer_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        }
        let foreign_batch = first.peek().await.unwrap();
        assert!(second.ack(&foreign_batch).await.is_err());
        assert_eq!(
            second_carrier.requests().len(),
            1,
            "cross-subscription ack must fail before calling Carrier"
        );
    }

    #[tokio::test]
    async fn collaboration_carrier_peek_counts_invalid_frames_without_metadata_authority() {
        let (signing_key, _) = generate_keypair();
        let profile = verified_profile(&signing_key, "frame-validation-test", 1, None, Vec::new());
        let topic = collaboration_topic(&profile);
        let envelope = br#"{"frame":"valid"}"#.to_vec();
        let oversized = "x".repeat(MAX_COLLABORATION_TRANSPORT_FRAME_BYTES + 1);
        let valid_signed = signed_frame(&envelope);
        let valid_frame = serde_json::json!({
            "content": std::str::from_utf8(&valid_signed).unwrap(),
            "sender_id": {"forged": true},
            "sender_nick": null,
            "signature": "untrusted",
            "ts": "untrusted",
            "nonce": [],
            "sender_session_id": {"untrusted": true},
        });
        let messages = vec![
            valid_frame,
            serde_json::json!({"content": ""}),
            serde_json::json!({
                "content": oversized,
            }),
            serde_json::json!({"sender_id": "missing-content"}),
        ];
        let carrier = FakeCarrier::new([
            join_ok(&topic),
            peek_ok(0, 4, messages.clone()),
            peek_ok(0, 4, messages),
        ]);
        let joined = join_collaboration_network(carrier, &profile).await.unwrap();

        for _ in 0..2 {
            let batch = joined.peek().await.unwrap();
            assert_eq!(batch.envelopes(), std::slice::from_ref(&valid_signed));
            assert_eq!(batch.rejected_frames(), 3);
            assert_eq!((batch.cursor, batch.next_cursor), (0, 4));
        }
    }

    #[tokio::test]
    async fn collaboration_carrier_peek_responses_fail_closed() {
        let (signing_key, _) = generate_keypair();
        let profile = verified_profile(&signing_key, "peek-failure-test", 1, None, Vec::new());
        let topic = collaboration_topic(&profile);
        let too_many = vec![gossip_frame(b"x"); COLLABORATION_RECEIVE_BATCH_SIZE + 1];
        let malformed = [
            serde_json::json!({
                "status":"ok", "data":{
                    "cursor":0, "next_cursor":0, "messages":[], "scanned":0,
                    "limit":COLLABORATION_RECEIVE_BATCH_SIZE, "unexpected":true
                }
            }),
            serde_json::json!({
                "status":"ok", "data":{
                    "cursor":0, "next_cursor":0, "messages":[], "scanned":0,
                    "limit":COLLABORATION_RECEIVE_BATCH_SIZE - 1
                }
            }),
            serde_json::json!({
                "status":"ok", "data":{
                    "cursor":0, "next_cursor":0, "messages":[gossip_frame(b"x")], "scanned":0,
                    "limit":COLLABORATION_RECEIVE_BATCH_SIZE
                }
            }),
            serde_json::json!({
                "status":"ok", "data":{
                    "cursor":0, "next_cursor":COLLABORATION_RECEIVE_BATCH_SIZE + 1,
                    "messages":too_many, "scanned":COLLABORATION_RECEIVE_BATCH_SIZE + 1,
                    "limit":COLLABORATION_RECEIVE_BATCH_SIZE
                }
            }),
            serde_json::json!({
                "status":"ok", "data":{
                    "cursor":9, "next_cursor":8, "messages":[], "scanned":0,
                    "limit":COLLABORATION_RECEIVE_BATCH_SIZE
                }
            }),
            serde_json::json!({
                "status":"ok", "data":{
                    "cursor":u64::MAX, "next_cursor":u64::MAX,
                    "messages":[gossip_frame(b"x")], "scanned":1,
                    "limit":COLLABORATION_RECEIVE_BATCH_SIZE
                }
            }),
            serde_json::json!({
                "status":"ok", "unexpected":true, "data":{
                    "cursor":0, "next_cursor":0, "messages":[], "scanned":0,
                    "limit":COLLABORATION_RECEIVE_BATCH_SIZE
                }
            }),
            serde_json::json!({
                "status":"ok", "data":{
                    "cursor":0, "next_cursor":1,
                    "messages":[{"content":"x".repeat(MAX_RECEIVE_RESPONSE_BYTES)}], "scanned":1,
                    "limit":COLLABORATION_RECEIVE_BATCH_SIZE
                }
            }),
        ];

        for response in malformed {
            let carrier = FakeCarrier::new([join_ok(&topic), FakeReply::Value(response)]);
            let joined = join_collaboration_network(carrier.clone(), &profile)
                .await
                .unwrap();
            assert!(joined.peek().await.is_err());
            assert_eq!(carrier.requests().len(), 2);
            assert_eq!(carrier.requests()[1]["op"], "gossip_peek");
        }

        let carrier = FakeCarrier::new([join_ok(&topic), FakeReply::Error("peek failed")]);
        let joined = join_collaboration_network(carrier, &profile).await.unwrap();
        assert!(joined.peek().await.is_err());
    }

    #[tokio::test]
    async fn collaboration_carrier_ack_responses_fail_closed() {
        let (signing_key, _) = generate_keypair();
        let profile = verified_profile(&signing_key, "ack-failure-test", 1, None, Vec::new());
        let topic = collaboration_topic(&profile);
        let malformed = [
            serde_json::json!({
                "status":"ok", "data":{"cursor":1, "next_cursor":1, "advanced":true}
            }),
            serde_json::json!({
                "status":"ok", "data":{"cursor":0, "next_cursor":2, "advanced":true}
            }),
            serde_json::json!({
                "status":"ok", "data":{
                    "cursor":0, "next_cursor":1, "advanced":true, "unexpected":true
                }
            }),
            serde_json::json!({"status":"error", "data":{}}),
        ];

        for response in malformed {
            let carrier = FakeCarrier::new([
                join_ok(&topic),
                peek_ok(0, 1, vec![gossip_frame(b"x")]),
                FakeReply::Value(response),
            ]);
            let joined = join_collaboration_network(carrier, &profile).await.unwrap();
            let batch = joined.peek().await.unwrap();
            assert!(joined.ack(&batch).await.is_err());
        }

        let empty_advanced = FakeCarrier::new([
            join_ok(&topic),
            peek_ok(4, 4, Vec::new()),
            ack_ok(4, 4, true),
        ]);
        let joined = join_collaboration_network(empty_advanced, &profile)
            .await
            .unwrap();
        let batch = joined.peek().await.unwrap();
        assert!(joined.ack(&batch).await.is_err());

        let provider_error = FakeCarrier::new([
            join_ok(&topic),
            peek_ok(0, 1, vec![gossip_frame(b"x")]),
            FakeReply::Error("ack failed"),
        ]);
        let joined = join_collaboration_network(provider_error, &profile)
            .await
            .unwrap();
        let batch = joined.peek().await.unwrap();
        assert!(joined.ack(&batch).await.is_err());
    }
}
