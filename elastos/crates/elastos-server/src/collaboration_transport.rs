//! Bounded Runtime driver for one durable collaboration core and Carrier subscription.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use elastos_runtime::provider::{
    Provider, ProviderCarrierRoute, ProviderError, ProviderInvocation, ProviderInvocationTransport,
    ProviderRegistry, ProviderTransfer, ResourceRequest, ResourceResponse,
};
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use crate::collaboration_carrier::{CollaborationCarrierSendOutcome, JoinedCollaborationNetwork};
use crate::collaboration_core::{CollaborationCore, CollaborationTransportIngestion};
use crate::collaboration_presence::CollaborationPresenceProductPort;
use crate::collaboration_protocol::{
    authenticated_carrier_source_endpoint, MAX_COLLABORATION_TRANSPORT_FRAME_BYTES,
};

pub(crate) struct CollaborationTransportDriver {
    core: Arc<CollaborationCore>,
    network: JoinedCollaborationNetwork,
    home_delivery: Option<SharedHomeDelivery>,
    gossip: Mutex<GossipSchedule>,
}

const SHARED_FAST_RETRY_WINDOW_SECS: u64 = 3;
const SHARED_CUSTODY_FAST_RETRY: Duration = Duration::from_millis(500);
const SHARED_CUSTODY_RETRY: Duration = Duration::from_secs(5);
const MAX_CUSTODY_RETRIES_PER_CYCLE: usize = 4;
pub(crate) const SHARED_PROVIDER: &str = "collaboration.shared";
const SHARED_OP: &str = "deliver";
const MAX_SHARED_DELIVERY_BYTES: usize = MAX_COLLABORATION_TRANSPORT_FRAME_BYTES * 6 + 2048;
// The Shared JSON payload bound plus Carrier's result envelope and newline.
pub(crate) const MAX_SHARED_CARRIER_REPLY_BYTES: usize = MAX_SHARED_DELIVERY_BYTES + 64;
const HOME_DELIVERY_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_SHARED_GOSSIP_RETRY: Duration = Duration::from_secs(60);

#[derive(Clone)]
struct SharedHomeDelivery {
    core: Arc<CollaborationCore>,
    presence: CollaborationPresenceProductPort,
    registry: Arc<ProviderRegistry>,
    attempts: Arc<Mutex<SharedRetrySchedule>>,
}

#[derive(Default)]
struct SharedRetrySchedule {
    attempts: HashMap<(String, String), Instant>,
    next: usize,
}

/// When each pending original last went out over gossip, and how often.
#[derive(Default)]
struct GossipSchedule {
    sent: HashMap<String, (Instant, u32)>,
}

