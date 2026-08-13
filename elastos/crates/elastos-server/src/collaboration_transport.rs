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
        let batch = self.network.peek().await?;
        let mut summary = CollaborationIncomingOnceSummary {
            carrier_rejected_frames: batch.rejected_frames(),
            ..CollaborationIncomingOnceSummary::default()
        };

        for envelope in batch.envelopes() {
            match self.core.ingest_transport_frame(envelope, now) {
                Err(_) => {
                    return Ok(CollaborationIncomingOnceOutcome::RetryRequired(summary));
                }
                Ok(CollaborationTransportIngestion::Rejected(_)) => {
                    summary.deterministic_rejections += 1;
                }
                Ok(CollaborationTransportIngestion::RemoteAcceptance(_)) => {
                    summary.remote_acceptances += 1;
                }
                Ok(CollaborationTransportIngestion::Incoming(accepted)) => {
                    summary.incoming_acceptances += 1;
                    match self.network.send(accepted.acceptance_receipt_bytes()).await {
                        Ok(CollaborationCarrierSendOutcome::RemoteBroadcast { .. }) => {
                            summary.acceptance_receipt_broadcasts += 1;
                        }
                        Ok(CollaborationCarrierSendOutcome::LocalOnlyBuffered) | Err(_) => {
                            return Ok(CollaborationIncomingOnceOutcome::RetryRequired(summary));
                        }
                    }
                }
            }
        }

        match self.network.ack(&batch).await {
            Ok(()) => Ok(CollaborationIncomingOnceOutcome::Acknowledged(summary)),
            Err(_) => Ok(CollaborationIncomingOnceOutcome::RetryRequired(summary)),
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
        let signer_did = crate::crypto::encode_signing_key_did(&signing_key);
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
