use super::home_system::{
    configured_discovery_network_profile_for_test, home_test_get_json, home_test_post_json,
    write_home_principal_object_json_for_authority,
};
use super::*;
use crate::api::gateway::gateway_model_service::{
    cancel_grant_runs, install_create_race_barrier, model_grant_id, remote_principal_id,
    CreateRacePoint,
};
use crate::collaboration_contact_store::CollaborationContactStore;
use crate::collaboration_discovery::*;
use crate::collaboration_profile_authority::{
    signed_profile_document_for_test, VerifiedCollaborationProfileDocument,
};
use elastos_common::collaboration_protocol::*;
use elastos_model_contract::{model_input_hash, RuntimeAccessBinding, RuntimeCreateBinding};
use elastos_runtime::signature::{generate_keypair, SigningKey};
use tokio::io::{AsyncBufReadExt as _, BufReader};

const NETWORK: &str = "remote-model";
const CARRIER_ALPN: &[u8] = b"elastos/carrier/1";
const REQUEST_ID: &str = "request-model-1";
const SEED_PRINCIPAL: &str = "seed-principal-a";
const OTHER_SEED_PRINCIPAL: &str = "seed-principal-b";

fn keys(value: &Value) -> BTreeSet<&str> {
    let object = value.as_object().unwrap();
    object.keys().map(String::as_str).collect()
}

fn provider_error(err: impl std::fmt::Display) -> ProviderError {
    ProviderError::Provider(err.to_string())
}

fn run_id_for(principal_id: &str, request_id: &str) -> String {
    run_id_for_capsule(principal_id, request_id, "assistant")
}

fn run_id_for_capsule(principal_id: &str, request_id: &str, capsule_id: &str) -> String {
    format!(
        "run:sha256:{}",
        hex::encode(Sha256::digest(format!(
            "{principal_id}:{capsule_id}:{request_id}"
        )))
    )
}

// Replies follow the provider contract (`capsules/model-provider/src/contract.rs`):
// a `RunView` carries `status` and, once settled, a `terminal` outcome; a
// `RunEventsPage` carries no status at all, only events with a `terminal` flag.
fn run_view(run_id: &str, status: &str) -> Value {
    let mut view = json!({
        "schema": "elastos.model.run-view/v1", "run_id": run_id, "offer_id": "qwen-local",
        "operation": "text.generate", "status": status, "sequence_cursor": 0 });
    if matches!(
        status,
        "completed" | "failed" | "cancelled" | "settlement_unknown"
    ) {
        let mut terminal = json!({ "status": status });
        if status == "completed" {
            terminal["output"] = json!({
                "schema": "elastos.model.output.text/v1",
                "text": "hello",
            });
        }
        view["terminal"] = terminal;
    }
    json!({ "status": "ok", "data": view })
}

fn events_page(run_id: &str, page: u32) -> Value {
    let event = |sequence: u64, kind: &str, text: &str, terminal: bool| {
        json!({ "schema": "elastos.model.run-event/v1", "sequence": sequence, "kind": kind,
            "data": { "schema": "elastos.model.output.text/v1", "text": text }, "terminal": terminal })
    };
    let events = match page {
        1 => vec![
            event(1, "delta", "hel", false),
            event(2, "delta", "lo", false),
        ],
        _ => vec![event(3, "output", "hello", true)],
    };
    json!({ "status": "ok", "data": {
        "schema": "elastos.model.run-events/v1", "run_id": run_id,
        "next_cursor": events.len(), "has_more": page == 1, "events": events } })
}

#[derive(Default)]
struct FakeModelProvider {
    calls: TokioMutex<Vec<Value>>,
    run_owners: std::sync::Mutex<BTreeMap<String, String>>,
    event_pages: std::sync::Mutex<BTreeMap<String, u32>>,
    run_status: std::sync::Mutex<BTreeMap<String, String>>,
    /// Injected once onto the next `runs_get` or `runs_events` reply.
    next_error: std::sync::Mutex<Option<String>>,
    /// Overlay applied to the next `runs_create` reply `data` object.
    next_create_data: std::sync::Mutex<Option<Value>>,
    /// Extra top-level `run_id` that disagrees with `data.run_id`.
    next_create_alias_run_id: std::sync::Mutex<Option<String>>,
}

#[async_trait::async_trait]
impl Provider for FakeModelProvider {
    fn name(&self) -> &'static str {
        "fake-model-provider"
    }
    fn schemes(&self) -> Vec<&'static str> {
        vec!["model"]
    }
    async fn handle(&self, _: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider("raw only".into()))
    }
    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        self.calls.lock().await.push(request.clone());
        let op = request["op"].as_str().unwrap_or_default();
        let typed_fields = match op {
            "offers_list" => vec!["op"],
            "runs_create" => vec!["op", "offer_id", "operation", "input", "runtime_binding"],
            "runs_events" => vec!["op", "run_id", "after_sequence", "runtime_binding"],
            _ => vec!["op", "run_id", "runtime_binding"],
        };
        if keys(request) != typed_fields.into_iter().collect() {
            return Ok(json!({ "status": "error", "message": "fields differ from the contract" }));
        }
        if op == "offers_list" {
            return Ok(json!({ "status": "ok", "data": { "offers": [
                { "id": "qwen-local", "title": "Qwen", "operation": "text.generate", "hosted": null },
                { "id": "gpt-hosted", "title": "GPT", "operation": "text.generate",
                  "hosted": { "placement": "hosted", "backend_provider_label": "OpenAI" } },
            ] } }));
        }
        if op == "runs_create" {
            let binding: RuntimeCreateBinding =
                serde_json::from_value(request["runtime_binding"].clone())
                    .map_err(provider_error)?;
            let offer_id = request["offer_id"].as_str().unwrap_or_default();
            let operation = request["operation"].as_str().unwrap_or_default();
            binding
                .validate(offer_id, operation, &request["input"])
                .map_err(provider_error)?;
            let run_id = run_id_for_capsule(
                &binding.principal_id,
                &binding.request_id,
                &binding.capsule_id,
            );
            self.run_owners
                .lock()
                .unwrap()
                .insert(run_id.clone(), binding.principal_id);
            self.run_status
                .lock()
                .unwrap()
                .insert(run_id.clone(), "running".to_string());
            let mut reply = run_view(&run_id, "running");
            if let Some(overlay) = self.next_create_data.lock().unwrap().take() {
                if let Some(data) = reply.get_mut("data").and_then(Value::as_object_mut) {
                    if let Some(object) = overlay.as_object() {
                        for (key, value) in object {
                            data.insert(key.clone(), value.clone());
                        }
                    }
                }
            }
            if let Some(alias) = self.next_create_alias_run_id.lock().unwrap().take() {
                reply["run_id"] = json!(alias);
            }
            return Ok(reply);
        }
        let binding: RuntimeAccessBinding =
            serde_json::from_value(request["runtime_binding"].clone()).map_err(provider_error)?;
        let run_id = request["run_id"].as_str().unwrap_or_default();
        binding.validate(run_id).map_err(provider_error)?;
        if matches!(op, "runs_get" | "runs_events") {
            if let Some(code) = self.next_error.lock().unwrap().take() {
                return Ok(json!({ "status": "error", "code": code, "message": code }));
            }
        }
        if !self.run_owners.lock().unwrap().contains_key(run_id) {
            return Ok(json!({
                "status": "error",
                "code": "run_not_found",
                "message": "model run is not available for the current caller",
            }));
        }
        if self.run_owners.lock().unwrap().get(run_id) != Some(&binding.principal_id) {
            return Ok(
                json!({ "status": "error", "code": "denied", "message": "run owner mismatch" }),
            );
        }
        Ok(match op {
            "runs_get" => {
                let status = self
                    .run_status
                    .lock()
                    .unwrap()
                    .get(&binding.run_id)
                    .cloned()
                    .unwrap_or_else(|| "running".to_string());
                run_view(&binding.run_id, &status)
            }
            "runs_cancel" => {
                self.run_status
                    .lock()
                    .unwrap()
                    .insert(binding.run_id.clone(), "cancelled".to_string());
                run_view(&binding.run_id, "cancelled")
            }
            _ => {
                let mut pages = self.event_pages.lock().unwrap();
                let page = pages.entry(binding.run_id.clone()).or_insert(0);
                *page += 1;
                let page = *page;
                drop(pages);
                if page > 1 {
                    self.run_status
                        .lock()
                        .unwrap()
                        .insert(binding.run_id.clone(), "completed".to_string());
                }
                events_page(&binding.run_id, page)
            }
        })
    }
}

