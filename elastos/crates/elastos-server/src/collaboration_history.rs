//! Bounded Community catch-up through the private Runtime/Carrier provider plane.
//! Live gossip admission and its replay/receipt lifetime remain unchanged.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use elastos_common::collaboration_protocol::MAX_COLLABORATION_ENVELOPE_BYTES;
use elastos_runtime::provider::{
    Provider, ProviderCarrierRoute, ProviderError, ProviderInvocation, ProviderInvocationTransport,
    ProviderRegistry, ProviderTransfer, ResourceRequest, ResourceResponse,
};
use serde::{Deserialize, Serialize};

use crate::collaboration_core::{
    MAX_CONVERSATION_HISTORY_BYTES, MAX_CONVERSATION_HISTORY_MESSAGES,
};
use crate::collaboration_presence::CollaborationPresenceProductPort;
use crate::collaboration_product::{CollaborationChatProductPort, CHAT_SERVICE};

pub(crate) const HISTORY_PROVIDER: &str = "collaboration.history";
const HISTORY_OP: &str = "read";
const REQUEST_SCHEMA: &str = "elastos.collaboration.history-request/v1";
const RESPONSE_SCHEMA: &str = "elastos.collaboration.history-response/v1";
const MAX_REQUEST_BYTES: usize = MAX_COLLABORATION_ENVELOPE_BYTES * 6 + 2_048;
// The retained envelope-array byte bound plus fixed scope/endpoint metadata.
pub(crate) const MAX_HISTORY_RESPONSE_BYTES: usize = MAX_CONVERSATION_HISTORY_BYTES + 4_096;
pub(crate) const MAX_HISTORY_CARRIER_REPLY_BYTES: usize = MAX_HISTORY_RESPONSE_BYTES + 64;
const MAX_PEERS_PER_PASS: usize = 4;
const PEER_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(crate) struct CollaborationHistoryService {
    inner: Arc<HistoryInner>,
}

struct HistoryInner {
    product: CollaborationChatProductPort,
    presence: CollaborationPresenceProductPort,
    registry: Arc<ProviderRegistry>,
    data_root: PathBuf,
    peers: Mutex<HistoryPeerRound>,
}

