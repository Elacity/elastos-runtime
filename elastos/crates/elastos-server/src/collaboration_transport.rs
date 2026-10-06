//! Bounded Runtime driver for one durable collaboration core and Carrier subscription.

use std::sync::Arc;

use crate::collaboration_carrier::{CollaborationCarrierSendOutcome, JoinedCollaborationNetwork};
use crate::collaboration_core::{CollaborationCore, CollaborationTransportIngestion};

pub(crate) struct CollaborationTransportDriver {
    core: Arc<CollaborationCore>,
    network: JoinedCollaborationNetwork,
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
        Self { core, network }
    }

    pub(crate) async fn retry_outgoing_once(
        &self,
        now: u64,
    ) -> anyhow::Result<CollaborationOutgoingRetrySummary> {
        let pending = self.core.pending_outgoing(now)?;
        let mut summary = CollaborationOutgoingRetrySummary::default();
        for outgoing in pending {
            summary.attempted += 1;
            let frame = self
                .core
                .prepare_transport_frame(outgoing.envelope_bytes())?;
            match self.network.send(&frame).await {
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

        // Frames this Home refused under a per-sender limit come first. Their
        // senders may never resend them, so this Home owns the retry.
        let mut held = self.core.take_held_frames(now).into_iter();
        while let Some(frame) = held.next() {
            if !self.ingest_frame(frame.frame(), now, &mut summary).await {
                self.core
                    .restore_held_frames(std::iter::once(frame).chain(held).collect());
                return Ok(CollaborationIncomingOnceOutcome::RetryRequired(summary));
            }
        }

        let batch = self.network.peek().await?;
        summary.carrier_rejected_frames = batch.rejected_frames();
        for envelope in batch.envelopes() {
            if !self.ingest_frame(envelope, now, &mut summary).await {
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
    ) -> bool {
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
                match self.network.send(accepted.acceptance_receipt_bytes()).await {
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

    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    use elastos_common::collaboration_protocol::{
        canonical_collaboration_message_bytes, canonical_signed_collaboration_message_bytes,
        collaboration_message_envelope_sha256, SignedCollaborationMessage,
        COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
    };
    use elastos_runtime::provider::{Provider, ProviderError, ResourceRequest, ResourceResponse};
    use elastos_runtime::signature::{generate_keypair, SigningKey};
    use sha2::{Digest, Sha256};

    use crate::collaboration_carrier::join_collaboration_network;
    use crate::collaboration_core::{CollaborationTransportIngestion, WriteFault};
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

    const NETWORK: &str = "collaboration-transport-test";
    const CONVERSATION: &str = "default-conversation";
    const SERVICE: &str = "chat";
    const OPERATION_CAPSULE: &str = "chat-room";
    const NOW: u64 = 1_800_000_000;
    const TTL: u64 = 300;

    enum FakeReply {
        JoinEcho,
        Value(serde_json::Value),
        Error(&'static str),
    }

    struct FakeCarrier {
        requests: Mutex<Vec<serde_json::Value>>,
        replies: Mutex<VecDeque<FakeReply>>,
        observed_core: Mutex<Option<Arc<CollaborationCore>>>,
        require_durable_incoming_on_send: AtomicBool,
    }

    impl FakeCarrier {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                requests: Mutex::new(Vec::new()),
                replies: Mutex::new(VecDeque::from([FakeReply::JoinEcho])),
                observed_core: Mutex::new(None),
                require_durable_incoming_on_send: AtomicBool::new(false),
            })
        }

        fn push(&self, replies: impl IntoIterator<Item = FakeReply>) {
            self.replies.lock().unwrap().extend(replies);
        }

        fn requests(&self) -> Vec<serde_json::Value> {
            self.requests.lock().unwrap().clone()
        }

        fn observe_durable_incoming_before_send(&self, core: Arc<CollaborationCore>) {
            *self.observed_core.lock().unwrap() = Some(core);
            self.require_durable_incoming_on_send
                .store(true, Ordering::SeqCst);
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
            "fake-collaboration-transport-carrier"
        }

        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            self.requests.lock().unwrap().push(request.clone());
            if request["op"] == "gossip_send"
                && self.require_durable_incoming_on_send.load(Ordering::SeqCst)
            {
                let core = self.observed_core.lock().unwrap().clone().unwrap();
                let summary = core.summary().unwrap();
                assert_eq!(
                    summary.pending_product_handoffs + summary.replay_tombstones,
                    1,
                    "receipt send happened before durable incoming state"
                );
            }
            match self.replies.lock().unwrap().pop_front() {
                Some(FakeReply::JoinEcho) => Ok(serde_json::json!({
                    "status": "ok",
                    "data": {"topic": request["topic"]},
                })),
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
            let profile = verified_profile(&profile_signer, raw_sha256_cid(&grant_bytes));
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
    ) -> VerifiedCollaborationNetworkProfile {
        let signer_did = crate::crypto::encode_signing_key_did(signing_key);
        let payload = CollaborationNetworkProfile {
            schema: COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
            network_id: NETWORK.to_string(),
            revision: 1,
            previous_profile_sha256: None,
            signer_did: signer_did.clone(),
            bootstrap_peers: Vec::new(),
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

    fn conflicting_message(original: &[u8], key: &SigningKey) -> Vec<u8> {
        let mut envelope: SignedCollaborationMessage = serde_json::from_slice(original).unwrap();
        envelope.payload.payload = serde_json::json!({"content":"conflict"});
        let payload_bytes = canonical_collaboration_message_bytes(&envelope.payload).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            key,
            COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
            &payload_bytes,
        );
        envelope.signature = signature;
        envelope.signer_did = signer_did;
        canonical_signed_collaboration_message_bytes(&envelope).unwrap()
    }

    fn transport_frame(key: &SigningKey, envelope: &[u8]) -> Vec<u8> {
        sign_collaboration_transport_frame(key, envelope).unwrap()
    }

    fn send_remote() -> FakeReply {
        FakeReply::Value(serde_json::json!({
            "status": "ok",
            "data": {"remote_peer_count": 1},
        }))
    }

    fn send_local_only() -> FakeReply {
        FakeReply::Value(serde_json::json!({
            "status": "ok",
            "broadcast": "local_only",
            "data": {"remote_peer_count": 1},
        }))
    }

    fn frame(envelope: &[u8]) -> serde_json::Value {
        serde_json::json!({
            "content": std::str::from_utf8(envelope).unwrap(),
        })
    }

    fn peek(cursor: u64, next_cursor: u64, messages: Vec<serde_json::Value>) -> FakeReply {
        let scanned = messages.len();
        FakeReply::Value(serde_json::json!({
            "status": "ok",
            "data": {
                "messages": messages,
                "scanned": scanned,
                "limit": 32,
                "cursor": cursor,
                "next_cursor": next_cursor,
            },
        }))
    }

    fn ack(cursor: u64, next_cursor: u64, advanced: bool) -> FakeReply {
        FakeReply::Value(serde_json::json!({
            "status": "ok",
            "data": {
                "cursor": cursor,
                "next_cursor": next_cursor,
                "advanced": advanced,
            },
        }))
    }

    fn request_ops(carrier: &FakeCarrier) -> Vec<String> {
        carrier
            .requests()
            .iter()
            .map(|request| request["op"].as_str().unwrap().to_string())
            .collect()
    }

    #[tokio::test]
    async fn outgoing_observations_never_replace_verified_remote_acceptance() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let first = prepare_outgoing(&core, "request-one", serde_json::json!({"text":"one"}));
        let second = prepare_outgoing(&core, "request-two", serde_json::json!({"text":"two"}));
        let third = prepare_outgoing(&core, "request-three", serde_json::json!({"text":"three"}));
        for outgoing in [&first, &second, &third] {
            core.acknowledge_outgoing_product_projection(outgoing.envelope_sha256())
                .unwrap();
        }
        let (receipt_key, _) = generate_keypair();
        let receipt = remote_receipt(&fixture, &first, receipt_key.clone());
        let carrier = FakeCarrier::new();
        let driver = fixture.driver(core.clone(), carrier.clone()).await;
        carrier.push([
            send_remote(),
            send_local_only(),
            FakeReply::Error("send failed"),
            peek(0, 1, vec![frame(&transport_frame(&receipt_key, &receipt))]),
            ack(0, 1, true),
        ]);

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
    async fn incoming_receipt_is_durable_before_send_and_replayed_until_exact_ack() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let (remote_key, _) = generate_keypair();
        let (_, incoming) = remote_message(&fixture, remote_key.clone(), "incoming");
        let incoming_frame = transport_frame(&remote_key, &incoming);
        let carrier = FakeCarrier::new();
        let driver = fixture.driver(core.clone(), carrier.clone()).await;
        carrier.observe_durable_incoming_before_send(core.clone());
        carrier.push([
            peek(0, 1, vec![frame(&incoming_frame)]),
            send_local_only(),
            peek(0, 1, vec![frame(&incoming_frame)]),
            FakeReply::Error("receipt send failed"),
            peek(0, 1, vec![frame(&incoming_frame)]),
            send_remote(),
            FakeReply::Error("ack failed"),
            peek(0, 1, vec![frame(&incoming_frame)]),
            send_remote(),
            ack(0, 1, true),
        ]);

        assert!(matches!(
            driver.process_incoming_once(NOW).await.unwrap(),
            CollaborationIncomingOnceOutcome::RetryRequired(_)
        ));
        assert_eq!(core.summary().unwrap().pending_product_handoffs, 1);
        let envelope_hash = collaboration_message_envelope_sha256(&incoming);
        core.acknowledge_product_handoff(&envelope_hash).unwrap();
        assert_eq!(core.summary().unwrap().replay_tombstones, 1);

        for _ in 0..2 {
            assert!(matches!(
                driver.process_incoming_once(NOW + 1).await.unwrap(),
                CollaborationIncomingOnceOutcome::RetryRequired(_)
            ));
        }
        assert!(matches!(
            driver.process_incoming_once(NOW + 1).await.unwrap(),
            CollaborationIncomingOnceOutcome::Acknowledged(_)
        ));

        let requests = carrier.requests();
        let receipt_sends: Vec<Vec<u8>> = requests
            .iter()
            .filter(|request| request["op"] == "gossip_send")
            .map(|request| request["message"].as_str().unwrap().as_bytes().to_vec())
            .collect();
        assert_eq!(receipt_sends.len(), 4);
        assert!(receipt_sends
            .windows(2)
            .all(|receipts| receipts[0] == receipts[1]));
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
    async fn mixed_batch_waits_for_retryable_core_work_but_consumes_deterministic_rejections() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let (conflict_key, _) = generate_keypair();
        let (conflict_key, original) = remote_message(&fixture, conflict_key, "original");
        let original_frame = transport_frame(&conflict_key, &original);
        assert!(matches!(
            core.ingest_transport_frame(&original_frame, NOW).unwrap(),
            CollaborationTransportIngestion::Incoming(_)
        ));
        core.acknowledge_product_handoff(&collaboration_message_envelope_sha256(&original))
            .unwrap();
        let conflict = conflicting_message(&original, &conflict_key);
        let self_message =
            prepare_outgoing(&core, "self-message", serde_json::json!({"text":"self"}));
        let (valid_key, _) = generate_keypair();
        let (_, valid) = remote_message(&fixture, valid_key.clone(), "valid");
        let unknown = serde_json::to_vec(&serde_json::json!({
            "payload": {"schema": "elastos.collaboration.unknown/v1"}
        }))
        .unwrap();
        let frames = vec![
            serde_json::json!({"content":"not-base64"}),
            frame(b"{"),
            frame(&transport_frame(&conflict_key, &unknown)),
            frame(
                &core
                    .prepare_transport_frame(self_message.envelope_bytes())
                    .unwrap(),
            ),
            frame(&transport_frame(&conflict_key, &conflict)),
            frame(&transport_frame(&valid_key, &valid)),
        ];
        let carrier = FakeCarrier::new();
        let driver = fixture.driver(core.clone(), carrier.clone()).await;
        carrier.push([
            peek(0, 6, frames.clone()),
            peek(0, 6, frames),
            send_remote(),
            ack(0, 6, true),
        ]);
        core.inject_write_fault(WriteFault::BeforeWrite);

        assert!(matches!(
            driver.process_incoming_once(NOW + 1).await.unwrap(),
            CollaborationIncomingOnceOutcome::RetryRequired(_)
        ));
        assert!(!request_ops(&carrier).contains(&"gossip_ack".to_string()));

        assert_eq!(
            driver.process_incoming_once(NOW + 1).await.unwrap(),
            CollaborationIncomingOnceOutcome::Acknowledged(CollaborationIncomingOnceSummary {
                carrier_rejected_frames: 2,
                deterministic_rejections: 3,
                incoming_acceptances: 1,
                acceptance_receipt_broadcasts: 1,
                ..CollaborationIncomingOnceSummary::default()
            })
        );
        assert_eq!(core.summary().unwrap().pending_product_handoffs, 1);
        let ops = request_ops(&carrier);
        assert_eq!(ops.iter().filter(|op| *op == "gossip_ack").count(), 1);
        assert!(!ops.iter().any(|op| op == "gossip_recv"));
    }

    fn project_pending(core: &CollaborationCore) {
        for handoff in core.pending_product_handoffs().unwrap() {
            core.acknowledge_product_handoff(
                handoff.authorized_message().message().envelope_sha256(),
            )
            .unwrap();
        }
    }

    fn pending_contains(core: &CollaborationCore, envelope: &[u8]) -> bool {
        let hash = collaboration_message_envelope_sha256(envelope);
        core.pending_product_handoffs()
            .unwrap()
            .iter()
            .any(|handoff| handoff.authorized_message().message().envelope_sha256() == hash)
    }

    fn summary(rejected: usize, accepted: usize) -> CollaborationIncomingOnceOutcome {
        CollaborationIncomingOnceOutcome::Acknowledged(CollaborationIncomingOnceSummary {
            deterministic_rejections: rejected,
            incoming_acceptances: accepted,
            acceptance_receipt_broadcasts: accepted,
            ..CollaborationIncomingOnceSummary::default()
        })
    }

    #[tokio::test]
    async fn held_message_lands_without_the_sender_resending_it() {
        let backlog = crate::collaboration_core::MAX_PENDING_INCOMING_PER_SENDER;
        let fixture = Fixture::new();
        let core = fixture.core();
        let (sender, _) = generate_keypair();
        // A delayed worker sees nine honest messages in one batch.
        let envelopes: Vec<Vec<u8>> = (0..=backlog)
            .map(|index| remote_message(&fixture, sender.clone(), &format!("m{index}")).1)
            .collect();
        let frames = envelopes
            .iter()
            .map(|envelope| frame(&transport_frame(&sender, envelope)))
            .collect::<Vec<_>>();
        let carrier = FakeCarrier::new();
        let driver = fixture.driver(core.clone(), carrier.clone()).await;
        let mut replies = vec![peek(0, frames.len() as u64, frames)];
        replies.extend((0..backlog).map(|_| send_remote()));
        replies.push(ack(0, envelopes.len() as u64, true));
        // Another Home accepted the last message, so its sender never resends
        // it: the next batch is empty.
        replies.extend([send_remote(), peek(9, 9, Vec::new()), ack(9, 9, false)]);
        carrier.push(replies);

        assert_eq!(
            driver.process_incoming_once(NOW).await.unwrap(),
            summary(1, backlog)
        );
        assert!(!pending_contains(&core, envelopes.last().unwrap()));
        project_pending(&core);
        assert_eq!(
            driver.process_incoming_once(NOW + 5).await.unwrap(),
            summary(0, 1)
        );
        assert!(pending_contains(&core, envelopes.last().unwrap()));
    }

    #[tokio::test]
    async fn honest_message_keeps_its_place_when_flooders_fill_the_held_queue() {
        let backlog = crate::collaboration_core::MAX_PENDING_INCOMING_PER_SENDER;
        let fixture = Fixture::new();
        let held_bound = 16;
        let core = Arc::new(
            Arc::into_inner(fixture.core())
                .unwrap()
                .with_held_frame_bound_for_test(held_bound),
        );
        // Two flooding Profiles fill their backlogs and the Home's held queue
        // (10 + 6 frames). Chat never drains them.
        let mut flood = Vec::new();
        for (flooder, extra) in [("a", 10), ("b", 6)] {
            let (key, _) = generate_keypair();
            for index in 0..backlog + extra {
                let (_, envelope) =
                    remote_message(&fixture, key.clone(), &format!("flood {flooder}/{index}"));
                flood.push(frame(&transport_frame(&key, &envelope)));
            }
        }
        let (honest, _) = generate_keypair();
        let honest_envelopes: Vec<Vec<u8>> = (0..=backlog)
            .map(|index| remote_message(&fixture, honest.clone(), &format!("honest {index}")).1)
            .collect();
        let honest_frames: Vec<_> = honest_envelopes
            .iter()
            .map(|envelope| frame(&transport_frame(&honest, envelope)))
            .collect();

        let carrier = FakeCarrier::new();
        let driver = fixture.driver(core.clone(), carrier.clone()).await;
        let flood_end = flood.len() as u64;
        let honest_end = flood_end + honest_frames.len() as u64;
        carrier.push([peek(0, flood_end, flood)]);
        carrier.push((0..2 * backlog).map(|_| send_remote()));
        carrier.push([ack(0, flood_end, true)]);
        carrier.push([peek(flood_end, honest_end, honest_frames)]);
        carrier.push((0..backlog).map(|_| send_remote()));
        carrier.push([ack(flood_end, honest_end, true)]);
        // The sender never resends the ninth message.
        carrier.push([
            send_remote(),
            peek(honest_end, honest_end, Vec::new()),
            ack(honest_end, honest_end, false),
        ]);

        assert_eq!(
            driver.process_incoming_once(NOW).await.unwrap(),
            summary(held_bound, 2 * backlog)
        );
        let held = core.take_held_frames(NOW);
        assert_eq!(held.len(), held_bound);
        core.restore_held_frames(held);
        // The ninth honest message takes a slot from the largest holder.
        assert_eq!(
            driver.process_incoming_once(NOW).await.unwrap(),
            summary(held_bound + 1, backlog)
        );

        // Chat shows the honest messages only; the flooders' stay waiting.
        let honest_hashes: Vec<String> = honest_envelopes
            .iter()
            .map(|envelope| collaboration_message_envelope_sha256(envelope))
            .collect();
        for handoff in core.pending_product_handoffs().unwrap() {
            let hash = handoff.authorized_message().message().envelope_sha256();
            if honest_hashes.iter().any(|honest| honest == hash) {
                core.acknowledge_product_handoff(hash).unwrap();
            }
        }
        assert_eq!(
            driver.process_incoming_once(NOW + 5).await.unwrap(),
            summary(held_bound - 1, 1)
        );
        assert!(pending_contains(&core, honest_envelopes.last().unwrap()));
    }

    #[tokio::test]
    async fn flooding_sender_is_held_back_without_holding_back_other_senders() {
        use crate::collaboration_rate_limit::{
            COMMUNITY_RATE_WINDOW_SECS, COMMUNITY_RECEIVES_PER_SENDER_PER_WINDOW,
        };
        let backlog = crate::collaboration_core::MAX_PENDING_INCOMING_PER_SENDER;
        let fixture = Fixture::new();
        let core = fixture.core();
        let (flooder, _) = generate_keypair();
        let flood: Vec<Vec<u8>> = (0..=COMMUNITY_RECEIVES_PER_SENDER_PER_WINDOW)
            .map(|index| {
                let (_, envelope) =
                    remote_message(&fixture, flooder.clone(), &format!("flood {index}"));
                transport_frame(&flooder, &envelope)
            })
            .collect();
        let (other_key, _) = generate_keypair();
        let (_, other) = remote_message(&fixture, other_key.clone(), "other");

        // One batch, no Chat projection between frames, as in production.
        let carrier = FakeCarrier::new();
        let driver = fixture.driver(core.clone(), carrier.clone()).await;
        let mut frames: Vec<_> = flood.iter().map(|bytes| frame(bytes)).collect();
        frames.push(frame(&transport_frame(&other_key, &other)));
        let end = frames.len() as u64;
        let mut replies = vec![peek(0, end, frames)];
        replies.extend((0..=backlog).map(|_| send_remote()));
        replies.push(ack(0, end, true));
        // Next cycle: the held frames fit up to the receive limit.
        let within_limit = COMMUNITY_RECEIVES_PER_SENDER_PER_WINDOW - backlog;
        replies.extend((0..within_limit).map(|_| send_remote()));
        replies.extend([peek(end, end, Vec::new()), ack(end, end, false)]);
        // Once the window slides, the last one lands.
        replies.extend([
            send_remote(),
            peek(end, end, Vec::new()),
            ack(end, end, false),
        ]);
        carrier.push(replies);

        let held = flood.len() - backlog;
        assert_eq!(
            driver.process_incoming_once(NOW).await.unwrap(),
            summary(held, backlog + 1)
        );
        assert!(pending_contains(&core, &other));
        project_pending(&core);
        assert_eq!(
            driver.process_incoming_once(NOW + 1).await.unwrap(),
            summary(held - within_limit, within_limit)
        );
        project_pending(&core);
        assert_eq!(
            driver
                .process_incoming_once(NOW + COMMUNITY_RATE_WINDOW_SECS)
                .await
                .unwrap(),
            summary(0, 1)
        );
        assert!(core
            .take_held_frames(NOW + COMMUNITY_RATE_WINDOW_SECS)
            .is_empty());
    }

    #[tokio::test]
    async fn empty_batch_is_acknowledged_without_product_or_transport_effects() {
        let fixture = Fixture::new();
        let core = fixture.core();
        let carrier = FakeCarrier::new();
        let driver = fixture.driver(core, carrier.clone()).await;
        carrier.push([peek(7, 7, Vec::new()), ack(7, 7, false)]);

        assert_eq!(
            driver.process_incoming_once(NOW).await.unwrap(),
            CollaborationIncomingOnceOutcome::Acknowledged(
                CollaborationIncomingOnceSummary::default()
            )
        );
        assert_eq!(
            request_ops(&carrier),
            ["gossip_join_exact", "gossip_peek", "gossip_ack"]
        );
    }
}