fn signed_message(
    key: &SigningKey,
    sender_profile_did: &str,
    conversation_id: &str,
    recipient: CollaborationRecipient,
    payload_type: &str,
    payload: Value,
    validity: std::ops::Range<u64>,
) -> Vec<u8> {
    let message = CollaborationMessage {
        schema: COLLABORATION_MESSAGE_SCHEMA_V1.to_string(),
        network_id: NETWORK.to_string(),
        conversation_id: conversation_id.to_string(),
        message_id: crate::collaboration_core::random_hex_128().unwrap(),
        nonce: crate::collaboration_core::random_hex_128().unwrap(),
        created_at: validity.start,
        expires_at: validity.end,
        sender_profile_did: sender_profile_did.to_string(),
        sender_service: COLLABORATION_DISCOVERY_SERVICE.to_string(),
        recipient,
        payload_type: payload_type.to_string(),
        payload,
    };
    let (signature, signer_did) = crate::crypto::domain_separated_sign(
        key,
        COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
        &canonical_collaboration_message_bytes(&message).unwrap(),
    );
    canonical_signed_collaboration_message_bytes(&SignedCollaborationMessage {
        payload: message,
        signature,
        signer_did,
    })
    .unwrap()
}

fn accept_seed_contact(
    store: &CollaborationContactStore,
    owner_key: &SigningKey,
    owner_profile: &VerifiedCollaborationProfileDocument,
    seed_key: &SigningKey,
    seed_did: &str,
) {
    let now = now_ts().saturating_sub(3);
    let owner_profile_did = owner_profile.document().profile_did.clone();
    let (seed_profile_key, _) = generate_keypair();
    let seed_profile = signed_profile_document_for_test(
        &seed_profile_key,
        "Seed",
        None,
        1,
        None,
        now,
        vec![seed_did.to_string()],
    )
    .unwrap();
    let advertisement = signed_message(
        owner_key,
        &owner_profile_did,
        COLLABORATION_DISCOVERY_DIRECTORY_ID,
        CollaborationRecipient {
            kind: CollaborationRecipientKind::Conversation,
            id: COLLABORATION_DISCOVERY_DIRECTORY_ID.to_string(),
        },
        COLLABORATION_DISCOVERY_ADVERTISEMENT_PAYLOAD_TYPE,
        serde_json::to_value(CollaborationDiscoveryAdvertisementPayload {
            signed_profile: owner_profile.signed_envelope().clone(),
        })
        .unwrap(),
        now..now + COLLABORATION_DISCOVERY_ADVERTISEMENT_TTL_SECS,
    );
    store
        .store_local_advertisement(&advertisement, now)
        .unwrap();
    let request = signed_message(
        seed_key,
        &seed_profile.document().profile_did,
        COLLABORATION_DISCOVERY_CONTACT_ID,
        CollaborationRecipient {
            kind: CollaborationRecipientKind::Profile,
            id: owner_profile_did.clone(),
        },
        COLLABORATION_DISCOVERY_CONTACT_REQUEST_PAYLOAD_TYPE,
        serde_json::to_value(CollaborationContactRequestPayload {
            advertisement_envelope_sha256: collaboration_message_envelope_sha256(&advertisement),
            signed_profile: seed_profile.signed_envelope().clone(),
        })
        .unwrap(),
        now + 1..now + 1 + COLLABORATION_DISCOVERY_CONTACT_REQUEST_TTL_SECS,
    );
    store
        .record_incoming_contact_request(&request, now + 1)
        .unwrap();
    let envelope: SignedCollaborationMessage = serde_json::from_slice(&request).unwrap();
    let payload = CollaborationContactDecisionReceipt {
        schema: COLLABORATION_CONTACT_DECISION_RECEIPT_SCHEMA_V1.to_string(),
        network_id: NETWORK.to_string(),
        request_envelope_sha256: collaboration_message_envelope_sha256(&request),
        conversation_id: COLLABORATION_DISCOVERY_CONTACT_ID.to_string(),
        requester_profile_did: seed_profile.document().profile_did.clone(),
        requester_endpoint_did: envelope.signer_did,
        request_message_id: envelope.payload.message_id,
        request_message_nonce: envelope.payload.nonce,
        recipient_profile_did: owner_profile_did,
        recipient_endpoint_did: crate::crypto::encode_signing_key_did(owner_key),
        decision: CollaborationContactDecision::Accepted,
        decided_at: now + 2,
    };
    let (signature, signer_did) = crate::crypto::domain_separated_sign(
        owner_key,
        COLLABORATION_CONTACT_DECISION_RECEIPT_SIGNATURE_DOMAIN_V1,
        &serde_json::to_vec(&serde_json::to_value(&payload).unwrap()).unwrap(),
    );
    let receipt = canonical_signed_collaboration_contact_decision_receipt_bytes(
        &SignedCollaborationContactDecisionReceipt {
            payload,
            signature,
            signer_did,
        },
    )
    .unwrap();
    store
        .record_contact_decision_receipt(&receipt, now + 2)
        .unwrap();
    let contacts = store.snapshot().unwrap();
    let contact = &contacts.contacts()[0];
    assert_eq!(contact.remote_presence_device_did(), seed_did);
}

async fn direct_addr(endpoint: &iroh::Endpoint) -> iroh::EndpointAddr {
    use iroh::Watcher as _;
    let mut watcher = endpoint.watch_addr();
    loop {
        let addr = watcher.get();
        let is_ip = |a: &iroh::TransportAddr| matches!(a, iroh::TransportAddr::Ip(_));
        if addr.addrs.iter().any(is_ip) {
            return addr;
        }
        watcher.updated().await.unwrap();
    }
}

struct TwoRuntimes {
    owner: tempfile::TempDir,
    seed: tempfile::TempDir,
    authority: TestPasskeyAuthority,
    provider: Arc<FakeModelProvider>,
    registry: Arc<ProviderRegistry>,
    /// The seed's own registry with a Carrier invoker: the real consumer path.
    seed_registry: Arc<ProviderRegistry>,
    owner_service: crate::carrier::CarrierRuntimeService,
    owner_addr: iroh::EndpointAddr,
    seed_node: crate::carrier::CarrierNode,
    seed_did: String,
    grant_id: String,
}

impl TwoRuntimes {
    async fn start() -> Self {
        let owner = tempfile::tempdir().unwrap();
        let seed = tempfile::tempdir().unwrap();
        let authority = passkey_authority_with_profile(owner.path(), "Mac");
        let (owner_key, owner_did) = elastos_identity::load_or_create_did(owner.path()).unwrap();
        let (seed_key, seed_did) = elastos_identity::load_or_create_did(seed.path()).unwrap();
        let (trusted_key, _) = generate_keypair();
        let network = configured_discovery_network_profile_for_test(&trusted_key, NETWORK);
        let owner_profile = load_profile_for_authority(owner.path(), &authority);
        let store = CollaborationContactStore::new(
            owner.path(),
            &authority.principal_id,
            &crate::auth::principal_localhost_root(&authority.principal_id),
            network.clone(),
            &owner_profile,
            &owner_did,
        )
        .unwrap();
        accept_seed_contact(&store, &owner_key, &owner_profile, &seed_key, &seed_did);
        let provider = Arc::new(FakeModelProvider::default());
        let registry = Arc::new(ProviderRegistry::new());
        registry
            .register_sub_provider("model", provider.clone())
            .await
            .unwrap();
        let owner_node = crate::carrier::start_isolated_carrier_node_with_registry(
            &owner_key,
            &owner_did,
            owner.path().to_path_buf(),
            Some(Arc::downgrade(&registry)),
        )
        .await
        .unwrap();
        let owner_service = crate::carrier::CarrierRuntimeService::new(owner_node);
        owner_service.configure_browser_exit_network(network).await;
        let owner_addr = direct_addr(&owner_service.endpoint().unwrap()).await;
        let seed_node = crate::carrier::start_isolated_carrier_node_with_registry(
            &seed_key,
            &seed_did,
            seed.path().to_path_buf(),
            None,
        )
        .await
        .unwrap();
        let seed_registry = Arc::new(ProviderRegistry::new());
        seed_registry
            .set_carrier_invoker(Arc::new(
                crate::carrier::CarrierProviderInvoker::with_carrier_endpoint_and_registry(
                    seed_node.endpoint.clone(),
                    Arc::downgrade(&seed_registry),
                ),
            ))
            .await;
        let fixture = Self {
            owner,
            seed,
            authority,
            provider,
            registry,
            seed_registry,
            owner_service,
            owner_addr,
            seed_node,
            seed_did,
            grant_id: model_grant_id(REQUEST_ID),
        };
        write_home_principal_object_json_for_authority(
            fixture.owner.path(),
            &fixture.authority,
            "services-state.json",
            json!({
                "schema": "elastos.services.state/v1",
                "principal_id": fixture.authority.principal_id,
                "localhost_root": fixture.localhost_root(),
                "updated_at": now_ts(),
                "local_offer_ids": ["local:provider:model"],
            }),
        );
        fixture.write_request_record("approved");
        fixture
    }