#[derive(Default)]
struct HistoryPeerRound {
    endpoints: Vec<String>,
    next: usize,
    available: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryRequest {
    schema: String,
    op: String,
    network_id: String,
    conversation_id: String,
    sender_service: String,
    presence: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryResponse {
    schema: String,
    network_id: String,
    conversation_id: String,
    sender_service: String,
    endpoint_did: String,
    envelopes: Vec<String>,
}

impl CollaborationHistoryService {
    pub(crate) fn community_membership(
        &self,
    ) -> &Arc<crate::collaboration_release_network::CommunityMembership> {
        self.inner.product.community_membership()
    }

    pub(crate) async fn new(
        product: CollaborationChatProductPort,
        presence: CollaborationPresenceProductPort,
        registry: Arc<ProviderRegistry>,
        data_root: PathBuf,
    ) -> anyhow::Result<Self> {
        let service = Self {
            inner: Arc::new(HistoryInner {
                product,
                presence,
                registry: registry.clone(),
                data_root,
                peers: Mutex::new(HistoryPeerRound::default()),
            }),
        };
        registry
            .register_runtime_provider_target(
                HISTORY_PROVIDER,
                Arc::new(HistoryProvider {
                    service: service.clone(),
                }),
            )
            .await?;
        Ok(service)
    }

    fn request(&self, presence: String) -> HistoryRequest {
        let (network_id, conversation_id) = self.inner.product.history_scope();
        HistoryRequest {
            schema: REQUEST_SCHEMA.to_string(),
            op: HISTORY_OP.to_string(),
            network_id: network_id.to_string(),
            conversation_id: conversation_id.to_string(),
            sender_service: CHAT_SERVICE.to_string(),
            presence,
        }
    }

    fn receive(
        &self,
        request: HistoryRequest,
        source: &str,
        now: u64,
    ) -> anyhow::Result<HistoryResponse> {
        self.inner.product.community_membership().require_joined()?;
        let (network, conversation) = self.inner.product.history_scope();
        if request.schema != REQUEST_SCHEMA
            || request.op != HISTORY_OP
            || request.network_id != network
            || request.conversation_id != conversation
            || request.sender_service != CHAT_SERVICE
            || request.presence.len() > MAX_COLLABORATION_ENVELOPE_BYTES
        {
            anyhow::bail!("history request has an invalid scope or presence");
        }
        self.inner.presence.authorize_history_requester(
            request.presence.as_bytes(),
            source,
            now,
        )?;
        let envelopes = self
            .inner
            .product
            .retained_history(now)?
            .into_iter()
            .map(String::from_utf8)
            .collect::<Result<Vec<_>, _>>()?;
        let response = HistoryResponse {
            schema: RESPONSE_SCHEMA.to_string(),
            network_id: network.to_string(),
            conversation_id: conversation.to_string(),
            sender_service: CHAT_SERVICE.to_string(),
            endpoint_did: self.inner.product.local_device_did(),
            envelopes,
        };
        if serde_json::to_vec(&response)?.len() > MAX_HISTORY_RESPONSE_BYTES {
            anyhow::bail!("history response exceeds its byte bound");
        }
        Ok(response)
    }

    fn apply_response(
        &self,
        response: serde_json::Value,
        expected_endpoint: &str,
        now: u64,
    ) -> anyhow::Result<()> {
        if serde_json::to_vec(&response)?.len() > MAX_HISTORY_RESPONSE_BYTES {
            anyhow::bail!("history response exceeds its byte bound");
        }
        let response: HistoryResponse = serde_json::from_value(response)?;
        let (network, conversation) = self.inner.product.history_scope();
        if response.schema != RESPONSE_SCHEMA
            || response.network_id != network
            || response.conversation_id != conversation
            || response.sender_service != CHAT_SERVICE
            || response.endpoint_did != expected_endpoint
            || response.envelopes.len() > MAX_CONVERSATION_HISTORY_MESSAGES
            || serde_json::to_vec(&response.envelopes)?.len() > MAX_CONVERSATION_HISTORY_BYTES
        {
            anyhow::bail!("history response has an invalid scope or bounds");
        }
        let messages = response
            .envelopes
            .into_iter()
            .map(String::into_bytes)
            .collect::<Vec<_>>();
        self.inner
            .product
            .project_history(&self.inner.data_root, &messages, now)
    }

    /// Independently scheduled from live gossip. Four unavailable peers cannot
    /// monopolize a pass or starve later available participants across passes.
    pub(crate) async fn fetch_once(&self, now: u64) -> anyhow::Result<()> {
        if !self.inner.product.community_membership().joined() {
            return Ok(());
        }
        let peers = match self.inner.presence.history_participants(now) {
            Ok(peers) => peers,
            Err(error) => {
                self.inner.product.set_history_status(false);
                return Err(error);
            }
        };
        let local_endpoint = self.inner.product.local_device_did();
        let local = peers
            .iter()
            .find(|peer| peer.endpoint_did == local_endpoint);
        let Some(local) = local else {
            self.inner.product.set_history_status(false);
            return Ok(());
        };
        let request = serde_json::to_value(self.request(local.presence_envelope.clone()))?;
        let mut remote = peers
            .into_iter()
            .filter(|peer| peer.endpoint_did != local_endpoint)
            .collect::<Vec<_>>();
        if remote.is_empty() {
            self.inner.product.set_history_status(false);
            return Ok(());
        }
        let round_complete = {
            let mut round = self
                .inner
                .peers
                .lock()
                .map_err(|_| anyhow::anyhow!("history peer cursor is unavailable"))?;
            let endpoints = remote
                .iter()
                .map(|peer| peer.endpoint_did.clone())
                .collect::<Vec<_>>();
            if round.endpoints != endpoints {
                *round = HistoryPeerRound {
                    endpoints,
                    next: 0,
                    available: false,
                };
            }
            let start = round.next;
            let end = (start + MAX_PEERS_PER_PASS).min(remote.len());
            let complete = end == remote.len();
            round.next = if complete { 0 } else { end };
            remote = remote[start..end].to_vec();
            complete
        };
        let mut available = false;
        for peer in remote {
            // History reads have no remote effect. A lost response can be
            // abandoned safely, and the next scheduled pass retries normally.
            let response = tokio::time::timeout(
                PEER_TIMEOUT,
                self.inner.registry.invoke_provider(ProviderInvocation {
                    source: HISTORY_PROVIDER.to_string(),
                    target: HISTORY_PROVIDER.to_string(),
                    op: HISTORY_OP.to_string(),
                    request: request.clone(),
                    transfer: ProviderTransfer::Json,
                    range: None,
                    progress: None,
                    transport: ProviderInvocationTransport::Carrier(
                        ProviderCarrierRoute::PeerDid {
                            peer_did: peer.endpoint_did.clone(),
                            timeout_ms: Some(PEER_TIMEOUT.as_millis() as u64),
                        },
                    ),
                }),
            )
            .await;
            let Ok(Ok(mut response)) = response else {
                continue;
            };
            if let Some(object) = response.as_object_mut() {
                object.remove("_runtime_transfer");
            }
            if self
                .apply_response(response, &peer.endpoint_did, now)
                .is_ok()
            {
                available = true;
            }
        }
        let mut round = self
            .inner
            .peers
            .lock()
            .map_err(|_| anyhow::anyhow!("history peer cursor is unavailable"))?;
        round.available |= available;
        if round.available || round_complete {
            self.inner.product.set_history_status(round.available);
        } else {
            self.inner.product.set_history_searching();
        }
        if round_complete {
            round.available = false;
        }
        Ok(())
    }
}

struct HistoryProvider {
    service: CollaborationHistoryService,
}

#[async_trait::async_trait]
impl Provider for HistoryProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "Community history is Runtime-owned".to_string(),
        ))
    }
    fn schemes(&self) -> Vec<&'static str> {
        Vec::new()
    }
    fn name(&self) -> &'static str {
        HISTORY_PROVIDER
    }
    async fn send_raw(
        &self,
        value: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let operation = (|| -> anyhow::Result<serde_json::Value> {
            if serde_json::to_vec(value)?.len() > MAX_REQUEST_BYTES {
                anyhow::bail!("history request exceeds its byte bound");
            }
            let mut request = value.clone();
            let object = request
                .as_object_mut()
                .context("history request must be an object")?;
            let source =
                authenticated_history_source(object.remove("_runtime_invocation").as_ref())?;
            let request: HistoryRequest = serde_json::from_value(request)?;
            Ok(serde_json::to_value(self.service.receive(
                request,
                &source,
                now_secs(),
            )?)?)
        })();
        operation.map_err(|_| {
            ProviderError::Provider("Community history request was refused".to_string())
        })
    }
}