/// Wait after `sends` gossip broadcasts of one original. Presence keeps the
/// slow cadence; a Shared post backs off 5, 10, 20, 40, then 60 s until a
/// Home receipt.
fn gossip_resend_interval(shared: bool, sends: u32) -> Duration {
    if !shared {
        return SHARED_CUSTODY_RETRY;
    }
    SHARED_CUSTODY_RETRY
        .saturating_mul(1 << sends.saturating_sub(1).min(4))
        .min(MAX_SHARED_GOSSIP_RETRY)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SharedDeliveryRequest {
    op: String,
    frame: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SharedDeliveryResponse {
    receipt: String,
}

#[async_trait::async_trait]
impl Provider for SharedHomeDelivery {
    async fn handle(&self, _: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "Shared delivery is Runtime-owned".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        Vec::new()
    }
    fn name(&self) -> &'static str {
        SHARED_PROVIDER
    }

    async fn send_raw(
        &self,
        value: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        self.receive(value, now_secs())
            .map_err(|_| ProviderError::Provider("Shared Home delivery was refused".to_string()))
    }
}

impl SharedHomeDelivery {
    fn receive(&self, value: &serde_json::Value, now: u64) -> anyhow::Result<serde_json::Value> {
        if serde_json::to_vec(value)?.len() > MAX_SHARED_DELIVERY_BYTES {
            anyhow::bail!("Shared delivery exceeds its byte bound");
        }
        let mut value = value.clone();
        let object = value
            .as_object_mut()
            .context("Shared delivery must be an object")?;
        let invocation = object
            .remove("_runtime_invocation")
            .context("Shared invocation is missing")?;
        let invocation = invocation
            .as_object()
            .context("Shared invocation is invalid")?;
        let capability = format!("provider:{SHARED_PROVIDER}->{SHARED_PROVIDER}:{SHARED_OP}");
        for (field, expected) in [
            ("schema", "elastos.provider.invocation/v1"),
            ("source", SHARED_PROVIDER),
            ("target", SHARED_PROVIDER),
            ("op", SHARED_OP),
            ("capability", capability.as_str()),
            ("transport", "carrier-provider-plane"),
            ("transfer", "json"),
        ] {
            if invocation.get(field).and_then(serde_json::Value::as_str) != Some(expected) {
                anyhow::bail!("Shared invocation is invalid");
            }
        }
        let source = authenticated_carrier_source_endpoint(invocation.get("carrier"))?;
        let request: SharedDeliveryRequest = serde_json::from_value(value)?;
        if request.op != SHARED_OP {
            anyhow::bail!("Shared operation is invalid");
        }
        let receipt = self
            .core
            .accept_home_chat(request.frame.as_bytes(), &source, now)?;
        Ok(serde_json::to_value(SharedDeliveryResponse {
            receipt: String::from_utf8(receipt)?,
        })?)
    }

    async fn retry_once(&self, now: u64) -> anyhow::Result<CollaborationOutgoingRetrySummary> {
        let pending = self
            .core
            .pending_outgoing(now)?
            .into_iter()
            .filter(|message| message.shared_chat())
            .collect::<Vec<_>>();
        let peers = self.presence.history_participants(now)?;
        let local = self.core.local_device_did();
        if self.core.is_bootstrap_endpoint(&local) {
            return Ok(CollaborationOutgoingRetrySummary::default());
        }
        let peers = peers
            .into_iter()
            .filter(|peer| {
                peer.endpoint_did != local && !self.core.is_bootstrap_endpoint(&peer.endpoint_did)
            })
            .collect::<Vec<_>>();
        let mut plan = Vec::new();
        let attempted_at = Instant::now();
        {
            let mut schedule = self
                .attempts
                .lock()
                .map_err(|_| anyhow::anyhow!("Shared retry schedule lock poisoned"))?;
            let live_messages = pending
                .iter()
                .map(|message| message.envelope_sha256())
                .collect::<HashSet<_>>();
            let live_peers = peers
                .iter()
                .map(|peer| peer.endpoint_did.as_str())
                .collect::<HashSet<_>>();
            schedule.attempts.retain(|(hash, peer), _| {
                live_messages.contains(hash.as_str()) && live_peers.contains(peer.as_str())
            });
            let slots = pending.len() * peers.len();
            if slots == 0 {
                schedule.next = 0;
            }
            for _ in 0..slots {
                let index = schedule.next % slots;
                schedule.next = (index + 1) % slots;
                let message = &pending[index / peers.len()];
                let peer = &peers[index % peers.len()].endpoint_did;
                if message.accepted_by(peer) {
                    continue;
                }
                let key = (message.envelope_sha256().to_string(), peer.clone());
                let interval =
                    if now.saturating_sub(message.created_at()) < SHARED_FAST_RETRY_WINDOW_SECS {
                        SHARED_CUSTODY_FAST_RETRY
                    } else {
                        SHARED_CUSTODY_RETRY
                    };
                if schedule
                    .attempts
                    .get(&key)
                    .is_some_and(|last| attempted_at.duration_since(*last) < interval)
                {
                    continue;
                }
                schedule.attempts.insert(key.clone(), attempted_at);
                plan.push((key, message));
                if plan.len() == MAX_CUSTODY_RETRIES_PER_CYCLE {
                    break;
                }
            }
        }
        let mut summary = CollaborationOutgoingRetrySummary::default();
        // Every call has an independent deadline. The original is idempotent:
        // a lost reply leaves it pending and a retry returns its durable receipt.
        let mut deliveries = tokio::task::JoinSet::new();
        for ((message_hash, peer), message) in plan {
            self.core.community_membership().require_joined()?;
            summary.attempted += 1;
            let service = self.clone();
            let frame = String::from_utf8(
                self.core
                    .prepare_transport_frame(message.envelope_bytes())?,
            )?;
            deliveries.spawn(async move {
                let response = tokio::time::timeout(
                    HOME_DELIVERY_TIMEOUT,
                    service.registry.invoke_provider(ProviderInvocation {
                        source: SHARED_PROVIDER.to_string(),
                        target: SHARED_PROVIDER.to_string(),
                        op: SHARED_OP.to_string(),
                        request: serde_json::to_value(SharedDeliveryRequest {
                            op: SHARED_OP.to_string(),
                            frame,
                        })?,
                        transfer: ProviderTransfer::Json,
                        range: None,
                        progress: None,
                        transport: ProviderInvocationTransport::Carrier(
                            ProviderCarrierRoute::PeerDid {
                                peer_did: peer.clone(),
                                timeout_ms: Some(HOME_DELIVERY_TIMEOUT.as_millis() as u64),
                            },
                        ),
                    }),
                )
                .await
                .context("Shared Home delivery timed out")??;
                let mut response = response;
                if let Some(object) = response.as_object_mut() {
                    object.remove("_runtime_transfer");
                }
                if serde_json::to_vec(&response)?.len() > MAX_SHARED_DELIVERY_BYTES {
                    anyhow::bail!("Shared receipt exceeds its byte bound");
                }
                let response: SharedDeliveryResponse = serde_json::from_value(response)?;
                service.core.record_home_chat_acceptance(
                    response.receipt.as_bytes(),
                    &message_hash,
                    &peer,
                    now,
                )?;
                Ok::<(), anyhow::Error>(())
            });
        }
        while let Some(result) = deliveries.join_next().await {
            if matches!(result, Ok(Ok(()))) {
                summary.remote_broadcasts += 1;
            } else {
                summary.send_failures += 1;
            }
        }
        Ok(summary)
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CollaborationOutgoingRetrySummary {
    pub(crate) attempted: usize,
    pub(crate) remote_broadcasts: usize,
    pub(crate) local_only_buffered: usize,
    pub(crate) send_failures: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CollaborationIncomingOnceSummary {
    pub(crate) carrier_rejected_frames: usize,
    pub(crate) deterministic_rejections: usize,
    pub(crate) incoming_acceptances: usize,
    pub(crate) remote_acceptances: usize,
    pub(crate) acceptance_receipt_broadcasts: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CollaborationIncomingOnceOutcome {
    Acknowledged(CollaborationIncomingOnceSummary),
    RetryRequired(CollaborationIncomingOnceSummary),
}

impl CollaborationTransportDriver {
    pub(crate) fn new(core: Arc<CollaborationCore>, network: JoinedCollaborationNetwork) -> Self {
        Self {
            core,
            network,
            home_delivery: None,
            gossip: Mutex::new(GossipSchedule::default()),
        }
    }

    pub(crate) async fn with_home_delivery(
        mut self,
        presence: CollaborationPresenceProductPort,
        registry: Arc<ProviderRegistry>,
    ) -> anyhow::Result<Self> {
        let delivery = SharedHomeDelivery {
            core: self.core.clone(),
            presence,
            registry: registry.clone(),
            attempts: Arc::new(Mutex::new(SharedRetrySchedule::default())),
        };
        registry
            .register_runtime_provider_target(SHARED_PROVIDER, Arc::new(delivery.clone()))
            .await?;
        self.home_delivery = Some(delivery);
        Ok(self)
    }

    pub(crate) async fn has_remote_peers(&self) -> anyhow::Result<bool> {
        self.network.has_remote_peers().await
    }

    pub(crate) async fn restore_missing_bootstrap_peers(&self) -> anyhow::Result<()> {
        self.network.restore_missing_bootstrap_peers().await
    }

    pub(crate) fn community_link(
        &self,
    ) -> &crate::collaboration_carrier::JoinedCollaborationNetwork {
        &self.network
    }

    pub(crate) async fn retry_outgoing_once(
        &self,
        now: u64,
    ) -> anyhow::Result<CollaborationOutgoingRetrySummary> {
        let gossip = async {
            match tokio::time::timeout(SHARED_CUSTODY_FAST_RETRY, self.retry_gossip_once(now)).await
            {
                Ok(result) => result,
                Err(_) => Ok(CollaborationOutgoingRetrySummary {
                    send_failures: 1,
                    ..CollaborationOutgoingRetrySummary::default()
                }),
            }
        };
        let homes = async {
            match &self.home_delivery {
                Some(delivery) if self.core.community_membership().joined() => {
                    delivery.retry_once(now).await
                }
                _ => Ok(CollaborationOutgoingRetrySummary::default()),
            }
        };
        // A gossip response cannot hold a Home attempt back. Both
        // branches remain owned by this existing worker future and cancel with it.
        let (gossip, homes) = tokio::join!(gossip, homes);
        let mut summary = gossip?;
        let homes = homes?;
        summary.attempted += homes.attempted;
        summary.remote_broadcasts += homes.remote_broadcasts;
        summary.send_failures += homes.send_failures;
        Ok(summary)
    }

    /// Broadcasts each pending original at once, then resends it until a
    /// receipt settles it: Presence every 5 s, a Shared post with back-off
    /// (`gossip_resend_interval`). Gossip reaches Homes that no presence or
    /// Discovery lists and no direct dial reaches. A bootstrap node only
    /// forwards and sends nothing.
    async fn retry_gossip_once(
        &self,
        now: u64,
    ) -> anyhow::Result<CollaborationOutgoingRetrySummary> {
        if !self.core.community_membership().joined()
            || self
                .core
                .is_bootstrap_endpoint(&self.core.local_device_did())
        {
            return Ok(CollaborationOutgoingRetrySummary::default());
        }
        let attempted_at = Instant::now();
        let selected = {
            let mut schedule = self
                .gossip
                .lock()
                .map_err(|_| anyhow::anyhow!("gossip schedule lock poisoned"))?;
            // A Shared post leaves gossip at its first Home receipt; direct
            // delivery keeps it pending for the other Homes.
            let live = self
                .core
                .pending_outgoing(now)?
                .into_iter()
                .filter(|message| !message.shared_chat() || !message.remote_acceptance_recorded())
                .collect::<Vec<_>>();
            let hashes = live
                .iter()
                .map(|message| message.envelope_sha256())
                .collect::<HashSet<_>>();
            schedule
                .sent
                .retain(|hash, _| hashes.contains(hash.as_str()));
            let mut shared = 0;
            live.into_iter()
                .filter(|message| {
                    if message.shared_chat() && shared == MAX_CUSTODY_RETRIES_PER_CYCLE {
                        return false;
                    }
                    let due =
                        schedule
                            .sent
                            .get(message.envelope_sha256())
                            .is_none_or(|(last, sends)| {
                                attempted_at.duration_since(*last)
                                    >= gossip_resend_interval(message.shared_chat(), *sends)
                            });
                    shared += usize::from(due && message.shared_chat());
                    due
                })
                .collect::<Vec<_>>()
        };
        let mut summary = CollaborationOutgoingRetrySummary::default();
        for outgoing in selected {
            self.core.community_membership().require_joined()?;
            // Every own-presence send presents the grant to the live manager.
            // A driver without the Runtime's port has no authority to send it.
            if let Some((principal_id, profile_did)) = outgoing.presence_sender() {
                let Some(delivery) = &self.home_delivery else {
                    self.core.retire_outgoing_presence(principal_id)?;
                    continue;
                };
                if !delivery
                    .presence
                    .own_presence_effect_usable(outgoing.envelope_sha256(), profile_did, now)
                    .await?
                {
                    continue;
                }
            }
            // Recorded per send, so a cycle cut short leaves the rest due.
            {
                let mut schedule = self
                    .gossip
                    .lock()
                    .map_err(|_| anyhow::anyhow!("gossip schedule lock poisoned"))?;
                let entry = schedule
                    .sent
                    .entry(outgoing.envelope_sha256().to_string())
                    .or_insert((attempted_at, 0));
                *entry = (attempted_at, entry.1.saturating_add(1));
            }
            let frame = self
                .core
                .prepare_transport_frame(outgoing.envelope_bytes())?;
            // No await separates this queue admission from Carrier dispatch.
            // Revocation can complete while an earlier selected send waits.
            if outgoing.presence_sender().is_some()
                && !self
                    .core
                    .pending_outgoing_contains(outgoing.envelope_sha256(), now)?
            {
                continue;
            }
            summary.attempted += 1;
            match self.network.send_gossip(&self.core, &frame, None).await {
                Ok(CollaborationCarrierSendOutcome::RemoteBroadcast { .. }) => {
                    summary.remote_broadcasts += 1;
                }
                Ok(CollaborationCarrierSendOutcome::LocalOnlyBuffered) => {
                    summary.local_only_buffered += 1;
                }
                Err(_) => {
                    summary.send_failures += 1;
                }
            }
        }
        Ok(summary)
    }

    pub(crate) async fn process_incoming_once(
        &self,
        now: u64,
    ) -> anyhow::Result<CollaborationIncomingOnceOutcome> {
        let mut summary = CollaborationIncomingOnceSummary::default();
        if !self.core.community_membership().joined() {
            return Ok(CollaborationIncomingOnceOutcome::Acknowledged(summary));
        }

        // Frames this Home refused under a per-sender limit come first. Their
        // senders may never resend them, so this Home owns the retry.
        let mut held = self.core.take_held_frames(now).into_iter();
        while let Some(frame) = held.next() {
            if !self
                .ingest_frame(frame.frame(), now, &mut summary, true)
                .await
            {
                self.core
                    .restore_held_frames(std::iter::once(frame).chain(held).collect());
                return Ok(CollaborationIncomingOnceOutcome::RetryRequired(summary));
            }
            self.core.complete_held_frame(frame);
        }

        let batch = self.network.peek().await?;
        if !self.core.community_membership().joined() {
            return Ok(CollaborationIncomingOnceOutcome::RetryRequired(summary));
        }
        summary.carrier_rejected_frames = batch.rejected_frames();
        for envelope in batch.envelopes() {
            if !self.core.community_membership().joined() {
                return Ok(CollaborationIncomingOnceOutcome::RetryRequired(summary));
            }
            if !self.ingest_frame(envelope, now, &mut summary, false).await {
                return Ok(CollaborationIncomingOnceOutcome::RetryRequired(summary));
            }
        }

        match self.network.ack(&batch).await {
            Ok(()) => Ok(CollaborationIncomingOnceOutcome::Acknowledged(summary)),
            Err(_) => Ok(CollaborationIncomingOnceOutcome::RetryRequired(summary)),
        }
    }

    /// Ingests one frame and sends its receipt. Returns false when the cycle
    /// must stop and retry this frame later.
    async fn ingest_frame(
        &self,
        frame: &[u8],
        now: u64,
        summary: &mut CollaborationIncomingOnceSummary,
        held: bool,
    ) -> bool {
        match self.core.allows_gossip_frame(frame) {
            Ok(true) => {}
            Ok(false) => {
                summary.deterministic_rejections += 1;
                return true;
            }
            Err(_) => return false,
        }
        match self.core.ingest_transport_frame(frame, now) {
            Err(_) => false,
            Ok(CollaborationTransportIngestion::Rejected(_)) => {
                summary.deterministic_rejections += 1;
                true
            }
            Ok(CollaborationTransportIngestion::RemoteAcceptance(_)) => {
                summary.remote_acceptances += 1;
                true
            }
            Ok(CollaborationTransportIngestion::Incoming(accepted)) => {
                summary.incoming_acceptances += 1;
                let Ok(receipt_frame) = self
                    .core
                    .prepare_transport_frame(accepted.acceptance_receipt_bytes())
                else {
                    return false;
                };
                match self
                    .network
                    .send_gossip(&self.core, &receipt_frame, Some(frame))
                    .await
                {
                    Ok(CollaborationCarrierSendOutcome::RemoteBroadcast { .. }) => {
                        summary.acceptance_receipt_broadcasts += 1;
                        true
                    }
                    // A held frame's senders fetch its durable receipt again
                    // by retrying: direct by the Home call, gossip by re-broadcast.
                    Ok(CollaborationCarrierSendOutcome::LocalOnlyBuffered) | Err(_) => held,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collaboration_carrier::join_collaboration_network;
    use crate::collaboration_core::WriteFault;
    use crate::collaboration_default_conversation::{
        canonical_default_conversation_grant_bytes, verify_default_conversation_grant,
        DefaultConversationAdmissionPolicy, DefaultConversationGrant,
        VerifiedDefaultConversationGrant, DEFAULT_CONVERSATION_GRANT_SCHEMA_V1,
    };
    use crate::collaboration_device_authority::DefaultConversationDeviceAuthority;
    use crate::collaboration_network::{
        canonical_collaboration_network_profile_payload_bytes,
        validate_collaboration_network_profile, CollaborationNetworkProfile,
        CollaborationNetworkProfileMode, DefaultConversationGrantDescriptor,
        SignedCollaborationNetworkProfile, VerifiedCollaborationNetworkProfile,
        COLLABORATION_NETWORK_PROFILE_SCHEMA, COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
    };
    use crate::collaboration_presence_grant::PresenceDecision;
    use crate::collaboration_protocol::sign_collaboration_transport_frame;
    use crate::esp_binding::{esp_request_binding, EspRequestBinding};
    use elastos_common::collaboration_protocol::{
        canonical_collaboration_message_bytes, canonical_signed_collaboration_message_bytes,
        collaboration_message_envelope_sha256, SignedCollaborationMessage,
        COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
    };
    use elastos_runtime::provider::ProviderCarrierInvoker;
    use elastos_runtime::signature::{generate_keypair, SigningKey};
    use sha2::{Digest, Sha256};
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    const NETWORK: &str = "collaboration-transport-test";
    const CONVERSATION: &str = "default-conversation";
    const SERVICE: &str = "chat";
    const OPERATION_CAPSULE: &str = "chat-room";
    const NOW: u64 = 1_800_000_000;
    const TTL: u64 = 300;

    struct FakeCarrier {
        requests: Mutex<Vec<serde_json::Value>>,
        replies: Mutex<VecDeque<Result<serde_json::Value, ProviderError>>>,
        blocked_metadata: AtomicBool,
        send_barrier: Mutex<Option<Arc<tokio::sync::Barrier>>>,
        durable_incoming_before_send: Mutex<Option<Arc<CollaborationCore>>>,
    }
    impl FakeCarrier {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                requests: Mutex::new(Vec::new()),
                replies: Mutex::new(VecDeque::new()),
                blocked_metadata: AtomicBool::new(false),
                send_barrier: Mutex::new(None),
                durable_incoming_before_send: Mutex::new(None),
            })
        }
        fn push(&self, replies: impl IntoIterator<Item = serde_json::Value>) {
            self.replies
                .lock()
                .unwrap()
                .extend(replies.into_iter().map(Ok));
        }
        fn push_failure(&self, message: &str) {
            self.replies
                .lock()
                .unwrap()
                .push_back(Err(ProviderError::Unavailable(message.to_string())));
        }
        fn observe_durable_incoming_before_send(&self, core: Arc<CollaborationCore>) {
            *self.durable_incoming_before_send.lock().unwrap() = Some(core);
        }
        fn requests(&self) -> Vec<serde_json::Value> {
            self.requests.lock().unwrap().clone()
        }
    }
    #[async_trait::async_trait]
    impl Provider for FakeCarrier {
        async fn handle(&self, _: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
            unreachable!()
        }
        fn schemes(&self) -> Vec<&'static str> {
            Vec::new()
        }
        fn name(&self) -> &'static str {
            "test-carrier"
        }
        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            self.requests.lock().unwrap().push(request.clone());
            if request["op"] == "gossip_send" {
                if let Some(core) = self.durable_incoming_before_send.lock().unwrap().as_ref() {
                    let summary = core.summary().unwrap();
                    assert_eq!(
                        summary.pending_product_handoffs + summary.replay_tombstones,
                        1,
                        "the receipt is sent only after acceptance is durable"
                    );
                }
            }
            let barrier = if request["op"] == "gossip_send" {
                self.send_barrier.lock().unwrap().take()
            } else {
                None
            };
            if let Some(barrier) = barrier {
                barrier.wait().await;
                barrier.wait().await;
            }
            if request["op"] == "gossip_send" && self.blocked_metadata.load(Ordering::SeqCst) {
                return std::future::pending().await;
            }
            if request["op"] == "gossip_join_exact" {
                return Ok(serde_json::json!({"status":"ok","data":{"topic":request["topic"]}}));
            }
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(ProviderError::Unavailable("offline".to_string())))
        }
    }
    fn peek(cursor: u64, next: u64, frames: Vec<serde_json::Value>) -> serde_json::Value {
        serde_json::json!({"status":"ok", "data":{
            "scanned":frames.len(), "messages":frames, "limit":32,
            "cursor":cursor, "next_cursor":next,
        }})
    }

    fn ack(cursor: u64, next: u64) -> serde_json::Value {
        serde_json::json!({"status":"ok", "data":{
            "cursor":cursor, "next_cursor":next, "advanced":cursor != next,
        }})
    }

    fn gossip(frame: &[u8]) -> serde_json::Value {
        serde_json::json!({"content":std::str::from_utf8(frame).unwrap()})
    }

    fn request_ops(carrier: &FakeCarrier) -> Vec<String> {
        carrier
            .requests()
            .iter()
            .map(|request| request["op"].as_str().unwrap().to_string())
            .collect()
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        data_root: std::path::PathBuf,
        profile: VerifiedCollaborationNetworkProfile,
        grant: VerifiedDefaultConversationGrant,
        device_key: SigningKey,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let data_root = temp.path().join("data");
            std::fs::create_dir(&data_root).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&data_root, std::fs::Permissions::from_mode(0o700))
                    .unwrap();
            }
            let grant_bytes =
                canonical_default_conversation_grant_bytes(&DefaultConversationGrant {
                    schema: DEFAULT_CONVERSATION_GRANT_SCHEMA_V1.to_string(),
                    network_id: NETWORK.to_string(),
                    conversation_id: CONVERSATION.to_string(),
                    sender_service: SERVICE.to_string(),
                    admission_policy: DefaultConversationAdmissionPolicy::ProfileScopedSigner,
                })
                .unwrap();
            let (profile_signer, _) = generate_keypair();
            let profile =
                verified_profile(&profile_signer, raw_sha256_cid(&grant_bytes), Vec::new());
            let grant = verify_default_conversation_grant(&profile, &grant_bytes).unwrap();
            let (device_key, _) = generate_keypair();
            Self {
                _temp: temp,
                data_root,
                profile,
                grant,
                device_key,
            }
        }

        fn core(&self) -> Arc<CollaborationCore> {
            Arc::new(
                CollaborationCore::new(
                    &self.data_root,
                    self.device_key.clone(),
                    self.profile.clone(),
                    self.grant.clone(),
                    OPERATION_CAPSULE,
                )
                .unwrap(),
            )
        }

        fn authority(&self, key: SigningKey) -> DefaultConversationDeviceAuthority {
            DefaultConversationDeviceAuthority::new(key, self.profile.clone(), self.grant.clone())
                .unwrap()
        }

        async fn driver(
            &self,
            core: Arc<CollaborationCore>,
            carrier: Arc<dyn Provider>,
        ) -> CollaborationTransportDriver {
            let joined = join_collaboration_network(carrier, &self.profile)
                .await
                .unwrap();
            CollaborationTransportDriver::new(core, joined)
        }
    }

    fn raw_sha256_cid(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        let multihash = cid::multihash::Multihash::<64>::wrap(0x12, digest.as_slice()).unwrap();
        cid::Cid::new_v1(0x55, multihash).to_string()
    }

    fn verified_profile(
        signing_key: &SigningKey,
        grant_cid: String,
        bootstrap_peers: Vec<crate::collaboration_network::CollaborationBootstrapPeer>,
    ) -> VerifiedCollaborationNetworkProfile {
        let signer_did = crate::crypto::encode_signing_key_did(signing_key);
        let payload = CollaborationNetworkProfile {
            schema: COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
            network_id: NETWORK.to_string(),
            revision: 1,
            previous_profile_sha256: None,
            signer_did: signer_did.clone(),
            bootstrap_peers,
            default_conversation: Some(DefaultConversationGrantDescriptor { grant_cid }),
        };
        let payload_bytes =
            canonical_collaboration_network_profile_payload_bytes(&payload).unwrap();
        let (signature, envelope_signer) = crate::crypto::domain_separated_sign(
            signing_key,
            COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
            &payload_bytes,
        );
        let bytes = serde_json::to_vec(
            &serde_json::to_value(SignedCollaborationNetworkProfile {
                payload,
                signature,
                signer_did: envelope_signer,
            })
            .unwrap(),
        )
        .unwrap();
        match validate_collaboration_network_profile(Some(&bytes), NETWORK, &[signer_did], None)
            .unwrap()
        {
            CollaborationNetworkProfileMode::Configured(profile) => profile,
            CollaborationNetworkProfileMode::Isolated => panic!("expected configured profile"),
        }
    }

    fn operation(
        profile: &crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
        request_id: &str,
        payload_type: &str,
        payload: &serde_json::Value,
        ttl_secs: u64,
    ) -> EspRequestBinding {
        let payload =
            crate::collaboration_default_conversation::profile_authenticated_conversation_payload(
                profile,
                payload.clone(),
            )
            .unwrap();
        let intent = serde_json::json!({
            "payload_type": payload_type,
            "payload": payload,
            "ttl_secs": ttl_secs,
        });
        esp_request_binding(
            request_id,
            "runtime-principal",
            OPERATION_CAPSULE,
            Some("elastos.chat.room"),
            "message.send",
            ["elastos://chat/message".to_string()],
            &intent,
        )
    }

    fn prepare_outgoing(
        core: &CollaborationCore,
        request_id: &str,
        payload: serde_json::Value,
    ) -> crate::collaboration_core::DurableOutgoingMessage {
        let payload_type = "elastos.chat.message/v1";
        let (profile_key, _) = generate_keypair();
        let profile = crate::collaboration_profile_authority::signed_profile_document_for_test(
            &profile_key,
            "Local Profile",
            None,
            1,
            None,
            NOW,
            vec![core.test_local_device_did()],
        )
        .unwrap();
        core.prepare_profile_outgoing(
            operation(&profile, request_id, payload_type, &payload, TTL),
            &profile,
            payload_type,
            payload,
            NOW,
            TTL,
        )
        .unwrap()
    }

    fn remote_message(fixture: &Fixture, key: SigningKey, content: &str) -> (SigningKey, Vec<u8>) {
        let authority = fixture.authority(key.clone());
        let prepared = authority
            .prepare_outgoing(
                SERVICE,
                "elastos.chat.message/v1",
                serde_json::json!({"body": content}),
                NOW,
                TTL,
            )
            .unwrap();
        (key, prepared.envelope_bytes().to_vec())
    }

    fn prepare_metadata(
        fixture: &Fixture,
        core: &CollaborationCore,
        request_id: &str,
    ) -> crate::collaboration_core::DurableOutgoingMessage {
        let profile = fixture
            .authority(fixture.device_key.clone())
            .sender_profile_for_test()
            .unwrap();
        let kind = crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE;
        core.prepare_profile_outgoing(
            operation(&profile, request_id, kind, &serde_json::json!({}), 45),
            &profile,
            kind,
            serde_json::json!({}),
            NOW,
            45,
        )
        .unwrap()
    }

    const PRESENCE_PRINCIPAL: &str = "runtime-principal";

    /// A presence port whose person has granted "Always", as on a Home.
    async fn granted_presence_port(
        fixture: &Fixture,
        core: Arc<CollaborationCore>,
    ) -> (
        CollaborationPresenceProductPort,
        Arc<crate::collaboration_presence_grant::PresenceAuthority>,
    ) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fixture.data_root, std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        crate::auth::store_test_principal_root_protection(&fixture.data_root, PRESENCE_PRINCIPAL);
        let mut seed = fixture.device_key.to_bytes();
        seed[0] ^= 0xa5;
        seed[31] ^= 0x5a;
        let key = SigningKey::from_bytes(&seed);
        let profile = fixture
            .authority(fixture.device_key.clone())
            .sender_profile_for_test()
            .unwrap();
        crate::collaboration_presence_grant::store_test_profile(
            &fixture.data_root,
            PRESENCE_PRINCIPAL,
            &key,
            &profile,
        );
        let authority = crate::collaboration_presence_grant::test_authority();
        let port = CollaborationPresenceProductPort::new(core)
            .unwrap()
            .with_presence_authority(authority.clone());
        set_presence(fixture, &port, &authority, PresenceDecision::Always).await;
        (port, authority)
    }

    async fn granted_presence_driver(
        fixture: &Fixture,
        core: Arc<CollaborationCore>,
        carrier: Arc<dyn Provider>,
    ) -> (
        CollaborationTransportDriver,
        CollaborationPresenceProductPort,
        Arc<crate::collaboration_presence_grant::PresenceAuthority>,
    ) {
        let (port, authority) = granted_presence_port(fixture, core.clone()).await;
        let driver = fixture
            .driver(core, carrier)
            .await
            .with_home_delivery(port.clone(), Arc::new(ProviderRegistry::new()))
            .await
            .unwrap();
        (driver, port, authority)
    }

    async fn set_presence(
        fixture: &Fixture,
        port: &CollaborationPresenceProductPort,
        authority: &crate::collaboration_presence_grant::PresenceAuthority,
        decision: PresenceDecision,
    ) {
        let profile = fixture
            .authority(fixture.device_key.clone())
            .sender_profile_for_test()
            .unwrap();
        crate::collaboration_presence_grant::set_presence_grant(
            authority,
            &fixture.data_root,
            PRESENCE_PRINCIPAL,
            &profile.document().profile_did,
            &port.presence_scope(),
            decision,
            NOW,
        )
        .await
        .unwrap();
    }

    async fn queued_presence(
        fixture: &Fixture,
        port: &CollaborationPresenceProductPort,
        request_id: &str,
    ) -> crate::collaboration_core::DurableOutgoingMessage {
        let profile = fixture
            .authority(fixture.device_key.clone())
            .sender_profile_for_test()
            .unwrap();
        let grant = crate::collaboration_presence_grant::presence_grant(
            port.presence_authority().unwrap(),
            &fixture.data_root,
            PRESENCE_PRINCIPAL,
            &profile.document().profile_did,
            &port.presence_scope(),
            crate::collaboration_presence_grant::PresencePublisher::Runtime,
            NOW,
        )
        .await
        .unwrap();
        let presence = port
            .test_core()
            .prepare_profile_outgoing(
                crate::collaboration_presence::presence_request_binding(
                    &grant.bind_request_id(request_id),
                    PRESENCE_PRINCIPAL,
                    &profile,
                )
                .unwrap(),
                &profile,
                crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE,
                serde_json::json!({}),
                NOW,
                45,
            )
            .unwrap();
        port.test_core()
            .acknowledge_outgoing_product_projection(presence.envelope_sha256())
            .unwrap();
        presence
    }

    #[tokio::test]
    async fn stale_profile_effect_preserves_the_replacement_profile_presence() {
        use crate::collaboration_presence_grant::{
            presence_grant, set_presence_grant, PresencePublisher,
        };
        let fixture = Fixture::new();
        let core = fixture.core();
        let carrier = FakeCarrier::new();
        let (driver, port, authority) =
            granted_presence_driver(&fixture, core.clone(), carrier.clone()).await;
        let old_profile = fixture
            .authority(fixture.device_key.clone())
            .sender_profile_for_test()
            .unwrap();
        let old = queued_presence(&fixture, &port, "old-profile-effect").await;

        // A selected old effect can outlive a Profile/grant replacement. The
        // new Profile has its own signed authority, grant and queued projection.
        let (key, _) = generate_keypair();
        let replacement = crate::collaboration_profile_authority::signed_profile_document_for_test(
            &key,
            "Replacement Profile",
            None,
            1,
            None,
            NOW,
            vec![core.local_device_did()],
        )
        .unwrap();
        crate::collaboration_presence_grant::store_test_profile(
            &fixture.data_root,
            PRESENCE_PRINCIPAL,
            &key,
            &replacement,
        );
        set_presence_grant(
            &authority,
            &fixture.data_root,
            PRESENCE_PRINCIPAL,
            &replacement.document().profile_did,
            &port.presence_scope(),
            PresenceDecision::Always,
            NOW,
        )
        .await
        .unwrap();
        let grant = presence_grant(
            &authority,
            &fixture.data_root,
            PRESENCE_PRINCIPAL,
            &replacement.document().profile_did,
            &port.presence_scope(),
            PresencePublisher::Runtime,
            NOW,
        )
        .await
        .unwrap();
        let fresh = port
            .prepare_presence(
                crate::collaboration_presence::presence_request_binding(
                    &grant.bind_request_id("replacement-profile-effect"),
                    PRESENCE_PRINCIPAL,
                    &replacement,
                )
                .unwrap(),
                &replacement,
                NOW,
            )
            .unwrap();
        port.project_authorized_presence(&fresh, NOW).await.unwrap();

        assert!(
            port.own_presence_effect_usable(
                fresh.envelope_sha256(),
                &replacement.document().profile_did,
                NOW,
            )
            .await
            .unwrap(),
            "replacement Profile effect must be authorized before stale cleanup"
        );
        assert!(
            core.pending_outgoing_contains(fresh.envelope_sha256(), NOW)
                .unwrap(),
            "replacement Profile effect must be pending before stale cleanup"
        );
        assert!(!port
            .own_presence_effect_usable(
                old.envelope_sha256(),
                &old_profile.document().profile_did,
                NOW,
            )
            .await
            .unwrap());
        assert!(core
            .pending_outgoing_contains(fresh.envelope_sha256(), NOW)
            .unwrap());
        let snapshot = port.authorized_snapshot(NOW).await.unwrap();
        assert_eq!(snapshot.records().len(), 1);
        assert_eq!(
            snapshot.records()[0].sender_profile_did(),
            replacement.document().profile_did
        );
        let crate::collaboration_presence::OwnHistoryPresence::Usable(history) =
            port.own_history_presence(NOW).await.unwrap()
        else {
            panic!("replacement Profile must retain history authority")
        };
        assert_eq!(
            history.into_envelope().as_bytes(),
            fresh.test_envelope_bytes()
        );
        assert_eq!(driver.retry_gossip_once(NOW).await.unwrap().attempted, 1);
        let sent = carrier
            .requests()
            .into_iter()
            .filter(|request| request["op"] == "gossip_send")
            .collect::<Vec<_>>();
        assert_eq!(sent.len(), 1);
        let frame = crate::collaboration_protocol::verify_collaboration_transport_frame(
            sent[0]["message"].as_str().unwrap().as_bytes(),
        )
        .unwrap();
        assert_eq!(frame.envelope_bytes(), fresh.test_envelope_bytes());

        // A real Hidden decision still retires all current work, even when the
        // maintenance caller carries the previous Profile identity.
        set_presence_grant(
            &authority,
            &fixture.data_root,
            PRESENCE_PRINCIPAL,
            &replacement.document().profile_did,
            &port.presence_scope(),
            PresenceDecision::Hidden,
            NOW,
        )
        .await
        .unwrap();
        assert!(
            !port
                .retire_denied_current_presence(
                    PRESENCE_PRINCIPAL,
                    &old_profile.document().profile_did,
                    NOW,
                )
                .await
        );
        assert!(core.pending_outgoing(NOW).unwrap().is_empty());
        assert!(port
            .authorized_snapshot(NOW)
            .await
            .unwrap()
            .records()
            .is_empty());
        assert!(!matches!(
            port.own_history_presence(NOW).await.unwrap(),
            crate::collaboration_presence::OwnHistoryPresence::Usable(_)
        ));
        assert_eq!(driver.retry_gossip_once(NOW).await.unwrap().attempted, 0);
    }

    #[tokio::test]
    async fn selected_presence_keeps_its_original_grant_and_pending_operation() {
        use crate::collaboration_presence_grant::{
            presence_grant, set_presence_grant, PresencePublisher,
        };
        // A currently valid grant cannot authorize an older effect. Exercise
        // both publisher replacement and a fresh generation of the same mode.
        for same_mode in [false, true] {
            let fixture = Fixture::new();
            let core = fixture.core();
            let carrier = FakeCarrier::new();
            let (driver, port, authority) =
                granted_presence_driver(&fixture, core.clone(), carrier.clone()).await;
            let first = queued_presence(&fixture, &port, "first-person").await;
            let second = "person:local:second-presence";
            crate::auth::store_test_principal_root_protection(&fixture.data_root, second);
            let (key, _) = generate_keypair();
            let profile = crate::collaboration_profile_authority::signed_profile_document_for_test(
                &key,
                "Second person",
                None,
                1,
                None,
                NOW,
                vec![core.local_device_did()],
            )
            .unwrap();
            crate::collaboration_presence_grant::store_test_profile(
                &fixture.data_root,
                second,
                &key,
                &profile,
            );
            set_presence_grant(
                &authority,
                &fixture.data_root,
                second,
                &profile.document().profile_did,
                &port.presence_scope(),
                PresenceDecision::Always,
                NOW,
            )
            .await
            .unwrap();
            let old_grant = presence_grant(
                &authority,
                &fixture.data_root,
                second,
                &profile.document().profile_did,
                &port.presence_scope(),
                PresencePublisher::Runtime,
                NOW,
            )
            .await
            .unwrap();
            let old = port
                .prepare_presence(
                    crate::collaboration_presence::presence_request_binding(
                        &old_grant.bind_request_id("second-person"),
                        second,
                        &profile,
                    )
                    .unwrap(),
                    &profile,
                    NOW,
                )
                .unwrap();
            port.project_authorized_presence(&old, NOW).await.unwrap();
            let barrier = Arc::new(tokio::sync::Barrier::new(2));
            *carrier.send_barrier.lock().unwrap() = Some(barrier.clone());
            let change = async {
                barrier.wait().await; // The first send owns the selected batch.
                if same_mode {
                    set_presence_grant(
                        &authority,
                        &fixture.data_root,
                        second,
                        &profile.document().profile_did,
                        &port.presence_scope(),
                        PresenceDecision::Hidden,
                        NOW,
                    )
                    .await
                    .unwrap();
                }
                let decision = if same_mode {
                    PresenceDecision::Always
                } else {
                    PresenceDecision::WhileOpen
                };
                set_presence_grant(
                    &authority,
                    &fixture.data_root,
                    second,
                    &profile.document().profile_did,
                    &port.presence_scope(),
                    decision,
                    NOW,
                )
                .await
                .unwrap();
                port.retire_own_presence(second, &profile.document().profile_did)
                    .unwrap();
                let publisher = if same_mode {
                    PresencePublisher::Runtime
                } else {
                    PresencePublisher::CommunityOpen
                };
                assert!(
                    presence_grant(
                        &authority,
                        &fixture.data_root,
                        second,
                        &profile.document().profile_did,
                        &port.presence_scope(),
                        publisher,
                        NOW,
                    )
                    .await
                    .is_some(),
                    "the replacement grant itself is live"
                );
                // No Chat heartbeat runs during the switch. The while-open
                // token is valid, but there is no new operation to announce.
                barrier.wait().await;
            };
            let (result, ()) = tokio::join!(driver.retry_gossip_once(NOW), change);
            assert_eq!(result.unwrap().attempted, 1);
            let sent = carrier
                .requests()
                .into_iter()
                .filter(|r| r["op"] == "gossip_send")
                .collect::<Vec<_>>();
            assert_eq!(sent.len(), 1);
            let sent_frame = crate::collaboration_protocol::verify_collaboration_transport_frame(
                sent[0]["message"].as_str().unwrap().as_bytes(),
            )
            .unwrap();
            assert_eq!(sent_frame.envelope_bytes(), first.envelope_bytes());
            assert!(!core
                .pending_outgoing_contains(old.envelope_sha256(), NOW)
                .unwrap());
            // Even a stale preparation that lands after retirement keeps its
            // old binding and cannot be projected or dispatched.
            let stale = port
                .prepare_presence(
                    crate::collaboration_presence::presence_request_binding(
                        &old_grant.bind_request_id("late-old-preparation"),
                        second,
                        &profile,
                    )
                    .unwrap(),
                    &profile,
                    NOW,
                )
                .unwrap();
            core.acknowledge_outgoing_product_projection(stale.envelope_sha256())
                .unwrap();
            port.project_authorized_presence(&stale, NOW).await.unwrap();
            assert!(!port
                .own_presence_effect_usable(
                    stale.envelope_sha256(),
                    &profile.document().profile_did,
                    NOW
                )
                .await
                .unwrap());
            // A new timer/Chat preparation under the new grant still works,
            // including within the very same heartbeat bucket.
            let publisher = if same_mode {
                PresencePublisher::Runtime
            } else {
                PresencePublisher::CommunityOpen
            };
            let new_grant = presence_grant(
                &authority,
                &fixture.data_root,
                second,
                &profile.document().profile_did,
                &port.presence_scope(),
                publisher,
                NOW,
            )
            .await
            .unwrap();
            assert_ne!(
                old_grant.bind_request_id("second-person"),
                new_grant.bind_request_id("second-person")
            );
            let fresh = port
                .prepare_presence(
                    crate::collaboration_presence::presence_request_binding(
                        &new_grant.bind_request_id("second-person"),
                        second,
                        &profile,
                    )
                    .unwrap(),
                    &profile,
                    NOW,
                )
                .unwrap();
            port.project_authorized_presence(&fresh, NOW).await.unwrap();
            assert!(port
                .own_presence_effect_usable(
                    fresh.envelope_sha256(),
                    &profile.document().profile_did,
                    NOW
                )
                .await
                .unwrap());
            // An old prepared handle also fails after the fresh projection:
            // its denial preserves the new record and history authority.
            port.project_authorized_presence(&old, NOW).await.unwrap();
            assert!(port
                .authorized_snapshot(NOW)
                .await
                .unwrap()
                .records()
                .iter()
                .any(|record| record.sender_profile_did() == profile.document().profile_did));
            let crate::collaboration_presence::OwnHistoryPresence::Usable(history) =
                port.own_history_presence(NOW).await.unwrap()
            else {
                panic!("fresh history presence")
            };
            assert_eq!(
                history.into_envelope().as_bytes(),
                fresh.test_envelope_bytes()
            );
            let result = driver.retry_gossip_once(NOW).await.unwrap();
            assert_eq!(result.attempted, 1);
            let sent = carrier
                .requests()
                .into_iter()
                .filter(|r| r["op"] == "gossip_send")
                .collect::<Vec<_>>();
            assert_eq!(sent.len(), 2);
            let frame = crate::collaboration_protocol::verify_collaboration_transport_frame(
                sent[1]["message"].as_str().unwrap().as_bytes(),
            )
            .unwrap();
            assert_eq!(frame.envelope_bytes(), fresh.test_envelope_bytes());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_queued_presence_is_not_sent_after_hidden() {
        let a = Fixture::new();
        let ac = a.core();
        let (driver, port, authority) =
            granted_presence_driver(&a, ac.clone(), FakeCarrier::new()).await;
        queued_presence(&a, &port, "granted-presence").await;
        assert_eq!(driver.retry_outgoing_once(NOW).await.unwrap().attempted, 1);
        // Hidden lands while the announcement is still queued and live; the
        // next send, as after a reconnect, discloses nothing.
        set_presence(&a, &port, &authority, PresenceDecision::Hidden).await;
        tokio::time::advance(SHARED_CUSTODY_RETRY).await;
        assert_eq!(
            driver.retry_outgoing_once(NOW + 5).await.unwrap().attempted,
            0
        );
        assert!(ac.pending_outgoing(NOW + 5).unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_queued_presence_is_not_sent_after_an_epoch_bump() {
        let a = Fixture::new();
        let ac = a.core();
        let (driver, port, authority) =
            granted_presence_driver(&a, ac.clone(), FakeCarrier::new()).await;
        queued_presence(&a, &port, "granted-presence").await;
        assert_eq!(driver.retry_outgoing_once(NOW).await.unwrap().attempted, 1);
        authority.manager().revoke_all("test epoch");
        tokio::time::advance(SHARED_CUSTODY_RETRY).await;
        assert_eq!(
            driver.retry_outgoing_once(NOW + 5).await.unwrap().attempted,
            0
        );
        assert!(ac.pending_outgoing(NOW + 5).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_bare_driver_cannot_send_queued_own_presence() {
        let a = Fixture::new();
        let core = a.core();
        let carrier = FakeCarrier::new();
        let driver = a.driver(core.clone(), carrier.clone()).await;
        let presence = prepare_metadata(&a, &core, "bare-driver-presence");
        core.acknowledge_outgoing_product_projection(presence.envelope_sha256())
            .unwrap();
        assert_eq!(driver.retry_outgoing_once(NOW).await.unwrap().attempted, 0);
        assert!(core.pending_outgoing(NOW).unwrap().is_empty());
        assert!(carrier
            .requests()
            .iter()
            .all(|request| request["op"] != "gossip_send"));
    }

    fn remote_receipt(
        fixture: &Fixture,
        outgoing: &crate::collaboration_core::DurableOutgoingMessage,
        key: SigningKey,
    ) -> Vec<u8> {
        let authority = fixture.authority(key);
        let authorized = authority
            .authorize_incoming(
                outgoing.envelope_bytes(),
                serde_json::from_slice::<SignedCollaborationMessage>(outgoing.envelope_bytes())
                    .unwrap()
                    .signer_did
                    .as_str(),
                NOW + 1,
            )
            .unwrap();
        authority
            .prepare_acceptance_receipt(&authorized, NOW + 1)
            .unwrap()
    }

    fn peer_fixture(owner: &Fixture) -> Fixture {
        let mut peer = Fixture::new();
        peer.profile = owner.profile.clone();
        peer.grant = owner.grant.clone();
        peer
    }

    fn presence_frame(fixture: &Fixture, key: &SigningKey, now: u64) -> Vec<u8> {
        let message = fixture
            .authority(key.clone())
            .prepare_outgoing(
                SERVICE,
                crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE,
                serde_json::json!({}),
                now,
                45,
            )
            .unwrap();
        sign_collaboration_transport_frame(key, message.envelope_bytes()).unwrap()
    }

    fn observe(
        fixture: &Fixture,
        core: &Arc<CollaborationCore>,
        presence: &CollaborationPresenceProductPort,
        peer: &Fixture,
    ) {
        let wire = presence_frame(fixture, &peer.device_key, NOW);
        assert!(matches!(
            core.ingest_transport_frame(&wire, NOW).unwrap(),
            CollaborationTransportIngestion::Incoming(_)
        ));
        for handoff in presence.pending_presences().unwrap() {
            presence.project_handoff(&handoff, NOW).unwrap();
        }
    }

    fn invocation(frame: &[u8], source: &str) -> serde_json::Value {
        serde_json::json!({"op":SHARED_OP,"frame":std::str::from_utf8(frame).unwrap(),"_runtime_invocation":{
            "schema":"elastos.provider.invocation/v1", "source":SHARED_PROVIDER,"target":SHARED_PROVIDER,"op":SHARED_OP,
            "capability":format!("provider:{SHARED_PROVIDER}->{SHARED_PROVIDER}:{SHARED_OP}"), "transport":"carrier-provider-plane", "transfer":"json", "carrier":{"source_endpoint_did":source}
        }})
    }

    fn delivery(
        core: Arc<CollaborationCore>,
        registry: Arc<ProviderRegistry>,
    ) -> SharedHomeDelivery {
        SharedHomeDelivery {
            presence: CollaborationPresenceProductPort::new(core.clone()).unwrap(),
            core,
            registry,
            attempts: Arc::new(Mutex::new(SharedRetrySchedule::default())),
        }
    }

    struct Plane {
        source: String,
        peers: BTreeMap<String, SharedHomeDelivery>,
        calls: Mutex<Vec<(String, Vec<u8>)>>,
        lose_reply: AtomicBool,
        stall: bool,
        active: Arc<AtomicUsize>,
    }
    struct Active(Arc<AtomicUsize>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    #[async_trait::async_trait]
    impl ProviderCarrierInvoker for Plane {
        async fn invoke_carrier_provider(
            &self,
            route: &ProviderCarrierRoute,
            _: &ProviderInvocation,
            mut request: serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            let ProviderCarrierRoute::PeerDid { peer_did, .. } = route else {
                panic!("Shared selects one Home");
            };
            self.calls.lock().unwrap().push((
                peer_did.clone(),
                request["frame"].as_str().unwrap().as_bytes().to_vec(),
            ));
            self.active.fetch_add(1, Ordering::SeqCst);
            let _active = Active(self.active.clone());
            if self.stall {
                std::future::pending::<()>().await;
            }
            let remote = self
                .peers
                .get(peer_did)
                .ok_or_else(|| ProviderError::Unavailable("NoAddress".to_string()))?;
            request["_runtime_invocation"]["carrier"] =
                serde_json::json!({"source_endpoint_did":self.source});
            let response = remote
                .receive(&request, NOW + 1)
                .map_err(|error| ProviderError::Provider(error.to_string()))?;
            if self.lose_reply.swap(false, Ordering::SeqCst) {
                return Err(ProviderError::Unavailable(
                    "reply lost after durable accept".to_string(),
                ));
            }
            Ok(response)
        }
    }

    async fn home_driver(
        fixture: &Fixture,
        core: Arc<CollaborationCore>,
        carrier: Arc<FakeCarrier>,
        peer: &Fixture,
        peer_core: Arc<CollaborationCore>,
        lose_reply: bool,
        stall: bool,
    ) -> (
        CollaborationTransportDriver,
        Arc<Plane>,
        CollaborationPresenceProductPort,
    ) {
        let presence = CollaborationPresenceProductPort::new(core.clone()).unwrap();
        observe(fixture, &core, &presence, peer);
        let peers = BTreeMap::from([(
            peer_core.local_device_did(),
            delivery(peer_core, Arc::new(ProviderRegistry::new())),
        )]);
        plane_driver(fixture, core, carrier, peers, lose_reply, stall).await
    }

    async fn plane_driver(
        fixture: &Fixture,
        core: Arc<CollaborationCore>,
        carrier: Arc<dyn Provider>,
        peers: BTreeMap<String, SharedHomeDelivery>,
        lose_reply: bool,
        stall: bool,
    ) -> (
        CollaborationTransportDriver,
        Arc<Plane>,
        CollaborationPresenceProductPort,
    ) {
        let registry = Arc::new(ProviderRegistry::new());
        let (presence, _authority) = granted_presence_port(fixture, core.clone()).await;
        let plane = Arc::new(Plane {
            source: core.local_device_did(),
            peers,
            calls: Mutex::new(Vec::new()),
            lose_reply: AtomicBool::new(lose_reply),
            stall,
            active: Arc::new(AtomicUsize::new(0)),
        });
        registry.set_carrier_invoker(plane.clone()).await;
        let driver = fixture
            .driver(core, carrier)
            .await
            .with_home_delivery(presence.clone(), registry)
            .await
            .unwrap();
        (driver, plane, presence)
    }

    /// One gossip topic whose bootstrap forwards every frame to every other
    /// joined Home. Homes share no direct path.
    struct BootstrapRelay {
        profile: VerifiedCollaborationNetworkProfile,
        homes: Mutex<Vec<(Vec<String>, u64)>>,
    }
    struct RelayCarrier {
        relay: Arc<BootstrapRelay>,
        home: usize,
    }
    impl BootstrapRelay {
        fn new(profile: &VerifiedCollaborationNetworkProfile) -> Arc<Self> {
            Arc::new(Self {
                profile: profile.clone(),
                homes: Mutex::default(),
            })
        }
        fn join(self: &Arc<Self>) -> Arc<RelayCarrier> {
            let mut homes = self.homes.lock().unwrap();
            homes.push((Vec::new(), 0));
            Arc::new(RelayCarrier {
                relay: self.clone(),
                home: homes.len() - 1,
            })
        }
        fn inbox(&self, home: &RelayCarrier) -> Vec<String> {
            self.homes.lock().unwrap()[home.home].0.clone()
        }
    }
    /// Carrier's reply to `remember_peer` for the profile's one bootstrap.
    fn remembered(profile: &VerifiedCollaborationNetworkProfile) -> serde_json::Value {
        let node = &profile.profile().bootstrap_peers[0].node_id;
        serde_json::json!({"status":"ok","data":{"added":[node]}})
    }
    #[async_trait::async_trait]
    impl Provider for RelayCarrier {
        async fn handle(&self, _: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
            unreachable!()
        }
        fn schemes(&self) -> Vec<&'static str> {
            Vec::new()
        }
        fn name(&self) -> &'static str {
            "test-relay-carrier"
        }
        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            let mut homes = self.relay.homes.lock().unwrap();
            match request["op"].as_str().unwrap() {
                "remember_peer" => Ok(remembered(&self.relay.profile)),
                "gossip_join_exact" => {
                    Ok(serde_json::json!({"status":"ok","data":{"topic":request["topic"]}}))
                }
                "gossip_send" => {
                    let message = request["message"].as_str().unwrap();
                    for (index, (inbox, _)) in homes.iter_mut().enumerate() {
                        if index != self.home {
                            inbox.push(message.to_string());
                        }
                    }
                    Ok(serde_json::json!({"status":"ok","data":{"remote_peer_count":1}}))
                }
                "gossip_peek" => {
                    let (inbox, cursor) = &homes[self.home];
                    let frames = inbox[*cursor as usize..]
                        .iter()
                        .take(32)
                        .map(|message| serde_json::json!({"content":message}))
                        .collect::<Vec<_>>();
                    Ok(peek(*cursor, *cursor + frames.len() as u64, frames))
                }
                "gossip_ack" => {
                    let cursor = request["cursor"].as_u64().unwrap();
                    let next = request["next_cursor"].as_u64().unwrap();
                    homes[self.home].1 = next;
                    Ok(ack(cursor, next))
                }
                _ => Err(ProviderError::Unavailable(
                    "relay has no such op".to_string(),
                )),
            }
        }
    }

    /// A fixture whose profile names `seed` as its Community bootstrap.
    fn bootstrap_fixture(seed: &SigningKey) -> Fixture {
        let mut fixture = Fixture::new();
        let key = crate::carrier::did_to_public_key(&crate::crypto::encode_signing_key_did(seed))
            .unwrap();
        let ticket = serde_json::to_vec(
            &serde_json::json!({"topic":null,"endpoints":[iroh::EndpointAddr::from(key)]}),
        )
        .unwrap();
        let bootstrap = crate::collaboration_network::CollaborationBootstrapPeer {
            node_id: key.to_string(),
            connect_ticket: data_encoding::BASE32_NOPAD
                .encode(&ticket)
                .to_ascii_lowercase(),
        };
        let grant_bytes =
            canonical_default_conversation_grant_bytes(fixture.grant.grant()).unwrap();
        let (signer, _) = generate_keypair();
        fixture.profile = verified_profile(&signer, raw_sha256_cid(&grant_bytes), vec![bootstrap]);
        fixture.grant = verify_default_conversation_grant(&fixture.profile, &grant_bytes).unwrap();
        fixture
    }

    fn receipt_message_hash(frame: &str) -> String {
        let receipt =
            crate::collaboration_protocol::verify_collaboration_transport_frame(frame.as_bytes())
                .unwrap();
        let receipt: serde_json::Value = serde_json::from_slice(receipt.envelope_bytes()).unwrap();
        receipt["payload"]["message_envelope_sha256"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[tokio::test(start_paused = true)]
    async fn direct_home_delivery_preserves_original_after_lost_reply_and_settles_only_selected_peer(
    ) {
        let a = Fixture::new();
        let b = peer_fixture(&a);
        let ac = a.core();
        let bc = b.core();
        let carrier = FakeCarrier::new();
        let outgoing = prepare_outgoing(
            &ac,
            "held-response",
            serde_json::json!({"body":"one original"}),
        );
        ac.acknowledge_outgoing_product_projection(outgoing.envelope_sha256())
            .unwrap();
        let (driver, plane, _) =
            home_driver(&a, ac.clone(), carrier.clone(), &b, bc.clone(), true, false).await;
        // The gossip broadcast fails offline next to the lost direct reply.
        assert_eq!(
            driver.retry_outgoing_once(NOW).await.unwrap().send_failures,
            2
        );
        assert_eq!(bc.summary().unwrap().pending_product_handoffs, 1);
        assert!(!ac.pending_outgoing(NOW).unwrap()[0].accepted_by(&bc.local_device_did()));
        tokio::time::advance(Duration::from_millis(500)).await;
        assert_eq!(
            driver
                .retry_outgoing_once(NOW + 1)
                .await
                .unwrap()
                .remote_broadcasts,
            1
        );
        assert_eq!(
            bc.summary().unwrap().pending_product_handoffs,
            1,
            "lost reply replays one durable acceptance"
        );
        assert!(ac.pending_outgoing(NOW + 1).unwrap()[0].accepted_by(&bc.local_device_did()));
        assert_eq!(
            driver.retry_outgoing_once(NOW + 1).await.unwrap().attempted,
            0
        );
        let calls = plane.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], calls[1]);
        let frame =
            crate::collaboration_protocol::verify_collaboration_transport_frame(&calls[0].1)
                .unwrap();
        assert_eq!(frame.envelope_bytes(), outgoing.envelope_bytes());
        let gossip = carrier
            .requests()
            .into_iter()
            .filter(|request| request["op"] == "gossip_send")
            .map(|request| request["message"].as_str().unwrap().as_bytes().to_vec())
            .collect::<Vec<_>>();
        assert_eq!(
            gossip,
            [calls[0].1.clone()],
            "one broadcast of the same original"
        );
        assert!(ac.pending_outgoing(NOW + TTL).unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn blocked_metadata_cannot_delay_first_direct_attempt_or_lost_reply_retry() {
        let a = Fixture::new();
        let b = peer_fixture(&a);
        let ac = a.core();
        let bc = b.core();
        let carrier = FakeCarrier::new();
        carrier.blocked_metadata.store(true, Ordering::SeqCst);
        // The blocked send has to be a granted presence. A presence with no
        // grant is refused and never occupies gossip, so the direct chat is
        // what blocks and the next cycle has nothing left to send.
        let (driver, plane, port) =
            home_driver(&a, ac.clone(), carrier.clone(), &b, bc.clone(), true, false).await;
        // Queue both metadata originals ahead of Chat. At the 500 ms retry,
        // the first is not due for its 5 s resend; the untouched second one is.
        let metadata = [
            queued_presence(&a, &port, "local-metadata-first").await,
            queued_presence(&a, &port, "local-metadata-retry").await,
        ];
        assert_ne!(metadata[0].envelope_sha256(), metadata[1].envelope_sha256());
        for message in &metadata {
            let (_, profile_did) = message.presence_sender().unwrap();
            assert!(
                port.own_presence_effect_usable(message.envelope_sha256(), profile_did, NOW)
                    .await
                    .unwrap(),
                "blocked metadata must be authorized by the driver's presence port"
            );
            assert!(ac
                .pending_outgoing(NOW)
                .unwrap()
                .iter()
                .any(|pending| pending.envelope_sha256() == message.envelope_sha256()));
        }
        let outgoing = prepare_outgoing(&ac, "Chat", serde_json::json!({"body":"direct"}));
        ac.acknowledge_outgoing_product_projection(outgoing.envelope_sha256())
            .unwrap();
        let driver = Arc::new(driver);
        for count in [1, 2] {
            let running = driver.clone();
            let cycle = tokio::spawn(async move { running.retry_outgoing_once(NOW + 1).await });
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while plane.calls.lock().unwrap().len() < count
                || carrier
                    .requests()
                    .iter()
                    .filter(|request| request["op"] == "gossip_send")
                    .count()
                    < count
            {
                assert!(
                    std::time::Instant::now() < deadline,
                    "direct attempt waited for blocked metadata"
                );
                tokio::task::yield_now().await;
            }
            let sends = carrier
                .requests()
                .into_iter()
                .filter(|request| request["op"] == "gossip_send")
                .collect::<Vec<_>>();
            assert_eq!(
                sends.len(),
                count,
                "each direct attempt starts while its metadata send is blocked"
            );
            let frame = crate::collaboration_protocol::verify_collaboration_transport_frame(
                sends[count - 1]["message"].as_str().unwrap().as_bytes(),
            )
            .unwrap();
            assert_eq!(frame.envelope_bytes(), metadata[count - 1].envelope_bytes());
            assert!(
                !cycle.is_finished(),
                "metadata must still be pending at direct dispatch"
            );
            tokio::time::advance(SHARED_CUSTODY_FAST_RETRY).await;
            cycle.await.unwrap().unwrap();
        }
        let calls = plane.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0], calls[1],
            "the lost reply retries the same original"
        );
        assert_eq!(bc.summary().unwrap().pending_product_handoffs, 1);
        assert!(ac
            .pending_outgoing(NOW + 1)
            .unwrap()
            .iter()
            .any(
                |message| message.envelope_sha256() == outgoing.envelope_sha256()
                    && message.accepted_by(&bc.local_device_did())
            ));
    }

    #[tokio::test(start_paused = true)]
    async fn unreachable_homes_keep_exact_expiry_and_four_attempts_rotate_without_starvation() {
        let a = Fixture::new();
        let b = peer_fixture(&a);
        let ac = a.core();
        let bc = b.core();
        let carrier = FakeCarrier::new();
        let (driver, plane, _) = home_driver(&a, ac.clone(), carrier, &b, bc, false, false).await;
        // Every selected Home is unavailable; a failure cannot discard a claim.
        plane
            .peers
            .values()
            .next()
            .unwrap()
            .core
            .set_community_joined(false)
            .unwrap();
        let mut hashes = HashSet::new();
        for i in 0..9 {
            let out = prepare_outgoing(
                &ac,
                &format!("fair-{i}"),
                serde_json::json!({"body":format!("m{i}")}),
            );
            ac.acknowledge_outgoing_product_projection(out.envelope_sha256())
                .unwrap();
            hashes.insert(out.envelope_sha256().to_string());
        }
        // Gossip keeps the same per-cycle bound and cadence beside direct.
        for expected in [4, 4, 1] {
            let result = driver.retry_outgoing_once(NOW + 3).await.unwrap();
            assert_eq!(result.attempted, 2 * expected);
            assert_eq!(result.send_failures, 2 * expected);
        }
        let attempted: HashSet<_> = plane
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, frame)| {
                collaboration_message_envelope_sha256(
                    crate::collaboration_protocol::verify_collaboration_transport_frame(frame)
                        .unwrap()
                        .envelope_bytes(),
                )
            })
            .collect();
        assert_eq!(attempted, hashes);
        assert_eq!(
            driver
                .retry_outgoing_once(NOW + 100)
                .await
                .unwrap()
                .attempted,
            0,
            "wall time does not bypass monotonic cadence"
        );
        tokio::time::advance(Duration::from_secs(5)).await;
        assert_eq!(
            driver.retry_outgoing_once(NOW + 8).await.unwrap().attempted,
            8
        );
        assert_eq!(
            driver
                .retry_outgoing_once(NOW + TTL)
                .await
                .unwrap()
                .attempted,
            0
        );
        assert!(ac.pending_outgoing(NOW + TTL).unwrap().is_empty());
    }

    #[tokio::test]
    async fn shared_gossip_is_admitted_once_and_forged_provider_source_is_refused() {
        let a = Fixture::new();
        let ac = a.core();
        let (sender, _) = generate_keypair();
        let (_, message) = remote_message(&a, sender.clone(), "Shared post");
        let frame = sign_collaboration_transport_frame(&sender, &message).unwrap();
        let service = delivery(ac.clone(), Arc::new(ProviderRegistry::new()));
        let (foreign, _) = generate_keypair();
        assert!(service
            .receive(
                &invocation(&frame, &crate::crypto::encode_signing_key_did(&foreign)),
                NOW
            )
            .is_err());
        let mut raw = invocation(&frame, &crate::crypto::encode_signing_key_did(&sender));
        raw.as_object_mut().unwrap().remove("_runtime_invocation");
        assert!(service.receive(&raw, NOW).is_err());
        assert_eq!(ac.summary().unwrap().pending_product_handoffs, 0);

        let carrier = FakeCarrier::new();
        let driver = a.driver(ac.clone(), carrier.clone()).await;
        carrier.push([
            peek(0, 1, vec![gossip(&frame)]),
            serde_json::json!({"status":"ok","data":{"remote_peer_count":1}}),
            ack(0, 1),
        ]);
        assert_eq!(
            driver.process_incoming_once(NOW).await.unwrap(),
            CollaborationIncomingOnceOutcome::Acknowledged(CollaborationIncomingOnceSummary {
                incoming_acceptances: 1,
                acceptance_receipt_broadcasts: 1,
                ..CollaborationIncomingOnceSummary::default()
            })
        );
        let receipt = carrier
            .requests()
            .into_iter()
            .find(|request| request["op"] == "gossip_send")
            .unwrap()["message"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(ac
            .allows_gossip_receipt(receipt.as_bytes(), &frame)
            .unwrap());
        // The direct copy replays the gossip admission: one row, one receipt.
        let response = service
            .receive(
                &invocation(&frame, &crate::crypto::encode_signing_key_did(&sender)),
                NOW,
            )
            .unwrap();
        assert_eq!(
            crate::collaboration_protocol::verify_collaboration_transport_frame(receipt.as_bytes())
                .unwrap()
                .envelope_bytes(),
            response["receipt"].as_str().unwrap().as_bytes()
        );
        assert_eq!(ac.summary().unwrap().pending_product_handoffs, 1);
    }

    #[tokio::test]
    async fn seed_sender_receiver_presence_and_receipt_are_excluded_from_chat_plane() {
        let (seed, _) = generate_keypair();
        let fixture = bootstrap_fixture(&seed);
        let core = fixture.core();
        let mut seed_fixture = peer_fixture(&fixture);
        seed_fixture.device_key = seed.clone();
        let seed_core = seed_fixture.core();
        assert!(core.is_bootstrap_endpoint(&seed_core.local_device_did()));
        let outgoing = prepare_outgoing(
            &core,
            "seed-refusal",
            serde_json::json!({"body":"Home only"}),
        );
        core.acknowledge_outgoing_product_projection(outgoing.envelope_sha256())
            .unwrap();
        let normal_frame = core
            .prepare_transport_frame(outgoing.envelope_bytes())
            .unwrap();
        assert!(seed_core
            .accept_home_chat(&normal_frame, &core.local_device_did(), NOW)
            .is_err());
        let (_, seed_message) = remote_message(&fixture, seed.clone(), "seed source");
        let seed_frame = sign_collaboration_transport_frame(&seed, &seed_message).unwrap();
        assert!(core
            .accept_home_chat(&seed_frame, &seed_core.local_device_did(), NOW)
            .is_err());
        // A seed-local Core refuses Shared posts over gossip too, and no Home
        // admits a Shared post that a seed signed.
        let refused = CollaborationTransportIngestion::Rejected(
            crate::collaboration_core::CollaborationTransportRejection::BootstrapEndpoint,
        );
        assert_eq!(
            seed_core
                .ingest_transport_frame(&normal_frame, NOW)
                .unwrap(),
            refused
        );
        assert_eq!(
            core.ingest_transport_frame(&seed_frame, NOW).unwrap(),
            refused
        );
        let seed_receipt = remote_receipt(&fixture, &outgoing, seed);
        assert!(core
            .record_home_chat_acceptance(
                &seed_receipt,
                outgoing.envelope_sha256(),
                &seed_core.local_device_did(),
                NOW + 1
            )
            .is_err());
        let (other, _) = generate_keypair();
        let receipt = remote_receipt(&fixture, &outgoing, other.clone());
        assert!(core
            .record_home_chat_acceptance(
                &receipt,
                outgoing.envelope_sha256(),
                &seed_core.local_device_did(),
                NOW + 1
            )
            .is_err());
        assert!(core
            .record_home_chat_acceptance(
                &receipt,
                "sha256:wrong-original",
                &crate::crypto::encode_signing_key_did(&other),
                NOW + 1
            )
            .is_err());
        let presence = CollaborationPresenceProductPort::new(core.clone()).unwrap();
        observe(&fixture, &core, &presence, &seed_fixture);
        assert!(presence.history_participants(NOW).unwrap().is_empty());
        let seed_presence = CollaborationPresenceProductPort::new(seed_core.clone()).unwrap();
        let normal_presence = crate::collaboration_protocol::verify_collaboration_transport_frame(
            &presence_frame(&fixture, &fixture.device_key, NOW),
        )
        .unwrap();
        assert!(seed_presence
            .authorize_history_requester(
                normal_presence.envelope_bytes(),
                &core.local_device_did(),
                NOW
            )
            .is_err());
        assert_eq!(seed_core.summary().unwrap().pending_product_handoffs, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn homes_without_discovery_or_direct_path_exchange_a_post_over_bootstrap_gossip() {
        for stale_listing in [false, true] {
            let (seed, _) = generate_keypair();
            let a = bootstrap_fixture(&seed);
            let b = peer_fixture(&a);
            let mut s = peer_fixture(&a);
            s.device_key = seed;
            let (ac, bc, sc) = (a.core(), b.core(), s.core());
            if stale_listing {
                // A listing makes A dial B directly; the dial fails.
                let presence = CollaborationPresenceProductPort::new(ac.clone()).unwrap();
                observe(&a, &ac, &presence, &b);
            }
            let relay = BootstrapRelay::new(&a.profile);
            let a_carrier = relay.join();
            let b_carrier = relay.join();
            let (ad, plane, _) = plane_driver(
                &a,
                ac.clone(),
                a_carrier.clone(),
                BTreeMap::new(),
                false,
                false,
            )
            .await;
            let bd = b.driver(bc.clone(), b_carrier.clone()).await;
            let seed_carrier = relay.join();
            let sd = s.driver(sc.clone(), seed_carrier.clone()).await;
            let outgoing = prepare_outgoing(&ac, "gossip-post", serde_json::json!({"body":"hi"}));
            ac.acknowledge_outgoing_product_projection(outgoing.envelope_sha256())
                .unwrap();

            ad.retry_outgoing_once(NOW).await.unwrap();
            ad.retry_outgoing_once(NOW).await.unwrap();
            assert_eq!(relay.inbox(&b_carrier).len(), 1, "one broadcast at send");
            assert_eq!(
                plane.calls.lock().unwrap().len(),
                usize::from(stale_listing)
            );
            tokio::time::advance(SHARED_CUSTODY_RETRY).await;
            ad.retry_outgoing_once(NOW + 5).await.unwrap();
            assert_eq!(relay.inbox(&b_carrier).len(), 2, "slow re-broadcast");

            assert_eq!(
                bd.process_incoming_once(NOW + 5).await.unwrap(),
                CollaborationIncomingOnceOutcome::Acknowledged(CollaborationIncomingOnceSummary {
                    incoming_acceptances: 2,
                    acceptance_receipt_broadcasts: 2,
                    ..CollaborationIncomingOnceSummary::default()
                })
            );
            assert_eq!(bc.pending_product_handoffs().unwrap().len(), 1);
            assert!(!ac.pending_outgoing(NOW + 5).unwrap()[0].remote_acceptance_recorded());
            assert_eq!(
                ad.process_incoming_once(NOW + 6).await.unwrap(),
                CollaborationIncomingOnceOutcome::Acknowledged(CollaborationIncomingOnceSummary {
                    remote_acceptances: 2,
                    ..CollaborationIncomingOnceSummary::default()
                })
            );
            let accepted = &ac.pending_outgoing(NOW + 6).unwrap()[0];
            assert!(accepted.accepted_by(&bc.local_device_did()));
            assert_eq!(ac.summary().unwrap().remotely_accepted_outgoing, 1);
            tokio::time::advance(SHARED_CUSTODY_RETRY).await;
            ad.retry_outgoing_once(NOW + 11).await.unwrap();
            assert_eq!(
                relay.inbox(&b_carrier).len(),
                2,
                "the Home receipt ends gossip"
            );

            // The seed forwarded both posts and receipts and admitted none.
            assert_eq!(relay.inbox(&seed_carrier).len(), 4);
            assert!(matches!(
                sd.process_incoming_once(NOW + 11).await.unwrap(),
                CollaborationIncomingOnceOutcome::Acknowledged(summary)
                    if summary.deterministic_rejections == 4 && summary.incoming_acceptances == 0
            ));
            let seed_state = sc.summary().unwrap();
            assert_eq!(
                seed_state.pending_product_handoffs + seed_state.replay_tombstones,
                0
            );
            assert!(sc.pending_product_handoffs().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn bootstrap_signed_receipt_never_accepts_a_shared_post() {
        let (seed, _) = generate_keypair();
        let a = bootstrap_fixture(&seed);
        let ac = a.core();
        let outgoing = prepare_outgoing(&ac, "seed-receipt", serde_json::json!({"body":"hi"}));
        ac.acknowledge_outgoing_product_projection(outgoing.envelope_sha256())
            .unwrap();
        let receipt = remote_receipt(&a, &outgoing, seed.clone());
        let frame = sign_collaboration_transport_frame(&seed, &receipt).unwrap();
        let carrier = FakeCarrier::new();
        carrier.push([remembered(&a.profile)]);
        let driver = a.driver(ac.clone(), carrier.clone()).await;
        carrier.push([peek(0, 1, vec![gossip(&frame)]), ack(0, 1)]);
        assert_eq!(
            driver.process_incoming_once(NOW + 1).await.unwrap(),
            CollaborationIncomingOnceOutcome::Acknowledged(CollaborationIncomingOnceSummary {
                deterministic_rejections: 1,
                ..CollaborationIncomingOnceSummary::default()
            })
        );
        assert_eq!(
            ac.ingest_transport_frame(&frame, NOW + 1).unwrap(),
            CollaborationTransportIngestion::Rejected(
                crate::collaboration_core::CollaborationTransportRejection::BootstrapEndpoint,
            )
        );
        assert!(ac.record_remote_acceptance(&receipt, NOW + 1).is_err());
        assert!(!ac.pending_outgoing(NOW + 1).unwrap()[0].remote_acceptance_recorded());
        assert_eq!(ac.summary().unwrap().remotely_accepted_outgoing, 0);
    }

    #[test]
    fn shared_gossip_backs_off_to_a_minute_and_presence_keeps_five_seconds() {
        let shared = (1..=7)
            .map(|sends| gossip_resend_interval(true, sends).as_secs())
            .collect::<Vec<_>>();
        assert_eq!(shared, [5, 10, 20, 40, 60, 60, 60]);
        assert!((1..=7).all(|sends| gossip_resend_interval(false, sends) == SHARED_CUSTODY_RETRY));
    }

    #[tokio::test(start_paused = true)]
    async fn unanswered_gossip_resends_on_its_schedule_not_every_cycle() {
        let a = Fixture::new();
        let ac = a.core();
        let carrier = FakeCarrier::new();
        let (driver, port, _authority) =
            granted_presence_driver(&a, ac.clone(), carrier.clone()).await;
        let presence = queued_presence(&a, &port, "unanswered-presence").await;
        let post = prepare_outgoing(&ac, "unanswered-post", serde_json::json!({"body":"hi"}));
        for message in [&presence, &post] {
            ac.acknowledge_outgoing_product_projection(message.envelope_sha256())
                .unwrap();
        }
        // No Home answers and every broadcast fails, as with an overloaded seed.
        let mut ticks = HashMap::<String, Vec<u32>>::new();
        let mut seen = 1;
        // 400 worker cycles of 500 ms: 200 s.
        for tick in 0..400 {
            driver.retry_outgoing_once(NOW).await.unwrap();
            let requests = carrier.requests();
            for request in &requests[seen..] {
                let frame = crate::collaboration_protocol::verify_collaboration_transport_frame(
                    request["message"].as_str().unwrap().as_bytes(),
                )
                .unwrap();
                ticks
                    .entry(collaboration_message_envelope_sha256(
                        frame.envelope_bytes(),
                    ))
                    .or_default()
                    .push(tick);
            }
            seen = requests.len();
            tokio::time::advance(SHARED_CUSTODY_FAST_RETRY).await;
        }
        let gaps = |hash: &str| {
            ticks[hash]
                .windows(2)
                .map(|pair| (pair[1] - pair[0]) / 2)
                .collect::<Vec<_>>()
        };
        assert_eq!(ticks[presence.envelope_sha256()][0], 0);
        assert_eq!(
            gaps(presence.envelope_sha256()),
            vec![5; 39],
            "Presence: every 5 s"
        );
        assert_eq!(ticks[post.envelope_sha256()][0], 0);
        assert_eq!(gaps(post.envelope_sha256()), [5, 10, 20, 40, 60, 60]);
    }

    #[tokio::test]
    async fn bootstrap_node_admits_and_sends_nothing_and_consumes_the_topic() {
        let (seed, _) = generate_keypair();
        let a = bootstrap_fixture(&seed);
        let mut s = peer_fixture(&a);
        s.device_key = seed;
        let sc = s.core();
        let (home, _) = generate_keypair();
        let (_, post) = remote_message(&a, home.clone(), "Shared post");
        let ac = a.core();
        let outgoing = prepare_outgoing(&ac, "home-post", serde_json::json!({"body":"hi"}));
        let receipt =
            sign_collaboration_transport_frame(&home, &remote_receipt(&a, &outgoing, home.clone()))
                .unwrap();
        let frames = vec![
            gossip(&presence_frame(&a, &home, NOW)),
            gossip(&sign_collaboration_transport_frame(&home, &post).unwrap()),
            gossip(&receipt),
        ];
        let own = prepare_metadata(&s, &sc, "seed-presence");
        sc.acknowledge_outgoing_product_projection(own.envelope_sha256())
            .unwrap();
        let carrier = FakeCarrier::new();
        carrier.push([remembered(&a.profile)]);
        let driver = s.driver(sc.clone(), carrier.clone()).await;
        carrier.push([peek(0, 3, frames), ack(0, 3)]);
        // The Carrier forwards the topic; the Core only advances past it.
        assert_eq!(
            driver.process_incoming_once(NOW).await.unwrap(),
            CollaborationIncomingOnceOutcome::Acknowledged(CollaborationIncomingOnceSummary {
                deterministic_rejections: 3,
                ..CollaborationIncomingOnceSummary::default()
            })
        );
        assert_eq!(driver.retry_outgoing_once(NOW).await.unwrap().attempted, 0);
        assert_eq!(
            request_ops(&carrier),
            [
                "remember_peer",
                "gossip_join_exact",
                "gossip_peek",
                "gossip_ack"
            ]
        );
        let summary = sc.summary().unwrap();
        assert_eq!(
            summary.pending_product_handoffs + summary.replay_tombstones,
            0
        );
    }

    #[tokio::test]
    async fn redundant_receipts_that_cannot_be_stored_never_hold_the_gossip_batch() {
        // Full receipt capacity, or any storage failure such as byte capacity.
        for at_capacity in [true, false] {
            let a = Fixture::new();
            let ac = a.core();
            let outgoing = prepare_outgoing(&ac, "many-homes", serde_json::json!({"body":"hi"}));
            ac.acknowledge_outgoing_product_projection(outgoing.envelope_sha256())
                .unwrap();
            let receipt_frame = |key: SigningKey| {
                let receipt = remote_receipt(&a, &outgoing, key.clone());
                sign_collaboration_transport_frame(&key, &receipt).unwrap()
            };
            let stored = if at_capacity {
                crate::collaboration_core::MAX_ACCEPTANCE_RECEIPTS_PER_OUTGOING
            } else {
                1
            };
            for _ in 0..stored {
                ac.ingest_transport_frame(&receipt_frame(generate_keypair().0), NOW + 1)
                    .unwrap();
            }
            if !at_capacity {
                ac.inject_write_fault(WriteFault::BeforeWrite);
            }
            let (other, _) = generate_keypair();
            let frames = vec![
                gossip(&receipt_frame(generate_keypair().0)),
                gossip(&presence_frame(&a, &other, NOW)),
            ];
            let carrier = FakeCarrier::new();
            let driver = a.driver(ac.clone(), carrier.clone()).await;
            carrier.push([
                peek(0, 2, frames),
                serde_json::json!({"status":"ok","data":{"remote_peer_count":1}}),
                ack(0, 2),
            ]);
            assert_eq!(
                driver.process_incoming_once(NOW + 1).await.unwrap(),
                CollaborationIncomingOnceOutcome::Acknowledged(CollaborationIncomingOnceSummary {
                    deterministic_rejections: 1,
                    incoming_acceptances: 1,
                    acceptance_receipt_broadcasts: 1,
                    ..CollaborationIncomingOnceSummary::default()
                })
            );
            assert!(ac.pending_outgoing(NOW + 1).unwrap()[0].remote_acceptance_recorded());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn direct_and_gossip_copies_give_one_row_and_one_receipt() {
        let a = Fixture::new();
        let b = peer_fixture(&a);
        let (ac, bc) = (a.core(), b.core());
        let relay = BootstrapRelay::new(&a.profile);
        let a_carrier = relay.join();
        let b_carrier = relay.join();
        let presence = CollaborationPresenceProductPort::new(ac.clone()).unwrap();
        observe(&a, &ac, &presence, &b);
        let peers = BTreeMap::from([(
            bc.local_device_did(),
            delivery(bc.clone(), Arc::new(ProviderRegistry::new())),
        )]);
        let (ad, plane, _) =
            plane_driver(&a, ac.clone(), a_carrier.clone(), peers, false, false).await;
        let bd = b.driver(bc.clone(), b_carrier).await;
        let outgoing = prepare_outgoing(&ac, "both-paths", serde_json::json!({"body":"hi"}));
        ac.acknowledge_outgoing_product_projection(outgoing.envelope_sha256())
            .unwrap();
        assert_eq!(
            ad.retry_outgoing_once(NOW).await.unwrap().remote_broadcasts,
            2,
            "one gossip broadcast and one direct delivery"
        );
        assert_eq!(plane.calls.lock().unwrap().len(), 1);
        assert!(ac.pending_outgoing(NOW).unwrap()[0].accepted_by(&bc.local_device_did()));
        assert_eq!(
            bd.process_incoming_once(NOW + 1).await.unwrap(),
            CollaborationIncomingOnceOutcome::Acknowledged(CollaborationIncomingOnceSummary {
                incoming_acceptances: 1,
                acceptance_receipt_broadcasts: 1,
                ..CollaborationIncomingOnceSummary::default()
            })
        );
        assert_eq!(bc.pending_product_handoffs().unwrap().len(), 1);
        let receipt = relay.inbox(&a_carrier).pop().unwrap();
        assert_eq!(
            ac.ingest_transport_frame(receipt.as_bytes(), NOW + 1)
                .unwrap(),
            CollaborationTransportIngestion::RemoteAcceptance(
                crate::collaboration_core::CollaborationTransportRemoteAcceptance::Replayed
            )
        );
        assert_eq!(
            ad.process_incoming_once(NOW + 1).await.unwrap(),
            CollaborationIncomingOnceOutcome::Acknowledged(CollaborationIncomingOnceSummary {
                remote_acceptances: 1,
                ..CollaborationIncomingOnceSummary::default()
            })
        );
        assert_eq!(ac.summary().unwrap().remotely_accepted_outgoing, 1);
    }

    #[tokio::test]
    async fn rate_limited_gossip_posts_are_held_and_released_in_order() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let (sender, _) = generate_keypair();
        let limit = crate::collaboration_rate_limit::COMMUNITY_RECEIVES_PER_SENDER_PER_WINDOW;
        let mut hashes = Vec::new();
        let mut frames = Vec::new();
        for i in 0..limit + 2 {
            let (_, message) = remote_message(&fixture, sender.clone(), &format!("post-{i}"));
            hashes.push(collaboration_message_envelope_sha256(&message));
            frames.push(gossip(
                &sign_collaboration_transport_frame(&sender, &message).unwrap(),
            ));
        }
        let sent = serde_json::json!({"status":"ok","data":{"remote_peer_count":1}});
        let carrier = FakeCarrier::new();
        let driver = fixture.driver(core.clone(), carrier.clone()).await;
        let count = frames.len() as u64;
        carrier.push([peek(0, count, frames)]);
        carrier.push(std::iter::repeat_n(sent.clone(), limit));
        carrier.push([ack(0, count)]);
        assert_eq!(
            driver.process_incoming_once(NOW).await.unwrap(),
            CollaborationIncomingOnceOutcome::Acknowledged(CollaborationIncomingOnceSummary {
                deterministic_rejections: 2,
                incoming_acceptances: limit,
                acceptance_receipt_broadcasts: limit,
                ..CollaborationIncomingOnceSummary::default()
            })
        );
        for handoff in core.pending_product_handoffs().unwrap() {
            core.acknowledge_product_handoff(
                handoff.authorized_message().message().envelope_sha256(),
            )
            .unwrap();
        }
        carrier.push([
            sent.clone(),
            sent,
            peek(count, count, Vec::new()),
            ack(count, count),
        ]);
        let window = crate::collaboration_rate_limit::COMMUNITY_RATE_WINDOW_SECS;
        assert_eq!(
            driver.process_incoming_once(NOW + window).await.unwrap(),
            CollaborationIncomingOnceOutcome::Acknowledged(CollaborationIncomingOnceSummary {
                incoming_acceptances: 2,
                acceptance_receipt_broadcasts: 2,
                ..CollaborationIncomingOnceSummary::default()
            })
        );
        let receipts = carrier
            .requests()
            .iter()
            .filter(|request| request["op"] == "gossip_send")
            .map(|request| receipt_message_hash(request["message"].as_str().unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(receipts, hashes, "every post is receipted, in send order");
    }

    #[tokio::test]
    async fn held_direct_message_retries_without_sender_and_storage_failure_retains_metadata_batch()
    {
        let fixture = Fixture::new();
        let core = fixture.core();
        let (sender, _) = generate_keypair();
        let source = crate::crypto::encode_signing_key_did(&sender);
        let receive_limit =
            crate::collaboration_rate_limit::COMMUNITY_RECEIVES_PER_SENDER_PER_WINDOW;
        let mut held_hash = None;
        for i in 0..=receive_limit {
            let (_, message) = remote_message(&fixture, sender.clone(), &format!("held-{i}"));
            if i == receive_limit {
                held_hash = Some(collaboration_message_envelope_sha256(&message));
            }
            let wire = sign_collaboration_transport_frame(&sender, &message).unwrap();
            let accepted = core.accept_home_chat(&wire, &source, NOW);
            assert_eq!(accepted.is_ok(), i < receive_limit);
        }
        for handoff in core.pending_product_handoffs().unwrap() {
            core.acknowledge_product_handoff(
                handoff.authorized_message().message().envelope_sha256(),
            )
            .unwrap();
        }
        assert!(core.pending_product_handoffs().unwrap().is_empty());
        let carrier = FakeCarrier::new();
        let driver = fixture.driver(core.clone(), carrier.clone()).await;
        // No gossip peer takes the receipt; the direct sender fetches it again.
        carrier.push_failure("no gossip peer");
        carrier.push([peek(0, 0, Vec::new()), ack(0, 0)]);
        let result = driver
            .process_incoming_once(
                NOW + crate::collaboration_rate_limit::COMMUNITY_RATE_WINDOW_SECS,
            )
            .await
            .unwrap();
        assert!(
            matches!(result, CollaborationIncomingOnceOutcome::Acknowledged(summary) if summary.incoming_acceptances == 1 && summary.acceptance_receipt_broadcasts == 0)
        );
        let handoffs = core.pending_product_handoffs().unwrap();
        assert_eq!(handoffs.len(), 1);
        assert_eq!(
            handoffs[0].authorized_message().message().envelope_sha256(),
            held_hash.unwrap()
        );
        assert_eq!(
            request_ops(&carrier),
            [
                "gossip_join_exact",
                "gossip_send",
                "gossip_peek",
                "gossip_ack"
            ]
        );
        let metadata = presence_frame(&fixture, &sender, NOW + 61);
        carrier.push([serde_json::json!({"status":"ok","data":{"messages":[{"content":std::str::from_utf8(&metadata).unwrap()}],"scanned":1,"limit":32,"cursor":0,"next_cursor":1}})]);
        core.inject_write_fault(WriteFault::BeforeWrite);
        assert!(matches!(
            driver.process_incoming_once(NOW + 61).await.unwrap(),
            CollaborationIncomingOnceOutcome::RetryRequired(_)
        ));
        assert_eq!(
            carrier.requests().last().unwrap()["op"],
            "gossip_peek",
            "transient storage failure cannot acknowledge a batch"
        );
    }

    #[tokio::test]
    async fn mixed_batch_waits_for_retryable_core_work_but_consumes_deterministic_rejections() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let (key, _) = generate_keypair();
        let original = presence_frame(&fixture, &key, NOW);
        let original_envelope =
            crate::collaboration_protocol::verify_collaboration_transport_frame(&original)
                .unwrap()
                .envelope_bytes()
                .to_vec();
        assert!(matches!(
            core.ingest_transport_frame(&original, NOW).unwrap(),
            CollaborationTransportIngestion::Incoming(_)
        ));
        core.acknowledge_product_handoff(&collaboration_message_envelope_sha256(
            &original_envelope,
        ))
        .unwrap();
        let mut conflict: SignedCollaborationMessage =
            serde_json::from_slice(&original_envelope).unwrap();
        conflict.payload.expires_at += 1;
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &key,
            COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
            &canonical_collaboration_message_bytes(&conflict.payload).unwrap(),
        );
        conflict.signature = signature;
        conflict.signer_did = signer_did;
        let conflict = sign_collaboration_transport_frame(
            &key,
            &canonical_signed_collaboration_message_bytes(&conflict).unwrap(),
        )
        .unwrap();
        let self_message = prepare_metadata(&fixture, &core, "self-metadata");
        let unknown = sign_collaboration_transport_frame(
            &key,
            br#"{"payload":{"schema":"elastos.collaboration.unknown/v1"}}"#,
        )
        .unwrap();
        let (_, shared) = remote_message(&fixture, key.clone(), "Shared post");
        let shared = sign_collaboration_transport_frame(&key, &shared).unwrap();
        let (valid_key, _) = generate_keypair();
        let metadata = presence_frame(&fixture, &valid_key, NOW);
        let frames = vec![
            serde_json::json!({"content":"not-a-frame"}),
            gossip(b"{"),
            gossip(&unknown),
            gossip(
                &core
                    .prepare_transport_frame(self_message.envelope_bytes())
                    .unwrap(),
            ),
            gossip(&conflict),
            gossip(&shared),
            gossip(&metadata),
        ];
        let carrier = FakeCarrier::new();
        let driver = fixture.driver(core.clone(), carrier.clone()).await;
        let sent = serde_json::json!({"status":"ok","data":{"remote_peer_count":1}});
        carrier.push([
            peek(0, 7, frames.clone()),
            peek(0, 7, frames),
            sent.clone(),
            sent,
            ack(0, 7),
        ]);
        core.inject_write_fault(WriteFault::BeforeWrite);
        assert!(matches!(
            driver.process_incoming_once(NOW).await.unwrap(),
            CollaborationIncomingOnceOutcome::RetryRequired(_)
        ));
        assert!(carrier
            .requests()
            .iter()
            .all(|request| request["op"] != "gossip_ack"));
        assert_eq!(
            driver.process_incoming_once(NOW).await.unwrap(),
            CollaborationIncomingOnceOutcome::Acknowledged(CollaborationIncomingOnceSummary {
                carrier_rejected_frames: 2,
                deterministic_rejections: 3,
                incoming_acceptances: 2,
                acceptance_receipt_broadcasts: 2,
                ..CollaborationIncomingOnceSummary::default()
            })
        );
        assert_eq!(core.pending_product_handoffs().unwrap().len(), 2);
        let requests = carrier.requests();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request["op"] == "gossip_ack")
                .count(),
            1
        );
        assert_eq!(
            request_ops(&carrier),
            [
                "gossip_join_exact",
                "gossip_peek",
                "gossip_peek",
                "gossip_send",
                "gossip_send",
                "gossip_ack"
            ]
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| request["op"] == "gossip_send")
                .count(),
            2
        );
        let receipts = requests
            .iter()
            .filter(|request| request["op"] == "gossip_send")
            .map(|request| request["message"].as_str().unwrap().as_bytes())
            .collect::<Vec<_>>();
        assert!(core.allows_gossip_receipt(receipts[0], &shared).unwrap());
        assert!(core.allows_gossip_receipt(receipts[1], &metadata).unwrap());
    }

    #[tokio::test]
    async fn empty_batch_is_acknowledged_without_product_or_transport_effects() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let carrier = FakeCarrier::new();
        let driver = fixture.driver(core.clone(), carrier.clone()).await;
        carrier.push([peek(7, 7, Vec::new()), ack(7, 7)]);
        assert_eq!(
            driver.process_incoming_once(NOW).await.unwrap(),
            CollaborationIncomingOnceOutcome::Acknowledged(
                CollaborationIncomingOnceSummary::default()
            )
        );
        assert!(core.pending_product_handoffs().unwrap().is_empty());
        assert_eq!(
            carrier
                .requests()
                .iter()
                .filter(|request| request["op"] == "gossip_send")
                .count(),
            0
        );
        assert_eq!(
            request_ops(&carrier),
            ["gossip_join_exact", "gossip_peek", "gossip_ack"]
        );
    }

    #[tokio::test]
    async fn incoming_receipt_is_durable_before_send_and_replayed_until_exact_ack() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let (sender, _) = generate_keypair();
        let frame = presence_frame(&fixture, &sender, NOW);
        let envelope = crate::collaboration_protocol::verify_collaboration_transport_frame(&frame)
            .unwrap()
            .envelope_bytes()
            .to_vec();
        let carrier = FakeCarrier::new();
        let driver = fixture.driver(core.clone(), carrier.clone()).await;
        // A separately opened Core reads the persisted acceptance before each send.
        carrier.observe_durable_incoming_before_send(fixture.core());
        let remote = serde_json::json!({"status":"ok","data":{"remote_peer_count":1}});
        carrier.push([
            peek(0, 1, vec![gossip(&frame)]),
            serde_json::json!({"status":"ok","broadcast":"local_only","data":{"remote_peer_count":0}}),
        ]);
        assert!(matches!(
            driver.process_incoming_once(NOW).await.unwrap(),
            CollaborationIncomingOnceOutcome::RetryRequired(_)
        ));
        assert_eq!(core.summary().unwrap().pending_product_handoffs, 1);
        core.acknowledge_product_handoff(&collaboration_message_envelope_sha256(&envelope))
            .unwrap();
        // Presence is suppressed in memory until its expiry, not tombstoned,
        // so after the ack its receipt replay is no longer a durable record.
        assert_eq!(core.summary().unwrap().replay_tombstones, 0);
        *carrier.durable_incoming_before_send.lock().unwrap() = None;

        carrier.push([peek(0, 1, vec![gossip(&frame)])]);
        carrier.push_failure("receipt send failed");
        assert!(matches!(
            driver.process_incoming_once(NOW + 1).await.unwrap(),
            CollaborationIncomingOnceOutcome::RetryRequired(_)
        ));
        carrier.push([peek(0, 1, vec![gossip(&frame)]), remote.clone()]);
        carrier.push_failure("ack failed");
        assert!(matches!(
            driver.process_incoming_once(NOW + 1).await.unwrap(),
            CollaborationIncomingOnceOutcome::RetryRequired(_)
        ));
        carrier.push([peek(0, 1, vec![gossip(&frame)]), remote, ack(0, 1)]);
        assert!(matches!(
            driver.process_incoming_once(NOW + 1).await.unwrap(),
            CollaborationIncomingOnceOutcome::Acknowledged(_)
        ));
        let receipts: Vec<Vec<u8>> = carrier
            .requests()
            .iter()
            .filter(|request| request["op"] == "gossip_send")
            .map(|request| request["message"].as_str().unwrap().as_bytes().to_vec())
            .collect();
        assert_eq!(receipts.len(), 4);
        assert!(receipts.windows(2).all(|pair| pair[0] == pair[1]));
        assert!(core.allows_gossip_receipt(&receipts[0], &frame).unwrap());
        assert_eq!(
            request_ops(&carrier),
            [
                "gossip_join_exact",
                "gossip_peek",
                "gossip_send",
                "gossip_peek",
                "gossip_send",
                "gossip_peek",
                "gossip_send",
                "gossip_ack",
                "gossip_peek",
                "gossip_send",
                "gossip_ack",
            ]
        );
    }

    #[tokio::test]
    async fn outgoing_observations_never_replace_verified_remote_acceptance() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let carrier = FakeCarrier::new();
        let (driver, presence, _authority) =
            granted_presence_driver(&fixture, core.clone(), carrier.clone()).await;
        let mut outgoing = Vec::new();
        for request in ["metadata-one", "metadata-two", "metadata-three"] {
            outgoing.push(queued_presence(&fixture, &presence, request).await);
        }
        for message in &outgoing {
            core.acknowledge_outgoing_product_projection(message.envelope_sha256())
                .unwrap();
        }
        let (recipient, _) = generate_keypair();
        let receipt = remote_receipt(&fixture, &outgoing[0], recipient.clone());
        let receipt = sign_collaboration_transport_frame(&recipient, &receipt).unwrap();
        carrier.push([
            serde_json::json!({"status":"ok","data":{"remote_peer_count":1}}),
            serde_json::json!({"status":"ok","broadcast":"local_only","data":{"remote_peer_count":0}}),
        ]);
        carrier.push_failure("metadata send failed");
        assert_eq!(
            driver.retry_outgoing_once(NOW).await.unwrap(),
            CollaborationOutgoingRetrySummary {
                attempted: 3,
                remote_broadcasts: 1,
                local_only_buffered: 1,
                send_failures: 1,
            }
        );
        assert_eq!(core.pending_outgoing(NOW).unwrap().len(), 3);
        carrier.push([peek(0, 1, vec![gossip(&receipt)]), ack(0, 1)]);
        assert_eq!(
            driver.process_incoming_once(NOW + 1).await.unwrap(),
            CollaborationIncomingOnceOutcome::Acknowledged(CollaborationIncomingOnceSummary {
                remote_acceptances: 1,
                ..CollaborationIncomingOnceSummary::default()
            })
        );
        assert_eq!(core.pending_outgoing(NOW + 1).unwrap().len(), 2);
        assert_eq!(
            request_ops(&carrier),
            [
                "gossip_join_exact",
                "gossip_send",
                "gossip_send",
                "gossip_send",
                "gossip_peek",
                "gossip_ack",
            ]
        );
    }

    #[tokio::test]
    async fn leave_and_cancellation_stop_owned_direct_attempts() {
        let a = Fixture::new();
        let b = peer_fixture(&a);
        let ac = a.core();
        let bc = b.core();
        let outgoing = prepare_outgoing(&ac, "cancel", serde_json::json!({"body":"pending"}));
        ac.acknowledge_outgoing_product_projection(outgoing.envelope_sha256())
            .unwrap();
        let (driver, plane, _) = home_driver(
            &a,
            ac.clone(),
            FakeCarrier::new(),
            &b,
            bc.clone(),
            false,
            true,
        )
        .await;
        let driver = Arc::new(driver);
        let running = driver.clone();
        let task = tokio::spawn(async move { running.retry_outgoing_once(NOW).await });
        tokio::time::timeout(Duration::from_secs(1), async {
            while plane.active.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(1), async {
            while plane.active.load(Ordering::SeqCst) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        ac.set_community_joined(false).unwrap();
        assert_eq!(driver.retry_outgoing_once(NOW).await.unwrap().attempted, 0);
        bc.set_community_joined(false).unwrap();
        let frame = ac
            .prepare_transport_frame(outgoing.envelope_bytes())
            .unwrap();
        assert!(bc
            .accept_home_chat(&frame, &ac.local_device_did(), NOW)
            .is_err());
        assert!(ac
            .pending_outgoing(NOW)
            .unwrap()
            .iter()
            .any(|message| message.envelope_bytes() == outgoing.envelope_bytes()));
    }
}