    fn localhost_root(&self) -> String {
        crate::auth::principal_localhost_root(&self.authority.principal_id)
    }

    fn write_request_record(&self, status: &str) {
        let now = now_ts();
        write_home_principal_object_json_for_authority(
            self.owner.path(),
            &self.authority,
            "services-requests.json",
            json!({
                "schema": "elastos.services.requests/v1",
                "principal_id": self.authority.principal_id,
                "localhost_root": self.localhost_root(),
                "updated_at": now,
                "requests": { REQUEST_ID: {
                    "request_id": REQUEST_ID,
                    "offer_id": "local:provider:model",
                    "service_uri": "elastos://peer/model",
                    "service_kind": "remote_model",
                    "service_display_name": "Mac model",
                    "requester_peer_id": self.seed_node.endpoint.id().to_string(),
                    "requester_did": self.seed_did,
                    "requester_principal_id": SEED_PRINCIPAL,
                    "requester_display_name": "Seed",
                    "created_at": now,
                    "updated_at": now,
                    "status": status,
                    "authenticated_request": true,
                    "grant_expires_at": now + 3600,
                } },
            }),
        );
    }

    fn remote(&self, principal_id: &str, capsule_id: &str) -> Value {
        json!({ "grant_id": self.grant_id, "principal_id": principal_id, "capsule_id": capsule_id })
    }

    fn remote_principal(&self) -> String {
        remote_principal_id(&self.seed_did, SEED_PRINCIPAL)
    }

    async fn call(&self, operation: &str, request: Value) -> Value {
        let conn = self
            .seed_node
            .endpoint
            .connect(self.owner_addr.clone(), CARRIER_ALPN)
            .await
            .unwrap();
        let (mut send, recv) = conn.open_bi().await.unwrap();
        let mut bytes = serde_json::to_vec(&json!({
            "op": "provider_invoke", "source": "model-consumer", "target": "model",
            "operation": operation, "transfer": "json", "range": null, "progress": null,
            "request": request,
        }))
        .unwrap();
        bytes.push(b'\n');
        send.write_all(&bytes).await.unwrap();
        send.finish().unwrap();
        let mut line = String::new();
        let mut reader = BufReader::new(recv);
        tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line))
            .await
            .expect("owner Runtime answered within the Carrier deadline")
            .unwrap();
        serde_json::from_str(line.trim()).unwrap()
    }

    async fn create_run(&self, request_id: &str, offer_id: &str) -> Value {
        self.create_run_as(
            request_id,
            offer_id,
            "assistant",
            json!({ "prompt": "hello" }),
        )
        .await
    }

    async fn create_run_as(
        &self,
        request_id: &str,
        offer_id: &str,
        capsule_id: &str,
        input: Value,
    ) -> Value {
        self.call(
            "runs_create",
            json!({
                "op": "runs_create", "offer_id": offer_id, "operation": "text.generate",
                "input": input, "request_id": request_id,
                "remote_model": self.remote(SEED_PRINCIPAL, capsule_id),
                "_runtime_invocation": { "schema": "elastos.provider.invocation/v1" },
            }),
        )
        .await
    }

    async fn run_events(&self, run_id: &str, after_sequence: u64) -> Value {
        self.call(
            "runs_events",
            json!({
                "op": "runs_events", "run_id": run_id, "after_sequence": after_sequence,
                "request_id": format!("events:{after_sequence}"),
                "remote_model": self.remote(SEED_PRINCIPAL, "assistant"),
            }),
        )
        .await
    }

    async fn run_operation(&self, op: &str, run_id: &str, principal_id: &str) -> Value {
        self.call(
            op,
            json!({
                "op": op, "run_id": run_id, "request_id": format!("{op}:{principal_id}"),
                "remote_model": self.remote(principal_id, "assistant"),
            }),
        )
        .await
    }

    fn run_record(&self, run_id: &str) -> Value {
        let path = self.owner.path().join("services-model-runs.json");
        let index: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(index["schema"], "elastos.services.model-runs/v1");
        index["runs"][run_id].clone()
    }

    async fn provider_ops(&self) -> Vec<String> {
        let calls = self.provider.calls.lock().await;
        let op = |call: &Value| call["op"].as_str().unwrap_or_default().to_string();
        calls.iter().map(op).collect()
    }

    async fn last_provider_call(&self) -> Value {
        self.provider.calls.lock().await.last().cloned().unwrap()
    }

    async fn create_count(&self) -> usize {
        self.provider_ops()
            .await
            .iter()
            .filter(|op| *op == "runs_create")
            .count()
    }

    async fn revoke_grant(&self) -> anyhow::Result<Vec<String>> {
        self.write_request_record("denied");
        cancel_grant_runs(self.registry.clone(), self.owner.path(), &self.grant_id).await
    }

    async fn shutdown(mut self) {
        self.owner_service.shutdown().await.unwrap();
        self.seed_node.shutdown().await;
    }
}

