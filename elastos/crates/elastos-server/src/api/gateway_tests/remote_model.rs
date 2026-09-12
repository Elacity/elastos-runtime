use super::home_system::{
    configured_discovery_network_profile_for_test, write_home_principal_object_json_for_authority,
};
use super::*;
use crate::api::gateway::gateway_model_service::{
    cancel_grant_runs, model_grant_id, remote_principal_id, settle_denied_grant,
};
use crate::collaboration_contact_store::CollaborationContactStore;
use crate::collaboration_discovery::*;
use crate::collaboration_profile_authority::{
    signed_profile_document_for_test, VerifiedCollaborationProfileDocument,
};
use elastos_common::collaboration_protocol::*;
use elastos_model_contract::{RuntimeAccessBinding, RuntimeCreateBinding};
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
    format!(
        "run:sha256:{}",
        hex::encode(Sha256::digest(format!("{principal_id}:{request_id}")))
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
        view["terminal"] = json!({ "status": status });
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
            let run_id = run_id_for(&binding.principal_id, &binding.request_id);
            self.run_owners
                .lock()
                .unwrap()
                .insert(run_id.clone(), binding.principal_id);
            return Ok(run_view(&run_id, "running"));
        }
        let binding: RuntimeAccessBinding =
            serde_json::from_value(request["runtime_binding"].clone()).map_err(provider_error)?;
        let run_id = request["run_id"].as_str().unwrap_or_default();
        binding.validate(run_id).map_err(provider_error)?;
        if self.run_owners.lock().unwrap().get(run_id) != Some(&binding.principal_id) {
            return Ok(json!({ "status": "error", "message": "run owner mismatch" }));
        }
        Ok(match op {
            "runs_get" => run_view(&binding.run_id, "running"),
            "runs_cancel" => run_view(&binding.run_id, "cancelled"),
            _ => {
                let mut pages = self.event_pages.lock().unwrap();
                let page = pages.entry(binding.run_id.clone()).or_insert(0);
                *page += 1;
                events_page(&binding.run_id, *page)
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
        self.call(
            "runs_create",
            json!({
                "op": "runs_create", "offer_id": offer_id, "operation": "text.generate",
                "input": { "prompt": "hello" }, "request_id": request_id,
                "remote_model": self.remote(SEED_PRINCIPAL, "assistant"),
                "_runtime_invocation": { "schema": "elastos.provider.invocation/v1" },
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
async fn a_full_run_index_refuses_before_any_model_work_starts() {
    let fx = TwoRuntimes::start().await;
    // 512 open records: the destination's recording capacity is exhausted.
    let mut runs = serde_json::Map::new();
    for n in 0..512 {
        runs.insert(
            format!("run:sha256:{n:064x}"),
            json!({
                "grant_id": fx.grant_id, "source_endpoint_did": fx.seed_did,
                "requester_principal_id": SEED_PRINCIPAL,
                "remote_principal_id": fx.remote_principal(), "capsule_id": "assistant",
                "offer_id": "qwen-local", "request_id": format!("open-{n}"), "created_at": now_ts(),
            }),
        );
    }
    std::fs::write(
        fx.owner.path().join("services-model-runs.json"),
        serde_json::to_vec(&json!({ "schema": "elastos.services.model-runs/v1", "runs": runs }))
            .unwrap(),
    )
    .unwrap();

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

// The seed gateway's own routing over Carrier: the path the Assistant uses.
mod consumer_path {
    use super::*;
    use crate::api::gateway::gateway_model_remote::{
        remote_offer_id, remote_run_route, route_run_operation, ConsumerModelGrant,
        RemoteRouteError,
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
            route_run_operation(
                self.seed_registry.clone(),
                self.seed.path(),
                grants,
                &seed_context(),
                "assistant",
                op,
                &normalized,
                now_ts(),
            )
            .await
        }
    }

    fn access(op: &str, run_id: &str, request_id: &str) -> Value {
        json!({ "op": op, "run_id": run_id, "after_sequence": 0,
            "runtime_binding": { "principal_id": SEED_PRINCIPAL, "request_id": request_id } })
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
}

#[tokio::test]
async fn a_denial_that_failed_to_reach_the_requester_still_cancels_open_runs() {
    let fx = TwoRuntimes::start().await;
    let open = fx.create_run("seed-req-open", "qwen-local").await;
    assert_eq!(open["ok"], true, "{open}");
    let open_run_id = run_id_for(&fx.remote_principal(), "seed-req-open");
    fx.write_request_record("denied");

    // The Inbox handler passes the denial result through the sweep unchanged,
    // so a gossip delivery error cannot skip the cancellation.
    let delivery_failed: anyhow::Result<String> =
        Err(anyhow::anyhow!("service access request delivery failed"));
    let result = settle_denied_grant(
        Some(fx.registry.clone()),
        fx.owner.path(),
        &fx.grant_id,
        delivery_failed,
    )
    .await;
    assert!(result.is_err(), "{result:?}");
    let cancel = fx.last_provider_call().await;
    assert_eq!(cancel["op"], "runs_cancel");
    assert_eq!(cancel["run_id"], open_run_id);
    assert_eq!(cancel["runtime_binding"]["grant_id"], fx.grant_id);

    // A delivered denial returns its message after the same sweep; nothing
    // is left to cancel the second time.
    let ops_before = fx.provider_ops().await;
    let delivered = settle_denied_grant(
        Some(fx.registry.clone()),
        fx.owner.path(),
        &fx.grant_id,
        Ok("Denied service request from Seed.".to_string()),
    )
    .await
    .unwrap();
    assert_eq!(delivered, "Denied service request from Seed.");
    assert_eq!(fx.provider_ops().await, ops_before);
    fx.shutdown().await;
}