fn authenticated_history_source(value: Option<&serde_json::Value>) -> anyhow::Result<String> {
    let runtime = value
        .and_then(serde_json::Value::as_object)
        .context("history invocation is missing")?;
    let capability = format!("provider:{HISTORY_PROVIDER}->{HISTORY_PROVIDER}:{HISTORY_OP}");
    for (field, expected) in [
        ("schema", "elastos.provider.invocation/v1"),
        ("source", HISTORY_PROVIDER),
        ("target", HISTORY_PROVIDER),
        ("op", HISTORY_OP),
        ("capability", capability.as_str()),
        ("transport", "carrier-provider-plane"),
        ("transfer", "json"),
    ] {
        if runtime.get(field).and_then(serde_json::Value::as_str) != Some(expected) {
            anyhow::bail!("history invocation is invalid");
        }
    }
    crate::collaboration_protocol::authenticated_carrier_source_endpoint(runtime.get("carrier"))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collaboration_presence::presence_request_binding;
    use crate::collaboration_product::{chat_message_request_binding, test_chat_product_port};
    use elastos_runtime::provider::{ProviderCarrierInvoker, ResourceAction};
    use std::collections::BTreeMap;

    struct Fixture {
        root: tempfile::TempDir,
        product: CollaborationChatProductPort,
        presence: CollaborationPresenceProductPort,
        registry: Arc<ProviderRegistry>,
        service: CollaborationHistoryService,
        own_presence: String,
    }

    async fn fixture(network: &str, conversation: &str, now: u64) -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let product = test_chat_product_port(root.path(), network, conversation);
        let presence = CollaborationPresenceProductPort::new(product.test_core().clone()).unwrap();
        let profile = product.test_person_profile("History person", None);
        let prepared = presence
            .prepare_presence(
                presence_request_binding("history-presence", "test-principal", &profile).unwrap(),
                &profile,
                now,
            )
            .unwrap();
        presence.project_prepared_presence(&prepared, now).unwrap();
        let own_presence = presence.history_participants(now).unwrap()[0]
            .presence_envelope
            .clone();
        let registry = Arc::new(ProviderRegistry::new());
        let service = CollaborationHistoryService::new(
            product.clone(),
            presence.clone(),
            registry.clone(),
            root.path().to_path_buf(),
        )
        .await
        .unwrap();
        Fixture {
            root,
            product,
            presence,
            registry,
            service,
            own_presence,
        }
    }

    fn observe(observer: &Fixture, peer: &Fixture, now: u64) {
        observer
            .product
            .test_core()
            .accept_incoming_from_signed_source_for_test(peer.own_presence.as_bytes(), now)
            .unwrap();
        for presence in observer.presence.pending_presences().unwrap() {
            observer.presence.project_handoff(&presence, now).unwrap();
        }
    }

    fn store_chat(peer: &Fixture, id: &str, body: &str, created_at: u64) -> Vec<u8> {
        let profile = peer.product.test_person_profile("History author", None);
        let message = peer
            .product
            .prepare_message(
                chat_message_request_binding(id, "test-principal", body, &profile).unwrap(),
                body,
                &profile,
                created_at,
            )
            .unwrap();
        peer.product
            .project_prepared_message(peer.root.path(), &message, None)
            .unwrap();
        peer.product
            .retained_history(now_secs())
            .unwrap()
            .into_iter()
            .find(|bytes| {
                elastos_common::collaboration_protocol::collaboration_message_envelope_sha256(bytes)
                    == message.envelope_sha256()
            })
            .unwrap()
    }

    struct Plane {
        source: String,
        peers: BTreeMap<String, Arc<ProviderRegistry>>,
        calls: Mutex<Vec<String>>,
        stall: bool,
    }

    #[async_trait::async_trait]
    impl ProviderCarrierInvoker for Plane {
        async fn invoke_carrier_provider(
            &self,
            route: &ProviderCarrierRoute,
            invocation: &ProviderInvocation,
            mut request: serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            let ProviderCarrierRoute::PeerDid { peer_did, .. } = route else {
                panic!("history selects verified Profile endpoint");
            };
            self.calls.lock().unwrap().push(peer_did.clone());
            if self.stall {
                std::future::pending::<()>().await;
            }
            let remote = self
                .peers
                .get(peer_did)
                .ok_or_else(|| ProviderError::Unavailable("offline".to_string()))?;
            request["_runtime_invocation"]["carrier"] =
                serde_json::json!({"source_endpoint_did": self.source});
            remote
                .send_runtime_provider_target_raw(&invocation.target, &request)
                .await
        }
    }

    #[tokio::test]
    async fn history_service_catches_up_canonical_expired_live_messages_once() {
        let now = now_secs();
        let a = fixture("history-test", "community", now).await;
        let b = fixture("history-test", "community", now).await;
        let bytes = store_chat(
            &b,
            "missed-community-message",
            "original history",
            now - 600,
        );
        assert!(a
            .product
            .test_core()
            .accept_incoming_from_signed_source_for_test(&bytes, now)
            .is_err());
        observe(&a, &b, now);
        let plane = Arc::new(Plane {
            source: a.product.local_device_did(),
            peers: BTreeMap::from([(b.product.local_device_did(), b.registry.clone())]),
            calls: Mutex::new(Vec::new()),
            stall: false,
        });
        a.registry.set_carrier_invoker(plane.clone()).await;
        let session = crate::room_service::start_local_runtime_session(
            a.root.path(),
            &a.product.local_device_did(),
            "Reader",
            "test",
        )
        .unwrap();
        a.service.fetch_once(now).await.unwrap();
        a.service.fetch_once(now).await.unwrap();
        let poll = a
            .product
            .conversation_poll(a.root.path(), &session.token, 0)
            .unwrap();
        assert_eq!(poll.objects.len(), 1);
        assert_eq!(poll.objects[0].body.as_deref(), Some("original history"));
        assert_eq!(poll.objects[0].created_at, now - 600);
        assert_eq!(
            a.product.retained_history(now).unwrap(),
            vec![bytes.clone()]
        );
        assert!(a
            .product
            .test_core()
            .pending_product_handoffs()
            .unwrap()
            .is_empty());
        assert_eq!(
            a.product
                .conversation_transport_view()
                .history
                .unwrap()
                .status,
            "available"
        );
        assert_eq!(plane.calls.lock().unwrap().len(), 2);
        // Catch-up keeps its originals separate from live replay entries.
        let state_path = a.root.path().join("collaboration/default-conversation");
        assert!(std::fs::read_dir(state_path).unwrap().any(|entry| entry
            .unwrap()
            .path()
            .join("history-v1.json")
            .exists()));
        assert!(a
            .product
            .test_core()
            .accept_incoming_from_signed_source_for_test(&bytes, now)
            .is_err());
    }

    #[tokio::test]
    async fn history_refuses_foreign_scope_source_expired_presence_and_untrusted_payloads() {
        let now = now_secs();
        let a = fixture("history-test", "community", now).await;
        let b = fixture("history-test", "community", now).await;
        let foreign = fixture("foreign-history", "other", now).await;
        let request = b.service.request(b.own_presence.clone());
        assert!(a
            .service
            .receive(request.clone(), &b.product.local_device_did(), now)
            .is_ok());
        assert!(a
            .service
            .receive(request.clone(), &a.product.local_device_did(), now)
            .is_err());
        assert!(a
            .service
            .receive(request.clone(), &b.product.local_device_did(), now + 46)
            .is_err());
        assert!(a
            .service
            .receive(
                foreign.service.request(foreign.own_presence.clone()),
                &foreign.product.local_device_did(),
                now
            )
            .is_err());
        for field in [
            "schema",
            "op",
            "network_id",
            "conversation_id",
            "sender_service",
        ] {
            let mut changed = serde_json::to_value(&request).unwrap();
            changed[field] = serde_json::json!("foreign");
            assert!(a
                .service
                .receive(
                    serde_json::from_value(changed).unwrap(),
                    &b.product.local_device_did(),
                    now
                )
                .is_err());
        }
        let original = store_chat(&b, "history-refusal", "valid", now - 600);
        let valid = serde_json::to_value(
            b.service
                .receive(request, &b.product.local_device_did(), now)
                .unwrap(),
        )
        .unwrap();
        for field in [
            "schema",
            "network_id",
            "conversation_id",
            "sender_service",
            "endpoint_did",
        ] {
            let mut changed = valid.clone();
            changed[field] = serde_json::json!("foreign");
            assert!(a
                .service
                .apply_response(changed, &b.product.local_device_did(), now)
                .is_err());
        }
        let mut malformed = valid.clone();
        malformed["unknown"] = serde_json::json!(true);
        assert!(a
            .service
            .apply_response(malformed, &b.product.local_device_did(), now)
            .is_err());
        let mut tampered: serde_json::Value = serde_json::from_slice(&original).unwrap();
        tampered["payload"]["payload"]["product"]["body"] = serde_json::json!("changed");
        let mut mixed = valid.clone();
        mixed["envelopes"] = serde_json::json!([
            String::from_utf8(original).unwrap(),
            serde_json::to_string(&tampered).unwrap()
        ]);
        assert!(a
            .service
            .apply_response(mixed, &b.product.local_device_did(), now)
            .is_err());
        let mut conflicting: elastos_common::collaboration_protocol::SignedCollaborationMessage =
            serde_json::from_str(valid["envelopes"][0].as_str().unwrap()).unwrap();
        conflicting.payload.payload["product"]["body"] =
            serde_json::json!("valid signature, conflicting identity");
        let (key, _) = elastos_identity::load_or_create_did(b.root.path()).unwrap();
        let (signature, signer) = crate::crypto::domain_separated_sign(
            &key,
            elastos_common::collaboration_protocol::COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
            &elastos_common::collaboration_protocol::canonical_collaboration_message_bytes(
                &conflicting.payload,
            )
            .unwrap(),
        );
        conflicting.signature = signature;
        conflicting.signer_did = signer;
        let conflicting =
            elastos_common::collaboration_protocol::canonical_signed_collaboration_message_bytes(
                &conflicting,
            )
            .unwrap();
        assert!(a
            .product
            .test_core()
            .authorize_history_message(&conflicting, now)
            .is_ok());
        let mut conflict_batch = valid.clone();
        conflict_batch["envelopes"] = serde_json::json!([
            valid["envelopes"][0].clone(),
            String::from_utf8(conflicting).unwrap()
        ]);
        assert!(a
            .service
            .apply_response(conflict_batch, &b.product.local_device_did(), now)
            .is_err());
        let mut presence_batch = valid.clone();
        presence_batch["envelopes"] =
            serde_json::json!([valid["envelopes"][0].clone(), b.own_presence]);
        assert!(a
            .service
            .apply_response(presence_batch, &b.product.local_device_did(), now)
            .is_err());
        let mut oversized = valid.clone();
        oversized["envelopes"] = serde_json::json!(["x".repeat(MAX_HISTORY_RESPONSE_BYTES)]);
        assert!(a
            .service
            .apply_response(oversized, &b.product.local_device_did(), now)
            .is_err());
        let mut too_many = valid;
        too_many["envelopes"] = serde_json::json!(vec![""; MAX_CONVERSATION_HISTORY_MESSAGES + 1]);
        assert!(a
            .service
            .apply_response(too_many, &b.product.local_device_did(), now)
            .is_err());
        assert!(a.product.retained_history(now).unwrap().is_empty());
        let room_root = elastos_common::localhost::rooted_localhost_fs_path(
            a.root.path(),
            crate::room_service::room_root_uri(),
        )
        .unwrap();
        assert!(!room_root.join("room/objects.json").exists());
    }

    #[tokio::test]
    async fn history_target_stays_private_and_requires_authenticated_carrier_invocation() {
        let now = now_secs();
        let a = fixture("history-private", "community", now).await;
        let b = fixture("history-private", "community", now).await;
        let request = serde_json::to_value(b.service.request(b.own_presence.clone())).unwrap();
        assert!(a
            .registry
            .send_raw(HISTORY_PROVIDER, &request)
            .await
            .is_err());
        assert!(a
            .registry
            .route(
                &format!("elastos://{HISTORY_PROVIDER}/read"),
                "capsule:chat-room",
                ResourceAction::Read,
                None
            )
            .await
            .is_err());
        assert!(a
            .registry
            .registrations()
            .await
            .iter()
            .all(|route| route.route != HISTORY_PROVIDER));
        let provider = HistoryProvider {
            service: a.service.clone(),
        };
        assert!(provider.send_raw(&request).await.is_err());
        let mut authenticated = request;
        authenticated["_runtime_invocation"] = serde_json::json!({
            "schema":"elastos.provider.invocation/v1", "source":HISTORY_PROVIDER,
            "target":HISTORY_PROVIDER, "op":"read",
            "capability":format!("provider:{HISTORY_PROVIDER}->{HISTORY_PROVIDER}:read"),
            "transport":"carrier-provider-plane", "transfer":"json",
            "carrier":{"source_endpoint_did":b.product.local_device_did()},
        });
        assert!(provider.send_raw(&authenticated).await.is_ok());
        for field in [
            "source",
            "target",
            "op",
            "capability",
            "transport",
            "transfer",
        ] {
            let mut changed = authenticated.clone();
            changed["_runtime_invocation"][field] = serde_json::json!("foreign");
            assert!(provider.send_raw(&changed).await.is_err());
        }
        authenticated["_runtime_invocation"]["carrier"] = serde_json::Value::Null;
        assert!(provider.send_raw(&authenticated).await.is_err());
    }

    #[tokio::test]
    async fn unavailable_history_keeps_live_transport_available_and_rotates_past_four_peers() {
        let now = now_secs();
        let a = fixture("history-fair", "community", now).await;
        a.service.fetch_once(now).await.unwrap();
        assert!(a.product.conversation_transport_view().available);
        assert_eq!(
            a.product
                .conversation_transport_view()
                .history
                .unwrap()
                .status,
            "unavailable"
        );
        let mut fixtures = Vec::new();
        for _ in 0..5 {
            let peer = fixture("history-fair", "community", now).await;
            observe(&a, &peer, now);
            fixtures.push(peer);
        }
        fixtures.sort_by_key(|peer| peer.product.local_device_did());
        let last = &fixtures[4];
        let bytes = store_chat(
            last,
            "reachable-history",
            "later reachable participant",
            now - 600,
        );
        let plane = Arc::new(Plane {
            source: a.product.local_device_did(),
            peers: BTreeMap::from([(last.product.local_device_did(), last.registry.clone())]),
            calls: Mutex::new(Vec::new()),
            stall: false,
        });
        a.registry.set_carrier_invoker(plane.clone()).await;
        a.service.fetch_once(now).await.unwrap();
        assert_eq!(plane.calls.lock().unwrap().len(), 4);
        assert!(a.product.retained_history(now).unwrap().is_empty());
        assert_eq!(
            a.product
                .conversation_transport_view()
                .history
                .unwrap()
                .status,
            "searching"
        );
        a.service.fetch_once(now).await.unwrap();
        assert_eq!(
            plane.calls.lock().unwrap()[4],
            last.product.local_device_did()
        );
        assert_eq!(a.product.retained_history(now).unwrap(), vec![bytes]);
        assert_eq!(
            a.product
                .conversation_transport_view()
                .history
                .unwrap()
                .status,
            "available"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_history_has_a_per_peer_deadline() {
        let now = now_secs();
        let a = fixture("history-stalled", "community", now).await;
        let b = fixture("history-stalled", "community", now).await;
        observe(&a, &b, now);
        let plane = Arc::new(Plane {
            source: a.product.local_device_did(),
            peers: BTreeMap::new(),
            calls: Mutex::new(Vec::new()),
            stall: true,
        });
        a.registry.set_carrier_invoker(plane.clone()).await;
        let start = tokio::time::Instant::now();
        a.service.fetch_once(now).await.unwrap();
        assert_eq!(tokio::time::Instant::now() - start, PEER_TIMEOUT);
        assert_eq!(plane.calls.lock().unwrap().len(), 1);
        assert!(a.product.conversation_transport_view().available);
        assert_eq!(
            a.product
                .conversation_transport_view()
                .history
                .unwrap()
                .status,
            "unavailable"
        );
    }
}