#[tokio::test]
async fn offers_list_over_carrier_shares_only_local_offers() {
    let fx = TwoRuntimes::start().await;
    let list = |capsule: &str| json!({ "op": "offers_list", "remote_model": fx.remote(SEED_PRINCIPAL, capsule) });
    let listed = fx.call("offers_list", list("assistant")).await;
    assert_eq!(listed["ok"], true, "{listed}");
    let offers = &listed["result"]["data"]["offers"];
    assert_eq!(offers.as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(offers[0]["id"], "qwen-local");
    assert!(!listed.to_string().contains("gpt-hosted"), "{listed}");
    let foreign = fx.call("offers_list", list("browser")).await;
    assert_eq!(foreign["ok"], false);
    assert_eq!(foreign["code"], "denied");
    assert_eq!(fx.provider_ops().await, vec!["offers_list"]);
    fx.shutdown().await;
}

#[tokio::test]
async fn runs_create_rebinds_the_request_to_the_remote_principal() {
    let fx = TwoRuntimes::start().await;
    let created = fx.create_run("seed-req-1", "qwen-local").await;
    assert_eq!(created["ok"], true, "{created}");
    let expected_principal = fx.remote_principal();
    let run_id = run_id_for(&expected_principal, "seed-req-1");
    assert_eq!(created["result"]["data"]["run_id"], run_id);
    let native = fx.last_provider_call().await;
    assert_eq!(
        keys(&native),
        BTreeSet::from(["input", "offer_id", "op", "operation", "runtime_binding"])
    );
    assert_eq!(
        native["runtime_binding"]["principal_id"],
        expected_principal
    );
    assert_eq!(native["runtime_binding"]["capsule_id"], "assistant");
    assert_eq!(native["runtime_binding"]["session_id"], fx.grant_id);
    assert_eq!(native["runtime_binding"]["grant_id"], fx.grant_id);
    assert_eq!(native["runtime_binding"]["request_id"], "seed-req-1");
    let record = fx.run_record(&run_id);
    assert_eq!(record["source_endpoint_did"], fx.seed_did);
    assert_eq!(record["requester_principal_id"], SEED_PRINCIPAL);
    assert_eq!(record["remote_principal_id"], expected_principal);
    assert_eq!(record["grant_id"], fx.grant_id);
    assert_eq!(record["offer_id"], "qwen-local");
    assert!(record["terminal_at"].is_null(), "{record}");

    let hosted = fx.create_run("seed-req-hosted", "gpt-hosted").await;
    assert_eq!(hosted["ok"], false, "{hosted}");
    assert_eq!(hosted["code"], "offer_unavailable");
    assert_eq!(
        fx.provider_ops().await,
        vec!["offers_list", "runs_create", "offers_list"],
        "a hosted offer never reaches runs_create"
    );
    fx.shutdown().await;
}

#[tokio::test]
async fn run_lifecycle_stays_with_the_creating_principal_through_denial_and_revoke() {
    let fx = TwoRuntimes::start().await;
    let created = fx.create_run("seed-req-1", "qwen-local").await;
    assert_eq!(created["ok"], true, "{created}");
    let run_id = run_id_for(&fx.remote_principal(), "seed-req-1");
    assert_eq!(created["result"]["data"]["run_id"], run_id);
    let ops_before = fx.provider_ops().await;

    let other = fx
        .run_operation("runs_get", &run_id, OTHER_SEED_PRINCIPAL)
        .await;
    assert_eq!(other["ok"], false, "{other}");
    assert_eq!(other["code"], "denied");
    assert_eq!(fx.provider_ops().await, ops_before);

    let first = fx
        .run_operation("runs_events", &run_id, SEED_PRINCIPAL)
        .await;
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(
        first["result"]["data"]["events"].as_array().unwrap().len(),
        2
    );
    // An events page has no status; only its terminal event settles the record.
    assert!(first["result"]["data"].get("status").is_none(), "{first}");
    assert!(fx.run_record(&run_id)["terminal_at"].is_null());
    let second = fx
        .run_operation("runs_events", &run_id, SEED_PRINCIPAL)
        .await;
    assert_eq!(second["ok"], true, "{second}");
    assert_eq!(second["result"]["data"]["events"][0]["terminal"], true);
    let record = fx.run_record(&run_id);
    assert!(record["terminal_at"].is_u64(), "{record}");
    let binding = fx.last_provider_call().await["runtime_binding"].clone();
    assert_eq!(binding["principal_id"], fx.remote_principal());
    assert_eq!(binding["run_id"], run_id);

    let open = fx.create_run("seed-req-2", "qwen-local").await;
    assert_eq!(open["ok"], true, "{open}");
    let open_run_id = run_id_for(&fx.remote_principal(), "seed-req-2");
    fx.write_request_record("denied");
    let blocked = fx.create_run("seed-req-3", "qwen-local").await;
    assert_eq!(blocked["ok"], false, "{blocked}");
    assert_eq!(blocked["code"], "denied");
    let read = fx.run_operation("runs_get", &run_id, SEED_PRINCIPAL).await;
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(read["result"]["data"]["run_id"], run_id);
    assert_eq!(fx.last_provider_call().await["op"], "runs_get");

    let cancelled = cancel_grant_runs(fx.registry.clone(), fx.owner.path(), &fx.grant_id)
        .await
        .unwrap();
    assert_eq!(cancelled, vec![open_run_id.clone()]);
    let cancel = fx.last_provider_call().await;
    assert_eq!(cancel["op"], "runs_cancel");
    assert_eq!(cancel["run_id"], open_run_id);
    assert_eq!(
        cancel["runtime_binding"]["principal_id"],
        fx.remote_principal()
    );
    assert_eq!(cancel["runtime_binding"]["grant_id"], fx.grant_id);
    assert_eq!(cancel["runtime_binding"]["request_id"], "revoke:seed-req-2");
    fx.shutdown().await;
}

#[tokio::test]
async fn revoke_before_dispatch_sends_no_create() {
    let fx = TwoRuntimes::start().await;
    let barrier = install_create_race_barrier(fx.owner.path(), CreateRacePoint::BeforeDispatch);
    let (created, _) = tokio::join!(
        fx.create_run("seed-req-revoke-before", "qwen-local"),
        async {
            barrier.wait_prepared().await;
            fx.revoke_grant().await.unwrap();
            barrier.release();
        }
    );
    assert_eq!(created["ok"], false, "{created}");
    assert_eq!(created["code"], "denied");
    assert_eq!(fx.provider_ops().await, vec!["offers_list"]);
    assert!(fx
        .run_record(&run_id_for(
            &fx.remote_principal(),
            "seed-req-revoke-before"
        ))
        .is_null());
    fx.shutdown().await;
}

#[tokio::test]
async fn revoke_after_dispatch_keeps_ownership_and_cancels() {
    let fx = TwoRuntimes::start().await;
    let barrier = install_create_race_barrier(fx.owner.path(), CreateRacePoint::BeforeCommit);
    let (created, _) = tokio::join!(
        fx.create_run("seed-req-revoke-after", "qwen-local"),
        async {
            barrier.wait_prepared().await;
            fx.revoke_grant().await.unwrap();
            barrier.release();
        }
    );
    assert_eq!(created["ok"], true, "{created}");
    let run_id = run_id_for(&fx.remote_principal(), "seed-req-revoke-after");
    let record = fx.run_record(&run_id);
    assert_eq!(record["grant_id"], fx.grant_id);
    assert_eq!(record["request_id"], "seed-req-revoke-after");
    assert_eq!(record["terminal_status"], "cancelled");
    let ops = fx.provider_ops().await;
    assert_eq!(
        ops.iter().filter(|op| *op == "runs_create").count(),
        1,
        "{ops:?}"
    );
    assert!(ops.iter().any(|op| op == "runs_cancel"), "{ops:?}");
    fx.shutdown().await;
}

#[tokio::test]
async fn a_mismatched_create_reply_keeps_the_run_and_cancels() {
    let fx = TwoRuntimes::start().await;
    *fx.provider.next_create_data.lock().unwrap() = Some(json!({ "offer_id": "other-local" }));
    let created = fx.create_run("seed-req-mismatch", "qwen-local").await;
    assert_eq!(created["ok"], false, "{created}");
    assert_eq!(created["code"], "invalid_provider_invocation", "{created}");
    let run_id = run_id_for(&fx.remote_principal(), "seed-req-mismatch");
    let record = fx.run_record(&run_id);
    assert_eq!(record["request_id"], "seed-req-mismatch");
    assert_eq!(record["offer_id"], "qwen-local");
    assert_eq!(record["terminal_status"], "cancelled");
    assert_eq!(fx.pending_len(), 0);
    assert_eq!(fx.create_count().await, 1);
    let ops = fx.provider_ops().await;
    assert!(ops.iter().any(|op| op == "runs_cancel"), "{ops:?}");
    let retried = fx.create_run("seed-req-mismatch", "qwen-local").await;
    assert_eq!(retried["ok"], true, "{retried}");
    assert_eq!(retried["result"]["data"]["run_id"], run_id);
    assert_eq!(fx.create_count().await, 1, "retry must not dispatch again");
    fx.shutdown().await;
}

#[tokio::test]
async fn an_ambiguous_create_reply_keeps_the_reservation() {
    let fx = TwoRuntimes::start().await;
    *fx.provider.next_create_alias_run_id.lock().unwrap() =
        Some("run:sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into());
    let created = fx.create_run("seed-req-ambiguous", "qwen-local").await;
    assert_eq!(created["ok"], false, "{created}");
    assert_eq!(created["code"], "invalid_provider_invocation", "{created}");
    assert!(fx
        .run_record(&run_id_for(&fx.remote_principal(), "seed-req-ambiguous"))
        .is_null());
    assert_eq!(fx.pending_len(), 1);
    assert_eq!(fx.create_count().await, 1);
    let ops = fx.provider_ops().await;
    assert!(
        ops.iter().all(|op| op != "runs_cancel"),
        "no trustworthy run id, no cancel: {ops:?}"
    );
    let retried = fx.create_run("seed-req-ambiguous", "qwen-local").await;
    assert_eq!(retried["ok"], false, "{retried}");
    assert_eq!(
        fx.create_count().await,
        1,
        "held reservation blocks redispatch"
    );
    fx.shutdown().await;
}

impl TwoRuntimes {
    /// Fill the owner's run index with `count` open records, optionally
    /// including the record a specific earlier request would have left.
    fn fill_run_index(&self, count: usize, recorded_request: Option<&str>) {
        let mut runs = serde_json::Map::new();
        let record = |request_id: String| {
            json!({
                "grant_id": self.grant_id, "source_endpoint_did": self.seed_did,
                "requester_principal_id": SEED_PRINCIPAL,
                "remote_principal_id": self.remote_principal(), "capsule_id": "assistant",
                "offer_id": "qwen-local", "operation": "text.generate",
                "request_id": request_id,
                "input_hash": model_input_hash(&json!({ "prompt": "hello" })).unwrap(),
                "created_at": now_ts(),
            })
        };
        for n in 0..count {
            runs.insert(format!("run:sha256:{n:064x}"), record(format!("open-{n}")));
        }
        if let Some(request_id) = recorded_request {
            runs.insert(
                run_id_for(&self.remote_principal(), request_id),
                record(request_id.to_string()),
            );
        }
        std::fs::write(
            self.owner.path().join("services-model-runs.json"),
            serde_json::to_vec(
                &json!({ "schema": "elastos.services.model-runs/v1", "runs": runs }),
            )
            .unwrap(),
        )
        .unwrap();
    }

    fn strip_record_input_hash(&self, run_id: &str) {
        let path = self.owner.path().join("services-model-runs.json");
        let mut index: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        index["runs"][run_id]
            .as_object_mut()
            .unwrap()
            .remove("input_hash");
        std::fs::write(path, serde_json::to_vec(&index).unwrap()).unwrap();
    }

    fn run_index_len(&self) -> usize {
        let path = self.owner.path().join("services-model-runs.json");
        let index: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        index["runs"].as_object().unwrap().len()
            + index["pending"].as_object().map_or(0, |p| p.len())
    }

    fn pending_len(&self) -> usize {
        let path = self.owner.path().join("services-model-runs.json");
        let index: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        index["pending"].as_object().map_or(0, |p| p.len())
    }
}

#[tokio::test]
async fn a_full_run_index_refuses_before_any_model_work_starts() {
    let fx = TwoRuntimes::start().await;
    fx.fill_run_index(512, None);
    let refused = fx.create_run("seed-req-full", "qwen-local").await;
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["code"], "rate_limited");
    // Only the offer check reached the provider; no run was dispatched.
    assert_eq!(fx.provider_ops().await, vec!["offers_list"]);
    assert!(fx
        .run_record(&run_id_for(&fx.remote_principal(), "seed-req-full"))
        .is_null());
    fx.shutdown().await;
}

#[tokio::test]
async fn two_creates_at_the_last_slot_dispatch_exactly_one_run() {
    let fx = TwoRuntimes::start().await;
    fx.fill_run_index(511, None);
    let (a, b) = tokio::join!(
        fx.create_run("seed-req-race-a", "qwen-local"),
        fx.create_run("seed-req-race-b", "qwen-local"),
    );
    let outcomes = [a["ok"] == true, b["ok"] == true];
    assert_eq!(
        outcomes.iter().filter(|ok| **ok).count(),
        1,
        "exactly one create wins the slot: {a} {b}"
    );
    let refused = if a["ok"] == true { &b } else { &a };
    assert_eq!(refused["code"], "rate_limited", "{refused}");
    let dispatched = fx
        .provider_ops()
        .await
        .iter()
        .filter(|op| *op == "runs_create")
        .count();
    assert_eq!(dispatched, 1, "the refused create dispatched nothing");
    assert_eq!(fx.run_index_len(), 512, "no slot stays held after commit");
    fx.shutdown().await;
}

#[tokio::test]
async fn a_recorded_request_retries_through_a_full_index_without_a_new_slot() {
    let fx = TwoRuntimes::start().await;
    // The reply to "seed-req-lost" was lost after its run was recorded, and
    // the index has since filled up.
    fx.fill_run_index(511, Some("seed-req-lost"));
    assert_eq!(fx.run_index_len(), 512);
    let retried = fx.create_run("seed-req-lost", "qwen-local").await;
    assert_eq!(retried["ok"], true, "{retried}");
    assert_eq!(
        retried["result"]["data"]["run_id"],
        run_id_for(&fx.remote_principal(), "seed-req-lost")
    );
    assert_eq!(fx.last_provider_call().await["op"], "runs_get");
    assert_eq!(
        fx.create_count().await,
        0,
        "a recorded retry must not create again"
    );
    assert_eq!(fx.run_index_len(), 512, "the retry consumed no capacity");
    // An unrelated new request is still refused.
    let refused = fx.create_run("seed-req-new", "qwen-local").await;
    assert_eq!(refused["code"], "rate_limited", "{refused}");
    fx.shutdown().await;
}

// The seed gateway's own routing over Carrier: the path the Assistant uses.
mod consumer_path {
    use super::*;
    use crate::api::gateway::gateway_model_remote::{
        remote_offer_id, remote_run_route, route_run_operation, ConsumerModelGrant,
        RemoteRouteError, REMOTE_RUN_INDEX_MAX,
    };

    fn ticket_for(endpoint: &iroh::EndpointAddr) -> String {
        let bytes = serde_json::to_vec(&json!({ "topic": null, "endpoints": [endpoint] })).unwrap();
        let mut encoded = data_encoding::BASE32_NOPAD.encode(&bytes);
        encoded.make_ascii_lowercase();
        encoded
    }

    fn seed_context() -> HomeLaunchTokenContext {
        HomeLaunchTokenContext {
            principal_id: SEED_PRINCIPAL.to_string(),
            session_id: "auth:seed".to_string(),
            proof_binding_id: None,
            grant_id: "grant:seed-launch".to_string(),
        }
    }

    impl TwoRuntimes {
        fn consumer_grant(&self) -> ConsumerModelGrant {
            ConsumerModelGrant::from_record(&json!({
                "grant_id": self.grant_id,
                "peer_did": self.owner_service.endpoint().unwrap().id().to_string(),
                "connect_ticket": ticket_for(&self.owner_addr),
                "service_display_name": "Mac model",
                "expires_at": now_ts() + 3600,
            }))
            .expect("grant record")
        }

        async fn route(
            &self,
            grants: &[ConsumerModelGrant],
            op: &str,
            normalized: Value,
        ) -> Result<Option<Value>, RemoteRouteError> {
            self.route_as("assistant", grants, op, normalized).await
        }

        async fn route_as(
            &self,
            capsule_id: &str,
            grants: &[ConsumerModelGrant],
            op: &str,
            normalized: Value,
        ) -> Result<Option<Value>, RemoteRouteError> {
            route_run_operation(
                self.seed_registry.clone(),
                self.seed.path(),
                grants,
                &seed_context(),
                capsule_id,
                (op, &normalized),
                now_ts(),
            )
            .await
        }

        fn fill_consumer_run_index(
            &self,
            count: usize,
            recorded_request: Option<&str>,
            capsule_id: &str,
        ) {
            let mut runs = serde_json::Map::new();
            let record = |request_id: String| {
                json!({
                    "principal_id": SEED_PRINCIPAL,
                    "grant_id": self.grant_id,
                    "peer_did": "did:key:z6Mkpeer",
                    "connect_ticket": "ticket",
                    "display_name": "Mac",
                    "offer_id": remote_offer_id(&self.grant_id, "qwen-local"),
                    "request_id": request_id,
                    "capsule_id": capsule_id,
                    "created_at": now_ts(),
                })
            };
            for n in 0..count {
                runs.insert(format!("run:sha256:{n:064x}"), record(format!("open-{n}")));
            }
            if let Some(request_id) = recorded_request {
                runs.insert(
                    format!("run:sha256:{:064x}", 0),
                    record(request_id.to_string()),
                );
            }
            std::fs::write(
                self.seed.path().join("services-model-remote-runs.json"),
                serde_json::to_vec(&json!({
                    "schema": "elastos.services.model-remote-runs/v1",
                    "runs": runs,
                }))
                .unwrap(),
            )
            .unwrap();
        }
    }

    fn access(op: &str, run_id: &str, request_id: &str) -> Value {
        json!({ "op": op, "run_id": run_id, "after_sequence": 0,
            "runtime_binding": { "principal_id": SEED_PRINCIPAL, "request_id": request_id } })
    }

    #[tokio::test]
    async fn route_run_operation_rewrites_protocol_offer_ids_only() {
        let fx = TwoRuntimes::start().await;
        let grant = fx.consumer_grant();
        *fx.provider.next_create_data.lock().unwrap() = Some(json!({
            "output": { "offer_id": "nested-user-offer", "text": "keep" }
        }));
        let created = fx
            .route(
                std::slice::from_ref(&grant),
                "runs_create",
                json!({
                    "op": "runs_create",
                    "offer_id": remote_offer_id(&fx.grant_id, "qwen-local"),
                    "operation": "text.generate",
                    "input": { "prompt": "hello" },
                    "runtime_binding": { "principal_id": SEED_PRINCIPAL, "request_id": "seed-req-offer" }
                }),
            )
            .await
            .unwrap()
            .expect("remote route");
        assert_eq!(
            created["data"]["offer_id"],
            remote_offer_id(&fx.grant_id, "qwen-local")
        );
        assert_eq!(created["data"]["output"]["offer_id"], "nested-user-offer");
        assert_eq!(created["data"]["output"]["text"], "keep");
        fx.shutdown().await;
    }

    #[tokio::test]
    async fn existing_runs_settle_after_the_grant_is_denied_while_new_runs_are_refused() {
        let fx = TwoRuntimes::start().await;
        let grant = fx.consumer_grant();
        let create = json!({ "op": "runs_create", "offer_id": remote_offer_id(&fx.grant_id, "qwen-local"),
            "operation": "text.generate", "input": { "prompt": "hello" },
            "runtime_binding": { "principal_id": SEED_PRINCIPAL, "request_id": "seed-req-c" } });
        let created = fx
            .route(std::slice::from_ref(&grant), "runs_create", create.clone())
            .await
            .unwrap()
            .expect("remote route");
        let run_id = run_id_for(&fx.remote_principal(), "seed-req-c");
        assert_eq!(created["data"]["run_id"], run_id);
        let route = remote_run_route(fx.seed.path(), SEED_PRINCIPAL, &run_id)
            .unwrap()
            .expect("stored route");
        assert_eq!(route.grant_id, fx.grant_id);
        assert_eq!(route.connect_ticket, grant.connect_ticket);
        assert!(route.terminal_at.is_none());

        // A retry of the same request (its reply was lost) reuses the route
        // and the owner's record; both sides answer with the same run.
        let retried = fx
            .route(std::slice::from_ref(&grant), "runs_create", create.clone())
            .await
            .unwrap()
            .expect("remote route");
        assert_eq!(retried["data"]["run_id"], run_id);
        let seed_index: Value = serde_json::from_slice(
            &std::fs::read(fx.seed.path().join("services-model-remote-runs.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(seed_index["runs"].as_object().unwrap().len(), 1);
        assert!(seed_index["pending"]
            .as_object()
            .is_none_or(|p| p.is_empty()));

        // The owner denies the request: no active grant remains on the seed.
        fx.write_request_record("denied");
        let refused = fx.route(&[], "runs_create", create).await;
        assert!(
            matches!(refused, Err(RemoteRouteError::Rejected { ref code }) if code == "denied"),
            "{refused:?}"
        );

        let page = fx
            .route(
                &[],
                "runs_events",
                access("runs_events", &run_id, "seed-ev-1"),
            )
            .await
            .unwrap()
            .expect("settlement route");
        assert_eq!(page["data"]["events"].as_array().unwrap().len(), 2);
        assert_eq!(fx.last_provider_call().await["op"], "runs_events");
        let last = fx
            .route(
                &[],
                "runs_events",
                access("runs_events", &run_id, "seed-ev-2"),
            )
            .await
            .unwrap()
            .expect("settlement route");
        assert_eq!(last["data"]["events"][0]["terminal"], true);
        let settled = remote_run_route(fx.seed.path(), SEED_PRINCIPAL, &run_id)
            .unwrap()
            .expect("stored route");
        assert!(settled.terminal_at.is_some(), "{settled:?}");
        assert!(fx.run_record(&run_id)["terminal_at"].is_u64());

        // A run this Home never routed remotely stays on the local path.
        let local = fx
            .route(
                &[],
                "runs_get",
                access("runs_get", "run:sha256:00", "seed-local"),
            )
            .await
            .unwrap();
        assert!(local.is_none());
        fx.shutdown().await;
    }

    #[tokio::test]
    async fn a_full_consumer_index_refuses_a_second_capsule_before_any_model_work() {
        let fx = TwoRuntimes::start().await;
        let grant = fx.consumer_grant();
        fx.fill_consumer_run_index(REMOTE_RUN_INDEX_MAX, Some("seed-req-shared"), "assistant");
        let ops_before = fx.provider_ops().await;
        let creates_before = fx.create_count().await;
        let create = json!({
            "op": "runs_create",
            "offer_id": remote_offer_id(&fx.grant_id, "qwen-local"),
            "operation": "text.generate",
            "input": { "prompt": "hello" },
            "runtime_binding": {
                "principal_id": SEED_PRINCIPAL,
                "request_id": "seed-req-shared",
            },
        });
        let refused = fx
            .route_as(
                "home-agent",
                std::slice::from_ref(&grant),
                "runs_create",
                create,
            )
            .await;
        assert!(
            matches!(refused, Err(RemoteRouteError::Rejected { ref code }) if code == "rate_limited"),
            "{refused:?}"
        );
        assert_eq!(fx.provider_ops().await, ops_before);
        assert_eq!(fx.create_count().await, creates_before);
        fx.shutdown().await;
    }
}

// Denial authority: the sweep follows only a denial this principal made and
// this Runtime saved. Delivery to the requester is a separate step.
mod denial_authority {
    use super::*;
    use crate::api::gateway::gateway_model_service::deny_grant_and_settle;

    impl TwoRuntimes {
        fn owner_context(&self) -> HomeLaunchTokenContext {
            HomeLaunchTokenContext {
                principal_id: self.authority.principal_id.clone(),
                session_id: self.authority.session_id.clone(),
                proof_binding_id: Some(self.authority.proof_binding_id.clone()),
                grant_id: self.authority.grant_id.clone(),
            }
        }

        async fn deny_as(&self, context: &HomeLaunchTokenContext) -> anyhow::Result<String> {
            deny_grant_and_settle(
                Some(self.registry.clone()),
                self.owner.path(),
                context,
                None,
                REQUEST_ID,
            )
            .await
        }

        fn requests_state_dir(&self) -> std::path::PathBuf {
            let root = crate::auth::principal_localhost_root(&self.authority.principal_id);
            elastos_common::localhost::rooted_localhost_fs_path(
                self.owner.path(),
                &format!("{root}/.AppData/ElastOS/Home/services-requests.json"),
            )
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
        }

        fn owner_inbox_app(&self) -> axum::Router {
            let mut state = test_state(self.owner.path());
            state.provider_registry = Some(self.registry.clone());
            gateway_router(state)
        }

        async fn post_inbox_action(
            app: &axum::Router,
            token: &str,
            action_id: &str,
        ) -> (StatusCode, String) {
            let response = app
                .clone()
                .oneshot(
                    test_browser_request("localhost:61180", "null")
                        .method("POST")
                        .uri("/api/apps/inbox/actions")
                        .header("x-elastos-home-token", token)
                        .header(CONTENT_TYPE, "application/json")
                        .body(Body::from(json!({ "action_id": action_id }).to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            (status, String::from_utf8_lossy(&body).into_owned())
        }
    }

    #[tokio::test]
    async fn another_principal_cannot_deny_or_cancel_the_owners_runs() {
        let fx = TwoRuntimes::start().await;
        let open = fx.create_run("seed-req-open", "qwen-local").await;
        assert_eq!(open["ok"], true, "{open}");
        let ops_before = fx.provider_ops().await;
        let intruder = HomeLaunchTokenContext {
            principal_id: "person:local:intruder".to_string(),
            session_id: "auth:intruder".to_string(),
            proof_binding_id: Some("proof:passkey:intruder".to_string()),
            grant_id: "grant:intruder".to_string(),
        };
        let result = fx.deny_as(&intruder).await;
        assert!(result.is_err(), "{result:?}");
        assert_eq!(fx.provider_ops().await, ops_before, "no cancel may follow");
        assert_eq!(
            fx.create_run("seed-req-still-open", "qwen-local").await["ok"],
            true,
            "the grant stays approved"
        );
        fx.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_denial_that_could_not_be_saved_cancels_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let fx = TwoRuntimes::start().await;
        let open = fx.create_run("seed-req-open", "qwen-local").await;
        assert_eq!(open["ok"], true, "{open}");
        let ops_before = fx.provider_ops().await;
        let state_dir = fx.requests_state_dir();
        let mode = std::fs::metadata(&state_dir).unwrap().permissions();
        std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let result = fx.deny_as(&fx.owner_context()).await;
        std::fs::set_permissions(&state_dir, mode).unwrap();
        assert!(result.is_err(), "{result:?}");
        assert_eq!(fx.provider_ops().await, ops_before, "no cancel may follow");
        fx.shutdown().await;
    }

    #[tokio::test]
    async fn a_saved_denial_cancels_the_open_run_even_when_delivery_fails() {
        let fx = TwoRuntimes::start().await;
        let open = fx.create_run("seed-req-open", "qwen-local").await;
        assert_eq!(open["ok"], true, "{open}");
        let open_run_id = run_id_for(&fx.remote_principal(), "seed-req-open");
        // No discovery runtime: the decision cannot be delivered, yet it is saved.
        let result = fx.deny_as(&fx.owner_context()).await;
        assert!(result.is_err(), "delivery failure surfaces: {result:?}");
        let cancel = fx.last_provider_call().await;
        assert_eq!(cancel["op"], "runs_cancel");
        assert_eq!(cancel["run_id"], open_run_id);
        assert_eq!(cancel["runtime_binding"]["grant_id"], fx.grant_id);
        let refused = fx.create_run("seed-req-after", "qwen-local").await;
        assert_eq!(refused["code"], "denied", "{refused}");
        fx.shutdown().await;
    }

    #[tokio::test]
    async fn inbox_revoke_cancels_the_open_run_after_the_owner_saves_denial() {
        let fx = TwoRuntimes::start().await;
        let open = fx.create_run("seed-req-open", "qwen-local").await;
        assert_eq!(open["ok"], true, "{open}");
        let open_run_id = run_id_for(&fx.remote_principal(), "seed-req-open");
        let app = fx.owner_inbox_app();
        let (status, _) = home_test_post_json(
            &app,
            "/api/apps/home/launch",
            &fx.authority.home_token,
            "http://localhost:61180",
            json!({ "target": INBOX_CAPSULE_ID }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let token = app_token_for_authority(fx.owner.path(), INBOX_CAPSULE_ID, &fx.authority);
        let (status, inbox) =
            home_test_get_json(&app, "/api/apps/inbox/summary", &token, "null").await;
        assert_eq!(status, StatusCode::OK, "{inbox}");
        assert_eq!(inbox["notifications"]["unread_count"], 0, "{inbox}");
        assert_eq!(inbox["notifications"]["attention_count"], 0, "{inbox}");
        let grant = inbox["notifications"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["kind"] == "service_access_grant")
            .expect("owner Inbox should show the approved model grant");
        assert_eq!(grant["read"], true);
        assert_eq!(
            grant["action_ref"]["action_id"],
            format!("service-deny-request:{REQUEST_ID}")
        );
        let (status, body) = TwoRuntimes::post_inbox_action(
            &app,
            &token,
            &format!("service-deny-request:{REQUEST_ID}"),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(
            body.contains("service access request delivery failed"),
            "{body}"
        );
        let cancel = fx.last_provider_call().await;
        assert_eq!(cancel["op"], "runs_cancel");
        assert_eq!(cancel["run_id"], open_run_id);
        assert_eq!(cancel["runtime_binding"]["grant_id"], fx.grant_id);
        let refused = fx.create_run("seed-req-after", "qwen-local").await;
        assert_eq!(refused["code"], "denied", "{refused}");
        let (status, inbox) =
            home_test_get_json(&app, "/api/apps/inbox/summary", &token, "null").await;
        assert_eq!(status, StatusCode::OK, "{inbox}");
        assert!(
            !inbox["notifications"]["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["kind"] == "service_access_grant"),
            "{inbox}"
        );
        fx.shutdown().await;
    }

    #[tokio::test]
    async fn another_principal_cannot_revoke_the_owners_grant_from_inbox() {
        let fx = TwoRuntimes::start().await;
        let open = fx.create_run("seed-req-open", "qwen-local").await;
        assert_eq!(open["ok"], true, "{open}");
        let ops_before = fx.provider_ops().await;
        let app = fx.owner_inbox_app();
        let other = passkey_authority_with_name_role(
            fx.owner.path(),
            Some("other"),
            crate::auth::RuntimePrincipalRole::Guest,
        );
        let (status, _) = home_test_post_json(
            &app,
            "/api/apps/home/launch",
            &other.home_token,
            "http://localhost:61180",
            json!({ "target": INBOX_CAPSULE_ID }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let token = app_token_for_authority(fx.owner.path(), INBOX_CAPSULE_ID, &other);
        let (status, inbox) =
            home_test_get_json(&app, "/api/apps/inbox/summary", &token, "null").await;
        assert_eq!(status, StatusCode::OK, "{inbox}");
        assert!(
            !inbox["notifications"]["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["kind"] == "service_access_grant"),
            "{inbox}"
        );
        let (status, body) = TwoRuntimes::post_inbox_action(
            &app,
            &token,
            &format!("service-deny-request:{REQUEST_ID}"),
        )
        .await;
        assert_ne!(status, StatusCode::OK, "{body}");
        assert_eq!(fx.provider_ops().await, ops_before, "no cancel may follow");
        assert_eq!(
            fx.create_run("seed-req-still-open", "qwen-local").await["ok"],
            true,
            "the grant stays approved"
        );
        fx.shutdown().await;
    }
}

// The provider journal prunes a run after its retention; the owning Runtime's
// record outlives it and answers the consumer's settlement read.
#[tokio::test]
async fn a_pruned_run_settles_from_the_owners_record() {
    let fx = TwoRuntimes::start().await;
    // Run A settles through its events page; run B is never observed settling.
    let a = fx.create_run("seed-req-a", "qwen-local").await;
    assert_eq!(a["ok"], true, "{a}");
    let run_a = run_id_for(&fx.remote_principal(), "seed-req-a");
    for _ in 0..2 {
        let page = fx
            .run_operation("runs_events", &run_a, SEED_PRINCIPAL)
            .await;
        assert_eq!(page["ok"], true, "{page}");
    }
    assert_eq!(fx.run_record(&run_a)["terminal_status"], "completed");
    let b = fx.create_run("seed-req-b", "qwen-local").await;
    assert_eq!(b["ok"], true, "{b}");
    let run_b = run_id_for(&fx.remote_principal(), "seed-req-b");
    assert!(fx.run_record(&run_b)["terminal_status"].is_null());

    // The provider forgets both runs (journal pruned).
    fx.provider.run_owners.lock().unwrap().clear();

    let settled = fx.run_operation("runs_get", &run_a, SEED_PRINCIPAL).await;
    assert_eq!(settled["ok"], true, "{settled}");
    assert_eq!(settled["result"]["data"]["status"], "completed");
    assert_eq!(settled["result"]["data"]["terminal"]["status"], "completed");
    assert_eq!(settled["result"]["data"]["output_retained"], false);
    assert_eq!(
        settled["result"]["data"]["settlement_source"],
        "runtime_index"
    );
    let page = fx
        .run_operation("runs_events", &run_a, SEED_PRINCIPAL)
        .await;
    assert_eq!(page["ok"], true, "{page}");
    assert_eq!(page["result"]["data"]["has_more"], false);
    let events = page["result"]["data"]["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "{page}");
    assert_eq!(events[0]["kind"], "completed");
    assert_eq!(events[0]["data"]["output_retained"], false);
    assert_eq!(events[0]["terminal"], true);
    assert_eq!(page["result"]["data"]["output_retained"], false);
    assert_eq!(page["result"]["data"]["next_cursor"], 1);
    assert_eq!(page["result"]["data"]["settlement_source"], "runtime_index");

    let unknown = fx.run_operation("runs_get", &run_b, SEED_PRINCIPAL).await;
    assert_eq!(unknown["ok"], true, "{unknown}");
    assert_eq!(unknown["result"]["data"]["status"], "settlement_unknown");
    assert_eq!(
        unknown["result"]["data"]["terminal"]["status"],
        "settlement_unknown"
    );
    assert_eq!(
        fx.run_record(&run_b)["terminal_status"],
        "settlement_unknown"
    );

    // Another principal still learns nothing about either run.
    let foreign = fx
        .run_operation("runs_get", &run_a, OTHER_SEED_PRINCIPAL)
        .await;
    assert_eq!(foreign["code"], "denied", "{foreign}");
    fx.shutdown().await;
}

#[tokio::test]
async fn a_completed_run_does_not_dispatch_again_after_the_journal_is_pruned() {
    let fx = TwoRuntimes::start().await;
    let created = fx.create_run("seed-req-replay", "qwen-local").await;
    assert_eq!(created["ok"], true, "{created}");
    let run_id = run_id_for(&fx.remote_principal(), "seed-req-replay");
    for _ in 0..2 {
        let page = fx
            .run_operation("runs_events", &run_id, SEED_PRINCIPAL)
            .await;
        assert_eq!(page["ok"], true, "{page}");
    }
    assert_eq!(fx.run_record(&run_id)["terminal_status"], "completed");
    let creates_before = fx.create_count().await;
    assert_eq!(creates_before, 1);
    fx.provider.run_owners.lock().unwrap().clear();

    let retried = fx.create_run("seed-req-replay", "qwen-local").await;
    assert_eq!(retried["ok"], true, "{retried}");
    assert_eq!(retried["result"]["data"]["run_id"], run_id);
    assert_eq!(retried["result"]["data"]["status"], "completed");
    assert_eq!(
        retried["result"]["data"]["settlement_source"],
        "runtime_index"
    );
    assert_eq!(
        fx.create_count().await,
        creates_before,
        "replay after journal cleanup must not dispatch again"
    );
    fx.shutdown().await;
}

#[tokio::test]
async fn a_recorded_request_rejects_a_conflicting_offer_on_retry() {
    let fx = TwoRuntimes::start().await;
    let created = fx.create_run("seed-req-bind", "qwen-local").await;
    assert_eq!(created["ok"], true, "{created}");
    let run_id = run_id_for(&fx.remote_principal(), "seed-req-bind");
    let path = fx.owner.path().join("services-model-runs.json");
    let mut index: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    index["runs"][&run_id]["offer_id"] = json!("other-local");
    std::fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();
    let retried = fx.create_run("seed-req-bind", "qwen-local").await;
    assert_eq!(retried["ok"], false, "{retried}");
    assert_eq!(retried["code"], "denied", "{retried}");
    assert_eq!(fx.create_count().await, 1);
    fx.shutdown().await;
}

#[tokio::test]
async fn a_provider_read_fault_stays_recoverable_instead_of_settling() {
    let fx = TwoRuntimes::start().await;
    let created = fx.create_run("seed-req-fault", "qwen-local").await;
    assert_eq!(created["ok"], true, "{created}");
    let run_id = run_id_for(&fx.remote_principal(), "seed-req-fault");
    for code in ["journal_corrupt", "internal_error", "not_initialized"] {
        *fx.provider.next_error.lock().unwrap() = Some(code.to_string());
        let get = fx.run_operation("runs_get", &run_id, SEED_PRINCIPAL).await;
        assert_eq!(get["ok"], false, "{code}: {get}");
        assert_eq!(get["code"], "provider_failure", "{code}: {get}");
        assert!(
            fx.run_record(&run_id)["terminal_status"].is_null(),
            "{code} must leave the owner record open"
        );
        *fx.provider.next_error.lock().unwrap() = Some(code.to_string());
        let events = fx
            .run_operation("runs_events", &run_id, SEED_PRINCIPAL)
            .await;
        assert_eq!(events["ok"], false, "{code}: {events}");
        assert_eq!(events["code"], "provider_failure", "{code}: {events}");
    }
    fx.shutdown().await;
}
#[tokio::test]
async fn a_lost_create_reply_recovers_completed_output_from_the_journal() {
    let fx = TwoRuntimes::start().await;
    let created = fx.create_run("seed-req-output", "qwen-local").await;
    assert_eq!(created["ok"], true, "{created}");
    let run_id = run_id_for(&fx.remote_principal(), "seed-req-output");
    for _ in 0..2 {
        let page = fx.run_events(&run_id, 0).await;
        assert_eq!(page["ok"], true, "{page}");
    }
    assert_eq!(fx.run_record(&run_id)["terminal_status"], "completed");
    let creates_before = fx.create_count().await;
    assert_eq!(creates_before, 1);

    let retried = fx.create_run("seed-req-output", "qwen-local").await;
    assert_eq!(retried["ok"], true, "{retried}");
    assert_eq!(retried["result"]["data"]["run_id"], run_id);
    assert_eq!(retried["result"]["data"]["status"], "completed");
    assert_eq!(
        retried["result"]["data"]["terminal"]["output"]["text"],
        "hello"
    );
    assert_ne!(
        retried["result"]["data"]["settlement_source"],
        "runtime_index"
    );
    assert_eq!(fx.last_provider_call().await["op"], "runs_get");
    assert_eq!(
        fx.create_count().await,
        creates_before,
        "a lost create reply must not dispatch again while the journal exists"
    );
    fx.shutdown().await;
}

#[tokio::test]
async fn an_attached_events_cursor_settles_after_the_journal_is_pruned() {
    let fx = TwoRuntimes::start().await;
    let created = fx.create_run("seed-req-cursor", "qwen-local").await;
    assert_eq!(created["ok"], true, "{created}");
    let run_id = run_id_for(&fx.remote_principal(), "seed-req-cursor");
    let first = fx.run_events(&run_id, 0).await;
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(first["result"]["data"]["events"][1]["sequence"], 2);
    fx.provider.run_owners.lock().unwrap().clear();

    let settled = fx.run_events(&run_id, 2).await;
    assert_eq!(settled["ok"], true, "{settled}");
    assert_eq!(settled["result"]["data"]["events"][0]["sequence"], 3);
    assert_eq!(settled["result"]["data"]["events"][0]["terminal"], true);
    assert_eq!(settled["result"]["data"]["next_cursor"], 3);
    assert_eq!(
        settled["result"]["data"]["settlement_source"],
        "runtime_index"
    );
    let high = fx.run_events(&run_id, 7).await;
    assert_eq!(high["ok"], true, "{high}");
    assert_eq!(high["result"]["data"]["events"][0]["sequence"], 8);
    assert_eq!(high["result"]["data"]["next_cursor"], 8);
    fx.shutdown().await;
}

#[tokio::test]
async fn a_changed_input_is_rejected_before_and_after_journal_prune() {
    let fx = TwoRuntimes::start().await;
    let created = fx.create_run("seed-req-input", "qwen-local").await;
    assert_eq!(created["ok"], true, "{created}");
    let run_id = run_id_for(&fx.remote_principal(), "seed-req-input");
    assert!(!fx.run_record(&run_id)["input_hash"]
        .as_str()
        .unwrap()
        .is_empty());
    let changed = fx
        .create_run_as(
            "seed-req-input",
            "qwen-local",
            "assistant",
            json!({ "prompt": "other" }),
        )
        .await;
    assert_eq!(changed["ok"], false, "{changed}");
    assert_eq!(changed["code"], "denied", "{changed}");
    assert_eq!(fx.create_count().await, 1);
    assert!(fx.run_record(&run_id)["terminal_status"].is_null());

    fx.provider.run_owners.lock().unwrap().clear();
    let changed_again = fx
        .create_run_as(
            "seed-req-input",
            "qwen-local",
            "assistant",
            json!({ "prompt": "other" }),
        )
        .await;
    assert_eq!(changed_again["ok"], false, "{changed_again}");
    assert_eq!(changed_again["code"], "denied", "{changed_again}");
    assert_eq!(fx.create_count().await, 1);
    assert!(fx.run_record(&run_id)["terminal_status"].is_null());
    fx.shutdown().await;
}

#[tokio::test]
async fn assistant_and_home_agent_keeps_separate_runs_for_the_same_request_id() {
    let fx = TwoRuntimes::start().await;
    let assistant = fx.create_run("seed-req-shared", "qwen-local").await;
    let home_agent = fx
        .create_run_as(
            "seed-req-shared",
            "qwen-local",
            "home-agent",
            json!({ "prompt": "hello" }),
        )
        .await;
    assert_eq!(assistant["ok"], true, "{assistant}");
    assert_eq!(home_agent["ok"], true, "{home_agent}");
    let assistant_id = run_id_for_capsule(&fx.remote_principal(), "seed-req-shared", "assistant");
    let home_id = run_id_for_capsule(&fx.remote_principal(), "seed-req-shared", "home-agent");
    assert_ne!(assistant_id, home_id);
    assert_eq!(assistant["result"]["data"]["run_id"], assistant_id);
    assert_eq!(home_agent["result"]["data"]["run_id"], home_id);
    assert_eq!(fx.create_count().await, 2);
    assert!(fx.run_record(&assistant_id)["terminal_status"].is_null());
    assert!(fx.run_record(&home_id)["terminal_status"].is_null());
    assert_eq!(fx.run_record(&assistant_id)["capsule_id"], "assistant");
    assert_eq!(fx.run_record(&home_id)["capsule_id"], "home-agent");

    let result = crate::api::gateway::gateway_model_service::deny_grant_and_settle(
        Some(fx.registry.clone()),
        fx.owner.path(),
        &HomeLaunchTokenContext {
            principal_id: fx.authority.principal_id.clone(),
            session_id: fx.authority.session_id.clone(),
            proof_binding_id: Some(fx.authority.proof_binding_id.clone()),
            grant_id: fx.authority.grant_id.clone(),
        },
        None,
        REQUEST_ID,
    )
    .await;
    assert!(result.is_err(), "delivery failure surfaces: {result:?}");
    let cancels = fx
        .provider_ops()
        .await
        .iter()
        .filter(|op| *op == "runs_cancel")
        .count();
    assert_eq!(cancels, 2, "each capsule run must be cancelled");
    fx.shutdown().await;
}

#[tokio::test]
async fn a_legacy_record_refuses_changed_input_replay_while_the_journal_exists() {
    let fx = TwoRuntimes::start().await;
    let created = fx.create_run("seed-req-legacy", "qwen-local").await;
    assert_eq!(created["ok"], true, "{created}");
    let run_id = run_id_for(&fx.remote_principal(), "seed-req-legacy");
    fx.strip_record_input_hash(&run_id);
    assert!(fx.run_record(&run_id).get("input_hash").is_none());

    let changed = fx
        .create_run_as(
            "seed-req-legacy",
            "qwen-local",
            "assistant",
            json!({ "prompt": "other" }),
        )
        .await;
    assert_eq!(changed["ok"], false, "{changed}");
    assert_eq!(changed["code"], "request_unbound", "{changed}");
    assert_eq!(fx.create_count().await, 1);
    assert_eq!(fx.last_provider_call().await["op"], "offers_list");

    let got = fx.run_operation("runs_get", &run_id, SEED_PRINCIPAL).await;
    assert_eq!(got["ok"], true, "{got}");
    assert_eq!(got["result"]["data"]["status"], "running", "{got}");
    let page = fx.run_events(&run_id, 0).await;
    assert_eq!(page["ok"], true, "{page}");
    let cancel = fx
        .run_operation("runs_cancel", &run_id, SEED_PRINCIPAL)
        .await;
    assert_eq!(cancel["ok"], true, "{cancel}");
    fx.shutdown().await;
}

#[tokio::test]
async fn a_legacy_record_refuses_changed_input_replay_after_the_journal_is_pruned() {
    let fx = TwoRuntimes::start().await;
    let created = fx.create_run("seed-req-legacy-pruned", "qwen-local").await;
    assert_eq!(created["ok"], true, "{created}");
    let run_id = run_id_for(&fx.remote_principal(), "seed-req-legacy-pruned");
    fx.strip_record_input_hash(&run_id);
    fx.provider.run_owners.lock().unwrap().clear();

    let changed = fx
        .create_run_as(
            "seed-req-legacy-pruned",
            "qwen-local",
            "assistant",
            json!({ "prompt": "other" }),
        )
        .await;
    assert_eq!(changed["ok"], false, "{changed}");
    assert_eq!(changed["code"], "request_unbound", "{changed}");
    assert_eq!(fx.create_count().await, 1);

    let got = fx.run_operation("runs_get", &run_id, SEED_PRINCIPAL).await;
    assert_eq!(got["ok"], true, "{got}");
    assert_eq!(
        got["result"]["data"]["status"], "settlement_unknown",
        "{got}"
    );
    let page = fx.run_events(&run_id, 0).await;
    assert_eq!(page["ok"], true, "{page}");
    assert_eq!(
        page["result"]["data"]["events"][0]["kind"],
        "settlement_unknown"
    );
    fx.shutdown().await;
}
