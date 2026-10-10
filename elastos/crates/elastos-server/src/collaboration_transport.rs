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
}

const SHARED_FAST_RETRY_WINDOW_SECS: u64 = 3;
const SHARED_CUSTODY_FAST_RETRY: Duration = Duration::from_millis(500);
const SHARED_CUSTODY_RETRY: Duration = Duration::from_secs(5);
const MAX_CUSTODY_RETRIES_PER_CYCLE: usize = 4;
pub(crate) const SHARED_PROVIDER: &str = "collaboration.shared";
const SHARED_OP: &str = "deliver";
const HOME_DELIVERY_TIMEOUT: Duration = Duration::from_secs(2);

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
        if serde_json::to_vec(value)?.len() > MAX_COLLABORATION_TRANSPORT_FRAME_BYTES * 6 + 2048 {
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
                if serde_json::to_vec(&response)?.len()
                    > MAX_COLLABORATION_TRANSPORT_FRAME_BYTES * 6 + 2048
                {
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

    pub(crate) async fn retry_outgoing_once(
        &self,
        now: u64,
    ) -> anyhow::Result<CollaborationOutgoingRetrySummary> {
        let metadata = async {
            match tokio::time::timeout(SHARED_CUSTODY_FAST_RETRY, self.retry_metadata_once(now))
                .await
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
        // Metadata cannot hold a Home attempt behind a gossip response. Both
        // branches remain owned by this existing worker future and cancel with it.
        let (metadata, homes) = tokio::join!(metadata, homes);
        let mut summary = metadata?;
        let homes = homes?;
        summary.attempted += homes.attempted;
        summary.remote_broadcasts += homes.remote_broadcasts;
        summary.send_failures += homes.send_failures;
        Ok(summary)
    }

    async fn retry_metadata_once(
        &self,
        now: u64,
    ) -> anyhow::Result<CollaborationOutgoingRetrySummary> {
        if !self.core.community_membership().joined() {
            return Ok(CollaborationOutgoingRetrySummary::default());
        }
        let selected = self
            .core
            .pending_outgoing(now)?
            .into_iter()
            .filter(|message| !message.shared_chat());
        let mut summary = CollaborationOutgoingRetrySummary::default();
        for outgoing in selected {
            self.core.community_membership().require_joined()?;
            summary.attempted += 1;
            let frame = self
                .core
                .prepare_transport_frame(outgoing.envelope_bytes())?;
            match self.network.send_metadata(&self.core, &frame, None).await {
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
            // Held Shared work was admitted only by the authenticated direct
            // operation. Its sender retries that operation for the receipt.
            if !self
                .ingest_frame(frame.frame(), now, &mut summary, true)
                .await
            {
                self.core
                    .restore_held_frames(std::iter::once(frame).chain(held).collect());
                return Ok(CollaborationIncomingOnceOutcome::RetryRequired(summary));
            }
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
        held_direct: bool,
    ) -> bool {
        match self.core.allows_gossip_frame(frame) {
            Ok(true) => {}
            Ok(false) if held_direct => {
                match self.core.ingest_transport_frame(frame, now) {
                    Ok(CollaborationTransportIngestion::Incoming(_)) => {
                        summary.incoming_acceptances += 1
                    }
                    Ok(CollaborationTransportIngestion::Rejected(_)) => {
                        summary.deterministic_rejections += 1
                    }
                    _ => return false,
                }
                return true;
            }
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
                    .send_metadata(&self.core, &receipt_frame, Some(frame))
                    .await
                {
                    Ok(CollaborationCarrierSendOutcome::RemoteBroadcast { .. }) => {
                        summary.acceptance_receipt_broadcasts += 1;
                        true
                    }
                    Ok(CollaborationCarrierSendOutcome::LocalOnlyBuffered) | Err(_) => false,
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
    use crate::collaboration_protocol::sign_collaboration_transport_frame;
    use crate::esp_binding::{esp_request_binding, EspRequestBinding};
    use elastos_common::collaboration_protocol::{
        collaboration_message_envelope_sha256, SignedCollaborationMessage,
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
        replies: Mutex<VecDeque<serde_json::Value>>,
        blocked_metadata: AtomicBool,
    }
    impl FakeCarrier {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                requests: Mutex::new(Vec::new()),
                replies: Mutex::new(VecDeque::new()),
                blocked_metadata: AtomicBool::new(false),
            })
        }
        fn push(&self, replies: impl IntoIterator<Item = serde_json::Value>) {
            self.replies.lock().unwrap().extend(replies);
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
                .ok_or_else(|| ProviderError::Unavailable("offline".to_string()))
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
            carrier: Arc<FakeCarrier>,
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
                serde_json::json!({"content": content}),
                NOW,
                TTL,
            )
            .unwrap();
        (key, prepared.envelope_bytes().to_vec())
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
    ) -> (CollaborationTransportDriver, Arc<Plane>) {
        let registry = Arc::new(ProviderRegistry::new());
        let presence = CollaborationPresenceProductPort::new(core.clone()).unwrap();
        observe(fixture, &core, &presence, peer);
        let plane = Arc::new(Plane {
            source: core.local_device_did(),
            peers: BTreeMap::from([(
                peer_core.local_device_did(),
                delivery(peer_core, Arc::new(ProviderRegistry::new())),
            )]),
            calls: Mutex::new(Vec::new()),
            lose_reply: AtomicBool::new(lose_reply),
            stall,
            active: Arc::new(AtomicUsize::new(0)),
        });
        registry.set_carrier_invoker(plane.clone()).await;
        let driver = fixture
            .driver(core, carrier)
            .await
            .with_home_delivery(presence, registry)
            .await
            .unwrap();
        (driver, plane)
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
            serde_json::json!({"content":"one original"}),
        );
        ac.acknowledge_outgoing_product_projection(outgoing.envelope_sha256())
            .unwrap();
        let (driver, plane) =
            home_driver(&a, ac.clone(), carrier.clone(), &b, bc.clone(), true, false).await;
        assert_eq!(
            driver.retry_outgoing_once(NOW).await.unwrap().send_failures,
            1
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
        assert!(carrier
            .requests()
            .iter()
            .all(|request| request["op"] != "gossip_send"));
        assert!(ac.pending_outgoing(NOW + TTL).unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn blocked_metadata_cannot_delay_first_direct_attempt_or_lost_reply_retry() {
        let a = Fixture::new();
        let b = peer_fixture(&a);
        let ac = a.core();
        let bc = b.core();
        let profile = a
            .authority(a.device_key.clone())
            .sender_profile_for_test()
            .unwrap();
        let kind = crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE;
        ac.prepare_profile_outgoing(
            operation(&profile, "local-metadata", kind, &serde_json::json!({}), 45),
            &profile,
            kind,
            serde_json::json!({}),
            NOW,
            45,
        )
        .unwrap();
        let outgoing = prepare_outgoing(&ac, "Chat", serde_json::json!({"content":"direct"}));
        ac.acknowledge_outgoing_product_projection(outgoing.envelope_sha256())
            .unwrap();
        let carrier = FakeCarrier::new();
        carrier.blocked_metadata.store(true, Ordering::SeqCst);
        let (driver, plane) =
            home_driver(&a, ac.clone(), carrier, &b, bc.clone(), true, false).await;
        let driver = Arc::new(driver);
        for count in [1, 2] {
            let running = driver.clone();
            let cycle = tokio::spawn(async move { running.retry_outgoing_once(NOW + 1).await });
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while plane.calls.lock().unwrap().len() < count {
                assert!(
                    std::time::Instant::now() < deadline,
                    "direct attempt waited for blocked metadata"
                );
                tokio::task::yield_now().await;
            }
            assert!(
                !cycle.is_finished(),
                "metadata must still be pending at direct dispatch"
            );
            tokio::time::advance(SHARED_CUSTODY_FAST_RETRY).await;
            cycle.await.unwrap().unwrap();
        }
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
        let (driver, plane) = home_driver(&a, ac.clone(), carrier, &b, bc, false, false).await;
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
                serde_json::json!({"content":format!("m{i}")}),
            );
            ac.acknowledge_outgoing_product_projection(out.envelope_sha256())
                .unwrap();
            hashes.insert(out.envelope_sha256().to_string());
        }
        for expected in [4, 4, 1] {
            let result = driver.retry_outgoing_once(NOW + 3).await.unwrap();
            assert_eq!(result.attempted, expected);
            assert_eq!(result.send_failures, expected);
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
            4
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
    async fn shared_gossip_and_forged_provider_source_are_refused_before_durable_admission() {
        let a = Fixture::new();
        let ac = a.core();
        let (sender, _) = generate_keypair();
        let (_, message) = remote_message(&a, sender.clone(), "legacy Chat");
        let frame = sign_collaboration_transport_frame(&sender, &message).unwrap();
        let carrier = FakeCarrier::new();
        let driver = a.driver(ac.clone(), carrier.clone()).await;
        carrier.push([serde_json::json!({"status":"ok","data":{"messages":[{"content":std::str::from_utf8(&frame).unwrap()}],"scanned":1,"limit":32,"cursor":0,"next_cursor":1}}), serde_json::json!({"status":"ok","data":{"cursor":0,"next_cursor":1,"advanced":true}})]);
        let result = driver.process_incoming_once(NOW).await.unwrap();
        assert!(
            matches!(result, CollaborationIncomingOnceOutcome::Acknowledged(summary) if summary.deterministic_rejections == 1 && summary.incoming_acceptances == 0)
        );
        assert_eq!(ac.summary().unwrap().pending_product_handoffs, 0);
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
        let count = carrier.requests().len();
        assert!(driver
            .network
            .send_metadata(&ac, &frame, None)
            .await
            .is_err());
        assert_eq!(carrier.requests().len(), count);
        let response = service
            .receive(
                &invocation(&frame, &crate::crypto::encode_signing_key_did(&sender)),
                NOW,
            )
            .unwrap();
        let receipt = ac
            .prepare_transport_frame(response["receipt"].as_str().unwrap().as_bytes())
            .unwrap();
        assert!(driver
            .network
            .send_metadata(&ac, &receipt, Some(&frame))
            .await
            .is_err());
        assert_eq!(carrier.requests().len(), count);
    }

    #[tokio::test]
    async fn seed_sender_receiver_presence_and_receipt_are_excluded_from_chat_plane() {
        let (seed, _) = generate_keypair();
        let mut fixture = Fixture::new();
        let key = crate::carrier::did_to_public_key(&crate::crypto::encode_signing_key_did(&seed))
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
        let core = fixture.core();
        let mut seed_fixture = peer_fixture(&fixture);
        seed_fixture.device_key = seed.clone();
        let seed_core = seed_fixture.core();
        assert!(core.is_bootstrap_endpoint(&seed_core.local_device_did()));
        let outgoing = prepare_outgoing(
            &core,
            "seed-refusal",
            serde_json::json!({"content":"Home only"}),
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

    #[tokio::test]
    async fn held_direct_message_retries_without_sender_and_storage_failure_retains_metadata_batch()
    {
        let fixture = Fixture::new();
        let core = fixture.core();
        let (sender, _) = generate_keypair();
        let source = crate::crypto::encode_signing_key_did(&sender);
        let backlog = crate::collaboration_core::MAX_PENDING_INCOMING_PER_SENDER;
        for i in 0..=backlog {
            let (_, message) = remote_message(&fixture, sender.clone(), &format!("held-{i}"));
            let wire = sign_collaboration_transport_frame(&sender, &message).unwrap();
            let accepted = core.accept_home_chat(&wire, &source, NOW);
            assert_eq!(accepted.is_ok(), i < backlog);
        }
        for handoff in core.pending_product_handoffs().unwrap() {
            core.acknowledge_product_handoff(
                handoff.authorized_message().message().envelope_sha256(),
            )
            .unwrap();
        }
        let carrier = FakeCarrier::new();
        let driver = fixture.driver(core.clone(), carrier.clone()).await;
        carrier.push([serde_json::json!({"status":"ok","data":{"messages":[],"scanned":0,"limit":32,"cursor":0,"next_cursor":0}}), serde_json::json!({"status":"ok","data":{"cursor":0,"next_cursor":0,"advanced":false}})]);
        let result = driver
            .process_incoming_once(
                NOW + crate::collaboration_rate_limit::COMMUNITY_RATE_WINDOW_SECS,
            )
            .await
            .unwrap();
        assert!(
            matches!(result, CollaborationIncomingOnceOutcome::Acknowledged(summary) if summary.incoming_acceptances == 1 && summary.acceptance_receipt_broadcasts == 0)
        );
        assert_eq!(core.pending_product_handoffs().unwrap().len(), 1);
        assert!(carrier
            .requests()
            .iter()
            .all(|request| request["op"] != "gossip_send"));
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
    async fn mixed_metadata_batch_retries_storage_and_consumes_legacy_chat_and_malformed_frames() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let (key, _) = generate_keypair();
        let (_, legacy) = remote_message(&fixture, key.clone(), "legacy Chat");
        let legacy = sign_collaboration_transport_frame(&key, &legacy).unwrap();
        let metadata = presence_frame(&fixture, &key, NOW);
        let frames = vec![gossip(b"{"), gossip(&legacy), gossip(&metadata)];
        let carrier = FakeCarrier::new();
        let driver = fixture.driver(core.clone(), carrier.clone()).await;
        carrier.push([
            peek(0, 3, frames.clone()),
            peek(0, 3, frames),
            serde_json::json!({"status":"ok","data":{"remote_peer_count":1}}),
            ack(0, 3),
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
                carrier_rejected_frames: 1,
                deterministic_rejections: 1,
                incoming_acceptances: 1,
                acceptance_receipt_broadcasts: 1,
                ..CollaborationIncomingOnceSummary::default()
            })
        );
        assert_eq!(core.pending_product_handoffs().unwrap().len(), 1);
        let requests = carrier.requests();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request["op"] == "gossip_ack")
                .count(),
            1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| request["op"] == "gossip_send")
                .count(),
            1
        );
        let receipt = requests
            .iter()
            .find(|request| request["op"] == "gossip_send")
            .unwrap();
        assert!(core
            .allows_gossip_receipt(receipt["message"].as_str().unwrap().as_bytes(), &metadata)
            .unwrap());
    }

    #[tokio::test]
    async fn empty_metadata_batch_acknowledges_without_product_or_send_effects() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let carrier = FakeCarrier::new();
        let driver = fixture.driver(core.clone(), carrier.clone()).await;
        carrier.push([peek(0, 0, Vec::new()), ack(0, 0)]);
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
    }

    #[tokio::test]
    async fn leave_and_cancellation_stop_owned_direct_attempts() {
        let a = Fixture::new();
        let b = peer_fixture(&a);
        let ac = a.core();
        let bc = b.core();
        let outgoing = prepare_outgoing(&ac, "cancel", serde_json::json!({"content":"pending"}));
        ac.acknowledge_outgoing_product_projection(outgoing.envelope_sha256())
            .unwrap();
        let (driver, plane) = home_driver(
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
