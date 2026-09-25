//! Runtime-owned remote Engine page resources. Carrier authenticates the caller;
//! the existing Engine adapter owns native effects and terminal receipts.

use super::*;
use crate::carrier::{
    browser_engine_binding::{self, RemoteEngineOwner},
    BrowserEngineGrant,
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use tokio::sync::{Mutex, Notify};

#[derive(Clone)]
struct Prepared {
    reservation: BrowserLaunchReservation,
    owner_launch_id: String,
    principal_id: String,
    request: Value,
    stream: Value,
    authority: Value,
    viewer: Value,
}

struct ServedPage {
    owner: RemoteEngineOwner,
    intent: Value,
    gateway: GatewayState,
    network: crate::collaboration_network::VerifiedCollaborationNetworkProfile,
    slots: tokio::sync::Semaphore,
    prepared: OnceLock<std::result::Result<Prepared, String>>,
    reservation: OnceLock<BrowserLaunchReservation>,
    dispatched: AtomicBool,
    expires_at: u64,
    launched: AtomicBool,
    closing: Arc<AtomicBool>,
    result: OnceLock<Value>,
    terminal: Mutex<Option<Value>>,
    changed: Notify,
    last_seen: Mutex<tokio::time::Instant>,
}

type ServedPages = BTreeMap<(PathBuf, String), Arc<ServedPage>>;
static SERVED: OnceLock<Mutex<ServedPages>> = OnceLock::new();

#[derive(Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PreparationCancellation {
    requester_endpoint: String,
    principal_id: String,
    grant_id: String,
    page_id: String,
    generation: String,
    retain_until: u64,
}

impl PreparationCancellation {
    fn from_request(source: &iroh::PublicKey, request: &Value) -> Result<Self> {
        let generation = request["lifecycle_generation"]
            .as_str()
            .context("Engine generation required")?;
        ensure!(
            generation
                .strip_prefix("sha256:")
                .is_some_and(|h| h.len() == 64
                    && h.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))),
            "Engine cancellation generation invalid"
        );
        let page_id = format!(
            "page:vz-{}",
            hex::encode(Sha256::digest(format!("{generation}\npage")))
        );
        ensure!(
            request["page_id"] == page_id,
            "Engine cancellation page differs from generation"
        );
        let id = |name: &str| -> Result<String> {
            let value = request[name]
                .as_str()
                .context("Engine cancellation owner required")?;
            ensure!(
                !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control),
                "Engine cancellation owner invalid"
            );
            Ok(value.into())
        };
        Ok(Self {
            requester_endpoint: source.to_string(),
            principal_id: id("principal_id")?,
            grant_id: id("grant_id")?,
            page_id,
            generation: generation.into(),
            // A previously admitted grant can live at most one hour; the extra
            // minute covers an invocation already inside the 55-second receiver.
            retain_until: now_ts().saturating_add(crate::carrier::ENGINE_GRANT_TTL_SECS + 60),
        })
    }
    fn same_owner(&self, other: &Self) -> bool {
        self.requester_endpoint == other.requester_endpoint
            && self.principal_id == other.principal_id
            && self.grant_id == other.grant_id
            && self.page_id == other.page_id
            && self.generation == other.generation
    }
    fn path(&self, data_dir: &FsPath) -> PathBuf {
        cancellation_dir(data_dir).join(format!(
            "{}.json",
            hex::encode(Sha256::digest(format!(
                "{}\n{}\n{}\n{}",
                self.requester_endpoint, self.principal_id, self.grant_id, self.generation
            )))
        ))
    }
    fn receipt(&self) -> Value {
        json!({"status":"ok","data":{"schema":"elastos.browser.remote-engine-preparation-closed/v1",
            "page_id":self.page_id,"generation":self.generation,"principal_id":self.principal_id,
            "grant_id":self.grant_id,"requester_endpoint":self.requester_endpoint,
            "native_dispatch":false,"cancellation_fenced":true}})
    }
}
fn cancellation_dir(data_dir: &FsPath) -> PathBuf {
    data_dir.join("Runtime/BrowserLifecycle/remote-engine-cancellations")
}
fn cancellations(data_dir: &FsPath) -> Result<Vec<PreparationCancellation>> {
    gateway_browser_sessions::read_bounded_browser_json_dir(
        data_dir,
        &cancellation_dir(data_dir),
        1024,
        4096,
    )
    .map_err(anyhow::Error::msg)
}
// Called under SERVED: cancellation and preparation have a single admission order.
fn persist_cancellation(data_dir: &FsPath, cancellation: &PreparationCancellation) -> Result<()> {
    let mut count = 0;
    for existing in cancellations(data_dir)? {
        if existing.retain_until < now_ts() {
            gateway_browser_sessions::remove_browser_durable_file(
                data_dir,
                existing.path(data_dir),
            )
            .map_err(anyhow::Error::msg)?;
        } else {
            if existing.same_owner(cancellation) {
                return Ok(());
            }
            count += 1;
        }
    }
    ensure!(
        count < 1024,
        "Engine cancellation retention capacity unavailable"
    );
    gateway_browser_sessions::write_browser_json_atomic(
        data_dir,
        &cancellation.path(data_dir),
        cancellation,
        4096,
    )
    .map_err(anyhow::Error::msg)
}

#[cfg(test)]
pub(in crate::api::gateway) struct RemoteAdmissionPause {
    pub(in crate::api::gateway) entered: Notify,
    pub(in crate::api::gateway) resume: Notify,
}
#[cfg(test)]
type AdmissionPauses = BTreeMap<(PathBuf, String, String), Arc<RemoteAdmissionPause>>;
#[cfg(test)]
static ADMISSION_PAUSES: OnceLock<Mutex<AdmissionPauses>> = OnceLock::new();
#[cfg(test)]
pub(in crate::api::gateway) async fn hold_remote_admission_for_test(
    root: &FsPath,
    page: &str,
    stage: &str,
) -> Arc<RemoteAdmissionPause> {
    let pause = Arc::new(RemoteAdmissionPause {
        entered: Notify::new(),
        resume: Notify::new(),
    });
    ADMISSION_PAUSES
        .get_or_init(Default::default)
        .lock()
        .await
        .insert((root.into(), page.into(), stage.into()), pause.clone());
    pause
}
#[cfg(test)]
async fn pause_remote_admission_for_test(root: &FsPath, page: &str, stage: &str) {
    let pause = ADMISSION_PAUSES
        .get_or_init(Default::default)
        .lock()
        .await
        .remove(&(root.into(), page.into(), stage.into()));
    if let Some(pause) = pause {
        pause.entered.notify_one();
        pause.resume.notified().await;
    }
}

fn runtime_state(
    data_dir: &FsPath,
    registry: Arc<ProviderRegistry>,
    endpoint: iroh::Endpoint,
) -> GatewayState {
    GatewayState {
        data_dir: data_dir.into(),
        cache_dir: data_dir.join("cache"),
        provider_registry: Some(registry),
        carrier_endpoint: Some(endpoint),
        identity_manager: Arc::new(OnceLock::new()),
        collaboration_chat_product_port: None,
        collaboration_presence_product_port: None,
        collaboration_discovery_service: None,
    }
}

fn prepare_intent(request: &Value) -> Result<Value> {
    let mut request = request
        .as_object()
        .context("Engine launch intent required")?
        .clone();
    request.remove("_runtime_invocation");
    ensure!(
        request.keys().all(|key| matches!(
            key.as_str(),
            "op" | "principal_id"
                | "grant_id"
                | "lifecycle_generation"
                | "page_id"
                | "vm_id"
                | "profile_key"
                | "adapter_id"
                | "url"
                | "stream_id"
                | "target"
                | "viewport"
                | "display_mode"
                | "guarantee_level"
        )),
        "Engine intent contains fields outside its Runtime grant"
    );
    request.remove("op");
    Ok(Value::Object(request))
}

async fn status_from_settlement_authority(
    registry: Arc<ProviderRegistry>,
    source: iroh::PublicKey,
    request: &Value,
) -> Result<Value> {
    let authority = request
        .get("transport_authority")
        .filter(|value| value.is_object())
        .context("Engine retained owner unavailable")?;
    let generation = request["lifecycle_generation"]
        .as_str()
        .context("Engine retained owner unavailable")?;
    let page_id = request["page_id"]
        .as_str()
        .context("Engine retained owner unavailable")?;
    let stream_id = request["stream_id"]
        .as_str()
        .context("Engine retained owner unavailable")?;
    let principal = request["principal_id"]
        .as_str()
        .context("Engine retained owner unavailable")?;
    let expected_page = format!(
        "page:vz-{}",
        hex::encode(Sha256::digest(format!("{generation}\npage")))
    );
    let storage = RemoteEngineOwner::storage_principal_for(&source.to_string(), principal);
    ensure!(
        page_id == expected_page
            && authority.get("generation").and_then(Value::as_str) == Some(generation)
            && authority.get("page_id").and_then(Value::as_str) == Some(page_id)
            && authority
                .pointer("/egress/stream_id")
                .and_then(Value::as_str)
                == Some(stream_id)
            && authority.get("principal_id").and_then(Value::as_str) == Some(storage.as_str()),
        "Engine requester or generation changed"
    );
    let mut local = request.clone();
    let object = local.as_object_mut().context("Engine request required")?;
    for key in ["grant_id", "page_id", "_runtime_invocation"] {
        object.remove(key);
    }
    local["op"] = json!("status");
    local["principal_id"] = json!(storage);
    local["lifecycle_generation"] = json!(generation);
    local["stream_id"] = json!(stream_id);
    Ok(registry.send_raw("browser-engine", &local).await?)
}

pub(crate) async fn invoke(
    data_dir: &FsPath,
    registry: Arc<ProviderRegistry>,
    endpoint: iroh::Endpoint,
    source: iroh::PublicKey,
    network: crate::collaboration_network::VerifiedCollaborationNetworkProfile,
    grant: Option<BrowserEngineGrant>,
    request: &Value,
) -> Result<Value> {
    let operation = request["op"]
        .as_str()
        .context("Engine operation required")?;
    let page_id = request["page_id"]
        .as_str()
        .context("Engine page binding required")?;
    let key = (data_dir.to_owned(), page_id.to_owned());
    let pages = SERVED.get_or_init(Default::default);
    if operation == "close_page" && request["cancel_preparation"] == true {
        let cancellation = PreparationCancellation::from_request(&source, request)?;
        let pages = pages.lock().await;
        let page = pages.get(&key).cloned();
        if let Some(page) = &page {
            ensure!(
                page.owner.matches_request(&source, request)
                    && !page.launched.load(Ordering::Acquire),
                "Engine cancellation requires its exact undispatched owner"
            );
        }
        #[cfg(test)]
        pause_remote_admission_for_test(data_dir, page_id, "cancel_checked").await;
        persist_cancellation(data_dir, &cancellation)?;
        if let Some(page) = &page {
            page.closing.store(true, Ordering::Release);
        }
        drop(pages);
        if let Some(page) = page {
            let closed = close(&page).await?;
            ensure!(
                closed["status"] == "ok"
                    && closed["data"]["schema"]
                        == "elastos.browser.remote-engine-preparation-closed/v1"
                    && closed["data"]["page_id"] == cancellation.page_id
                    && closed["data"]["generation"] == cancellation.generation
                    && closed["data"]["native_dispatch"] == false,
                "Engine cancellation requires its actual native settlement"
            );
        } else {
            ensure!(
                gateway_browser_sessions::browser_remote_generation_absent(
                    data_dir,
                    &cancellation.generation
                )
                .await
                .map_err(anyhow::Error::msg)?,
                "Engine durable owner still requires exact settlement"
            );
        }
        return Ok(cancellation.receipt());
    }
    let page = if operation == "prepare_launch" {
        let grant = grant.as_ref().context("Engine launch authority required")?;
        let owner = RemoteEngineOwner::from_launch(grant, request)?;
        let intent = prepare_intent(request)?;
        let mut pages = pages.lock().await;
        let cancellation = PreparationCancellation::from_request(&source, request)?;
        ensure!(
            !cancellations(data_dir)?
                .iter()
                .any(|c| c.retain_until >= now_ts() && c.same_owner(&cancellation)),
            "Engine preparation was terminally canceled"
        );
        if let Some(page) = pages.get(&key) {
            ensure!(
                page.owner == owner && page.intent == intent,
                "Engine launch replay changed its binding"
            );
            page.clone()
        } else {
            pages.retain(|_, page| {
                !(page.expires_at <= now_ts()
                    && (page
                        .terminal
                        .try_lock()
                        .is_ok_and(|receipt| receipt.is_some())
                        || page.prepared.get().is_some_and(Result::is_err)))
            });
            ensure!(
                pages
                    .iter()
                    .filter(|((root, _), page)| root == data_dir
                        && page
                            .terminal
                            .try_lock()
                            .is_ok_and(|receipt| receipt.is_none())
                        && !page.prepared.get().is_some_and(Result::is_err))
                    .count()
                    < 16
                    && pages.keys().filter(|(root, _)| root == data_dir).count() < 1024,
                "Engine retained page capacity unavailable"
            );
            let page = Arc::new(ServedPage {
                owner,
                intent,
                gateway: runtime_state(data_dir, registry, endpoint),
                network,
                slots: tokio::sync::Semaphore::new(32),
                prepared: OnceLock::new(),
                reservation: OnceLock::new(),
                dispatched: AtomicBool::new(false),
                expires_at: grant.expires_at,
                launched: AtomicBool::new(false),
                closing: Arc::new(AtomicBool::new(false)),
                result: OnceLock::new(),
                terminal: Mutex::new(None),
                changed: Notify::new(),
                last_seen: Mutex::new(tokio::time::Instant::now()),
            });
            pages.insert(key, page.clone());
            let task = page.clone();
            tokio::spawn(async move {
                let result = prepare(&task).await.map_err(|e| e.to_string());
                task.prepared.set(result).ok();
                task.changed.notify_waiters();
                supervise(task).await;
            });
            page
        }
    } else if let Some(page) = pages.lock().await.get(&key).cloned() {
        page
    } else if operation == "status" {
        return status_from_settlement_authority(registry, source, request).await;
    } else {
        return Err(anyhow::anyhow!("Engine retained owner unavailable"));
    };
    ensure!(
        page.owner.matches_request(&source, request),
        "Engine requester or generation changed"
    );
    if !matches!(operation, "close_page" | "status") {
        let grant = grant
            .as_ref()
            .context("Engine current authority required")?;
        ensure!(
            grant.revision == page.owner.grant_revision && !page.closing.load(Ordering::Acquire),
            "Engine authority changed or page is retiring"
        );
        *page.last_seen.lock().await = tokio::time::Instant::now();
    } else if operation == "close_page" {
        if page.launched.load(Ordering::Acquire) {
            let known = page
                .result
                .get()
                .and_then(provider_response_data)
                .and_then(|page| page.get("runtime_cleanup").cloned());
            ensure!(
                known.as_ref() == request.get("runtime_cleanup"),
                "Engine close requires its retained exact cleanup binding"
            );
        }
        page.closing.store(true, Ordering::Release);
    }
    let _slot = if operation == "close_page" {
        None
    } else {
        Some(
            page.slots
                .try_acquire()
                .context("Engine page operation capacity unavailable")?,
        )
    };
    if operation == "close_page" {
        return close(&page).await;
    }
    let prepared = page.wait_prepared().await?;
    match operation {
        "prepare_launch" => {
            ensure!(
                !page.closing.load(Ordering::Acquire),
                "Engine preparation was canceled before publication"
            );
            Ok(json!({"status":"ok","data":{
                "schema":"elastos.browser.remote-engine-preparation/v1",
                "page_id":page.owner.page_id,"generation":page.owner.generation,
                "adapter_id":page.owner.adapter_id,"transport_authority":prepared.authority,
                "viewer_turn_capability":prepared.viewer,
            }}))
        }
        "launch" => {
            ensure!(
                request
                    .as_object()
                    .is_some_and(|r| r.keys().all(|key| matches!(
                        key.as_str(),
                        "op" | "principal_id"
                            | "grant_id"
                            | "page_id"
                            | "lifecycle_generation"
                            | "_runtime_invocation"
                    ))),
                "Engine launch replay cannot change its prepared intent"
            );
            #[cfg(test)]
            pause_remote_admission_for_test(data_dir, page_id, "launch_captured").await;
            // Use the same admission lock as cancellation, including its durable
            // fence write. A captured launch cannot cross that decision later.
            let admission = pages.lock().await;
            ensure!(
                !page.closing.load(Ordering::Acquire),
                "Engine launch was canceled"
            );
            if !page.launched.swap(true, Ordering::AcqRel) {
                let task = page.clone();
                tokio::spawn(async move {
                    let result = launch(&task).await.unwrap_or_else(|error| {
                        task.closing.store(true, Ordering::Release);
                        tracing::warn!(%error, "Runtime remote Engine launch requires reconciliation");
                        json!({"status":"error","code":"browser_engine_unavailable",
                            "message":"Remote Engine launch requires reconciliation"})
                    });
                    if result["status"] != "ok" {
                        task.closing.store(true, Ordering::Release);
                    }
                    task.result.set(result).ok();
                    task.changed.notify_waiters();
                    if task.closing.load(Ordering::Acquire) {
                        let _ = close(&task).await;
                    }
                });
            }
            drop(admission);
            let result = page.wait_result().await?;
            ensure!(
                !page.closing.load(Ordering::Acquire),
                "Engine owner retired before launch publication"
            );
            Ok(result)
        }
        "close_page" => close(&page).await,
        _ => {
            ensure!(
                page.launched.load(Ordering::Acquire) || operation == "status",
                "Engine page was not launched"
            );
            if operation != "status" {
                let _ = page.wait_result().await?;
            }
            ensure!(
                operation == "status" || !page.closing.load(Ordering::Acquire),
                "Engine page is retiring"
            );
            let mut local = request.clone();
            let object = local.as_object_mut().context("Engine request required")?;
            for key in ["grant_id", "lifecycle_generation", "_runtime_invocation"] {
                object.remove(key);
            }
            local["principal_id"] = json!(prepared.principal_id);
            if operation == "status" {
                local.as_object_mut().unwrap().remove("page_id");
                local["lifecycle_generation"] = json!(page.owner.generation);
                local["stream_id"] = prepared.stream["stream_id"].clone();
                local["adapter_id"] = json!(page.owner.adapter_id);
                local["transport_authority"] = prepared.authority.clone();
            }
            if operation == "attach_stream" {
                ensure!(
                    request["stream_session"]["stream_id"] == prepared.stream["stream_id"]
                        && request["stream_session"]["target"] == prepared.stream["target"],
                    "Engine transport is fixed for this launch generation"
                );
                local["stream_session"] = browser_engine_stream_session(&prepared.stream);
            }
            let response = page
                .gateway
                .provider_registry
                .as_ref()
                .unwrap()
                .send_raw("browser-engine", &local)
                .await?;
            ensure!(
                operation == "status" || !page.closing.load(Ordering::Acquire),
                "Engine owner retired during operation"
            );
            touch_browser_page(
                &page.gateway.data_dir,
                &page.owner.page_id,
                &prepared.principal_id,
                &prepared.owner_launch_id,
            )
            .await;
            Ok(response)
        }
    }
}

impl ServedPage {
    async fn wait_prepared(&self) -> Result<Prepared> {
        loop {
            let changed = self.changed.notified();
            if let Some(result) = self.prepared.get() {
                return result.clone().map_err(anyhow::Error::msg);
            }
            changed.await;
        }
    }
    async fn wait_result(&self) -> Result<Value> {
        loop {
            let changed = self.changed.notified();
            if let Some(result) = self.result.get() {
                return Ok(result.clone());
            }
            changed.await;
        }
    }
}

const REMOTE_WALLET_CONSUMER_MEDIATION_SCHEMA: &str =
    "elastos.browser.wallet-consumer-mediation/v1";
const REMOTE_WALLET_CONSUMER_REQUEST_SCHEMA: &str = "elastos.browser.wallet-consumer-request/v1";

fn remote_wallet_consumer_mediation(
    requester_endpoint: &str,
    page_id: &str,
    generation: &str,
) -> Value {
    json!({
        "schema": REMOTE_WALLET_CONSUMER_MEDIATION_SCHEMA,
        "resolution": "consumer_runtime",
        "requester_endpoint": requester_endpoint,
        "page_id": page_id,
        "lifecycle_generation": generation,
        "bus": "private",
        "authority": "consumer_runtime",
    })
}

fn remote_wallet_consumer_request(
    mediation: &Value,
    operation: &str,
    page_url: &str,
    origin: &str,
    payload: Value,
) -> Result<Value> {
    ensure!(
        mediation["schema"] == REMOTE_WALLET_CONSUMER_MEDIATION_SCHEMA
            && mediation["resolution"] == "consumer_runtime"
            && mediation["authority"] == "consumer_runtime"
            && mediation["bus"] == "private",
        "Remote Wallet request requires consumer mediation"
    );
    ensure!(
        mediation.get("home_token").is_none() && mediation.get("account_access_url").is_none(),
        "Remote Wallet request keeps tokens on the consumer"
    );
    ensure!(
        matches!(
            operation,
            "request_accounts"
                | "request_signature"
                | "request_transaction"
                | "approval_status"
                | "read"
                | "broadcast_transaction"
        ),
        "Remote Wallet operation is outside the consumer mediation set"
    );
    let requester_endpoint = mediation["requester_endpoint"]
        .as_str()
        .context("Remote Wallet consumer peer required")?;
    let page_id = mediation["page_id"]
        .as_str()
        .context("Remote Wallet page required")?;
    let generation = mediation["lifecycle_generation"]
        .as_str()
        .context("Remote Wallet generation required")?;
    Ok(json!({
        "schema": REMOTE_WALLET_CONSUMER_REQUEST_SCHEMA,
        "resolution": "consumer_runtime",
        "requester_endpoint": requester_endpoint,
        "page_id": page_id,
        "lifecycle_generation": generation,
        "operation": operation,
        "page_url": page_url,
        "origin": origin,
        "payload": payload,
        "bus": "private",
        "authority": "consumer_runtime",
    }))
}

fn authenticated_engine_peer(binding: &ConsumerBinding) -> Result<iroh::PublicKey> {
    binding.grant["peer_did"]
        .as_str()
        .context("Remote Wallet Engine peer required")?
        .parse()
        .context("Remote Wallet Engine peer invalid")
}

#[allow(dead_code)]
pub(in crate::api::gateway) fn admit_remote_wallet_consumer_request(
    binding: &ConsumerBinding,
    request: &Value,
    source: &iroh::PublicKey,
) -> Result<()> {
    ensure!(
        request["schema"] == REMOTE_WALLET_CONSUMER_REQUEST_SCHEMA
            && request["resolution"] == "consumer_runtime"
            && request["page_id"] == binding.page_id
            && request["lifecycle_generation"] == binding.generation
            && request.get("home_token").is_none()
            && request.get("account_access_url").is_none(),
        "Remote Wallet request is not bound to this consumer page"
    );
    ensure!(
        *source == authenticated_engine_peer(binding)?,
        "Remote Wallet request is not bound to the authenticated Engine peer"
    );
    Ok(())
}

#[allow(dead_code)]
pub(in crate::api::gateway) fn forward_remote_wallet_consumer_request(
    binding: &ConsumerBinding,
    launch_wallet: &Value,
    source: &iroh::PublicKey,
    operation: &str,
    page_url: &str,
    origin: &str,
    payload: Value,
) -> Result<Value> {
    let request =
        remote_wallet_consumer_request(launch_wallet, operation, page_url, origin, payload)?;
    admit_remote_wallet_consumer_request(binding, &request, source)?;
    Ok(request)
}

async fn prepare(page: &ServedPage) -> Result<Prepared> {
    let gateway = &page.gateway;
    let registry = gateway
        .provider_registry
        .as_ref()
        .context("Engine registry required")?;
    let owner = &page.owner;
    let intent = &page.intent;
    let (url, target) =
        browser_url_to_stream_target(intent["url"].as_str().context("Engine URL required")?)?;
    ensure!(
        intent["target"] == target,
        "Engine egress target differs from initial URL"
    );
    let mode: BrowserDisplayMode = serde_json::from_value(intent["display_mode"].clone())?;
    let guarantee: BrowserGuaranteeLevel =
        serde_json::from_value(intent["guarantee_level"].clone())?;
    validate_browser_launch_contract(mode, guarantee)?;
    let principal = owner.storage_principal();
    let adapter = resolve_browser_engine_adapter(
        registry,
        &gateway.data_dir,
        &principal,
        Some(&owner.adapter_id),
        mode,
        guarantee,
    )
    .await?;
    let registration = registry
        .registration_for_uri("elastos://browser-engine/launch")
        .await
        .context("Engine route missing")?;
    let owner_launch_id = format!(
        "remote-engine:{}",
        hex::encode(Sha256::digest(format!(
            "{}\n{}",
            owner.requester_endpoint, owner.generation
        )))
    );
    let reservation = reserve_remote_browser_launch(
        &gateway.data_dir,
        &principal,
        BrowserLaunchLifecycle {
            owner_launch_id: owner_launch_id.clone(),
            browser_instance: None,
            url: url.clone(),
            exit_id: "consumer-runtime".into(),
            engine_route_provider: registration.provider,
            selected_engine_adapter: Some(adapter),
            service_selection: None,
            profile_key_hash: browser_lifecycle_hash(&owner.profile_key),
            vm_key_hash: browser_lifecycle_hash(&owner.generation),
        },
        &owner.generation,
    )
    .await
    .map_err(|(_, error)| anyhow::anyhow!(error))?;
    page.reservation.set(reservation.clone()).ok();
    let result = async {
        ensure!(!page.closing.load(Ordering::Acquire), "Engine preparation was canceled");
        let stream_id = intent["stream_id"].as_str().filter(|s| s.len() <= 256 && is_safe_runtime_id(s)).context("Engine stream identity invalid")?;
        let stream = attach_remote_engine_stream(&gateway.data_dir, owner, gateway.carrier_endpoint.as_ref().unwrap(),
            json!({"stream_id":stream_id,"target":target})).await?;
        let transport = prepare_browser_vz_transport_launch(&gateway.data_dir, BrowserVzTransportLaunchBinding {
            generation:&owner.generation, page_id:&owner.page_id, vm_id:&owner.vm_id, principal_id:&principal,
            egress_stream_id:stream_id, egress_target:&target,
            egress_runtime_socket_path:stream["adapter_ipc"]["runtime_stream_path"].as_str().unwrap_or_default(),
        }).map_err(anyhow::Error::msg)?.context("Remote Engine transport preparation unsupported")?;
        bind_browser_vz_transport_authority(&gateway.data_dir, &reservation, stream_id, None, transport.authority.clone()).await.map_err(anyhow::Error::msg)?;
        let (_, mut profile) = browser_profile_launch_descriptor(&gateway.data_dir, &principal)?;
        profile["profile_key"] = json!(owner.profile_key);
        let viewer = browser_vz_viewer_turn_capability(&transport.authority, &transport.secret).map_err(anyhow::Error::msg)?;
        crate::carrier::browser_engine_media::bind_target(&gateway.data_dir, owner.requester_endpoint.parse()?, &transport.authority, page.closing.clone()).await?;
        let wallet = remote_wallet_consumer_mediation(&owner.requester_endpoint, &owner.page_id, &owner.generation);
        let origin = url::Url::parse(&url)
            .ok()
            .map(|parsed| parsed.origin().ascii_serialization())
            .filter(|origin| origin.starts_with("http"))
            .context("Engine Wallet page origin required")?;
        ensure!(
            remote_wallet_consumer_request(
                &wallet,
                "request_accounts",
                &url,
                &origin,
                json!({"method": "eth_requestAccounts"}),
            )?["page_id"]
                == owner.page_id,
            "Engine Wallet launch request escaped its page"
        );
        let request = json!({"op":"launch","url":url,"stream_session":browser_engine_stream_session(&stream),
            "lifecycle_generation":owner.generation,"principal_id":principal,"profile":profile,
            "wallet":wallet,
            "viewport":intent["viewport"],"display_mode":mode,"guarantee_level":guarantee,
            "adapter_id":owner.adapter_id,"page_id":owner.page_id,"vm_id":owner.vm_id,
            "transport_authority":transport.authority,"transport_secret":transport.secret});
        Ok::<_, anyhow::Error>(Prepared {reservation:reservation.clone(),owner_launch_id,
            principal_id:principal,request,stream,authority:transport.authority,viewer})
    }.await;
    if result.is_err() {
        let _ = close_undispatched(page).await;
    }
    result
}

async fn launch(page: &ServedPage) -> Result<Value> {
    let prepared = page.wait_prepared().await?;
    ensure!(
        !page.closing.load(Ordering::Acquire),
        "Engine launch was canceled"
    );
    spawn_browser_vz_fixed_media_listener(&prepared.authority).await?;
    mark_browser_vz_transport_dispatched(
        &page.gateway.data_dir,
        &prepared.reservation,
        prepared.stream["stream_id"].as_str().unwrap(),
    )
    .await
    .map_err(anyhow::Error::msg)?;
    page.dispatched.store(true, Ordering::Release);
    let response = page
        .gateway
        .provider_registry
        .as_ref()
        .unwrap()
        .send_raw("browser-engine", &prepared.request)
        .await?;
    if response["status"] != "ok" {
        let _ = reconcile_dispatched_browser_launch_failure(
            &page.gateway,
            &prepared.reservation,
            &prepared.principal_id,
            &prepared.owner_launch_id,
            prepared.stream["stream_id"].as_str().unwrap(),
            None,
            "Remote Engine launch failed",
        )
        .await;
        return Ok(response);
    }
    let raw = provider_response_data(&response).context("Engine launch response missing")?;
    let mode = serde_json::from_value(prepared.request["display_mode"].clone())?;
    let guarantee = serde_json::from_value(prepared.request["guarantee_level"].clone())?;
    let mut visible = validate_browser_engine_page(raw.clone(), mode, guarantee)?;
    visible["transport_proof"] =
        browser_vz_public_transport_proof(&prepared.authority, &raw["transport_receipt"])
            .map_err(anyhow::Error::msg)?;
    for field in [
        "runtime_cleanup",
        "transport_authority",
        "transport_receipt",
    ] {
        visible.as_object_mut().unwrap().remove(field);
    }
    let cleanup = browser_provider_cleanup_binding(
        &raw,
        &page.owner.generation,
        &page.owner.page_id,
        &page.owner.adapter_id,
        raw["engine"].as_str().context("Engine identity missing")?,
        prepared.stream["stream_id"].as_str(),
        Some(&prepared.authority),
    )
    .map_err(anyhow::Error::msg)?;
    complete_browser_launch(
        &page.gateway.data_dir,
        &prepared.reservation,
        BrowserLaunchEffect {
            page_id: page.owner.page_id.clone(),
            engine_provider: raw["provider"]
                .as_str()
                .context("Engine provider missing")?
                .into(),
            engine_protocol_version: raw["protocol_version"]
                .as_str()
                .context("Engine protocol missing")?
                .into(),
            engine_adapter: page.owner.adapter_id.clone(),
            engine: raw["engine"].as_str().unwrap().into(),
            provider_cleanup: cleanup,
            browser_page: visible,
            viewer_turn_capability: Some(prepared.viewer),
            stream_cleanup: None,
        },
    )
    .await
    .map_err(anyhow::Error::msg)?;
    Ok(response)
}

async fn close_undispatched(page: &ServedPage) -> Result<()> {
    ensure!(
        !page.dispatched.load(Ordering::Acquire),
        "Engine native settlement is required"
    );
    if let Some(stream_id) = page.intent["stream_id"].as_str() {
        close_browser_runtime_stream_listener(&page.gateway.data_dir, stream_id)
            .await
            .map_err(anyhow::Error::msg)?;
    }
    crate::carrier::browser_engine_media::remove_target(
        &page.gateway.data_dir,
        &page.owner.page_id,
        &page.owner.generation,
    )
    .await?;
    if let Some(reservation) = page.reservation.get() {
        if let Some(authority) = browser_launch_transport_authority(reservation).await {
            close_browser_vz_fixed_media_listener(&authority)
                .await
                .map_err(anyhow::Error::msg)?;
        }
        discard_browser_vz_transport_preparation(&page.gateway.data_dir, reservation)
            .await
            .map_err(anyhow::Error::msg)?;
        release_browser_launch(reservation).await;
    }
    Ok(())
}

async fn close(page: &ServedPage) -> Result<Value> {
    let mut terminal = page.terminal.lock().await;
    if let Some(receipt) = &*terminal {
        return Ok(receipt.clone());
    }
    {
        let _admission = SERVED.get_or_init(Default::default).lock().await;
        page.closing.store(true, Ordering::Release);
    }
    let prepared_result = page.wait_prepared().await;
    if page.launched.load(Ordering::Acquire) {
        let _ = page.wait_result().await?;
    }
    if !page.dispatched.load(Ordering::Acquire) {
        close_undispatched(page).await?;
        let response = json!({"status":"ok","data":{"schema":"elastos.browser.remote-engine-preparation-closed/v1",
            "page_id":page.owner.page_id,"generation":page.owner.generation,"native_dispatch":false}});
        *terminal = Some(response.clone());
        return Ok(response);
    }
    let prepared = prepared_result?;
    let mut cleanup = browser_page_cleanup_for_principal(
        &page.gateway.data_dir,
        &page.owner.page_id,
        &prepared.principal_id,
        &prepared.owner_launch_id,
        prepared.reservation.cleanup_id(),
    )
    .await
    .map_err(anyhow::Error::msg)?;
    if cleanup.is_none() {
        let reconciliation = attempt_browser_launch_reconciliation_bounded(
            &page.gateway,
            BrowserLaunchReconciliationAttempt {
                generation: &page.owner.generation,
                engine_route_provider: prepared.reservation.engine_route_provider(),
                selected_engine_adapter: Some(&page.owner.adapter_id),
                principal_id: &prepared.principal_id,
                stream_id: prepared.stream["stream_id"].as_str().unwrap(),
                stream_cleanup: None,
                transport_authority: Some(&prepared.authority),
            },
        )
        .await
        .map_err(anyhow::Error::msg)?;
        match reconciliation {
            BrowserDispatchedLaunchReconciliation::EffectAcquired(effect) => {
                complete_browser_launch(&page.gateway.data_dir, &prepared.reservation, *effect)
                    .await
                    .map_err(anyhow::Error::msg)?;
                cleanup = browser_page_cleanup_for_principal(
                    &page.gateway.data_dir,
                    &page.owner.page_id,
                    &prepared.principal_id,
                    &prepared.owner_launch_id,
                    prepared.reservation.cleanup_id(),
                )
                .await
                .map_err(anyhow::Error::msg)?;
            }
            BrowserDispatchedLaunchReconciliation::DidNotAct
            | BrowserDispatchedLaunchReconciliation::TerminalPostEffectCleanup { .. } => {
                close_browser_runtime_stream_listener(
                    &page.gateway.data_dir,
                    prepared.stream["stream_id"].as_str().unwrap(),
                )
                .await
                .map_err(anyhow::Error::msg)?;
                close_browser_vz_fixed_media_listener(&prepared.authority)
                    .await
                    .map_err(anyhow::Error::msg)?;
                crate::carrier::browser_engine_media::remove_target(
                    &page.gateway.data_dir,
                    &page.owner.page_id,
                    &page.owner.generation,
                )
                .await?;
                discard_browser_vz_transport_preparation(
                    &page.gateway.data_dir,
                    &prepared.reservation,
                )
                .await
                .map_err(anyhow::Error::msg)?;
                release_browser_launch(&prepared.reservation).await;
                // This internal outcome does not replace the native settlement
                // receipt. The consumer obtains that exact receipt through status.
                let response = json!({"status":"ok","data":{"schema":"elastos.browser.remote-engine-launch-settled/v1",
                    "page_id":page.owner.page_id,"generation":page.owner.generation}});
                *terminal = Some(response.clone());
                return Ok(response);
            }
            BrowserDispatchedLaunchReconciliation::CleanupPending => {
                anyhow::bail!("Engine terminal settlement is pending")
            }
        }
    }
    let cleanup = cleanup.context("Engine terminal settlement is pending")?;
    mark_browser_page_retiring(
        &page.gateway.data_dir,
        &page.owner.page_id,
        &prepared.principal_id,
        &prepared.owner_launch_id,
    )
    .await;
    record_browser_engine_cleanup_obligation(
        &page.gateway.data_dir,
        cleanup.engine_cleanup.clone(),
        None,
    )
    .await
    .map_err(anyhow::Error::msg)?;
    let receipt = attempt_browser_engine_cleanup(&page.gateway, &cleanup.engine_cleanup)
        .await
        .map_err(anyhow::Error::msg)?;
    close_browser_runtime_stream_listener(
        &page.gateway.data_dir,
        prepared.stream["stream_id"].as_str().unwrap(),
    )
    .await
    .map_err(anyhow::Error::msg)?;
    crate::carrier::browser_engine_media::remove_target(
        &page.gateway.data_dir,
        &page.owner.page_id,
        &page.owner.generation,
    )
    .await?;
    commit_browser_terminal_cleanup(&page.gateway, &cleanup.engine_cleanup, None, None)
        .await
        .map_err(anyhow::Error::msg)?;
    release_browser_page_for_principal(
        &page.gateway.data_dir,
        &page.owner.page_id,
        &prepared.principal_id,
        &prepared.owner_launch_id,
    )
    .await;
    let response = json!({"status":"ok","data":receipt});
    *terminal = Some(response.clone());
    Ok(response)
}

async fn supervise(page: Arc<ServedPage>) {
    if page.prepared.get().is_some_and(Result::is_err) {
        return;
    }
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        if page.terminal.lock().await.is_some() {
            return;
        }
        let stale = page.last_seen.lock().await.elapsed() >= Duration::from_secs(50);
        let retirement_reason = if stale {
            Some("owner_heartbeat_expired")
        } else {
            let root = page.gateway.data_dir.clone();
            let network = page.network.clone();
            let owner = page.owner.clone();
            let observation = tokio::time::timeout(Duration::from_secs(1), tokio::task::spawn_blocking(move || {
                let source = owner.requester_endpoint.parse::<iroh::PublicKey>().ok()?;
                let current = crate::api::gateway::authorize_home_service_engine(&root, &network, &source,
                    &json!({"op":"page_status","principal_id":owner.requester_principal_id,"grant_id":owner.grant_id}), now_ts()).ok()?;
                (current.revision == owner.grant_revision && current.execution_allowed).then_some(())
            })).await;
            match observation {
                Ok(Ok(Some(()))) => None,
                Ok(Ok(None)) => Some("authority_rejected_or_changed"),
                Ok(Err(_)) => Some("authority_worker_failed"),
                Err(_) => Some("authority_observation_deadline"),
            }
        };
        if let Some(reason) = retirement_reason {
            if !page.closing.load(Ordering::Acquire) {
                tracing::warn!(reason, "Remote Engine owner retired");
            }
            page.closing.store(true, Ordering::Release);
        }
        if page.closing.load(Ordering::Acquire) {
            // A timed-out close remains an obligation. The existing durable
            // lifecycle reconciler also owns the exact native cleanup binding.
            let _ = tokio::time::timeout(Duration::from_secs(5), close(&page)).await;
        }
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::api::gateway) struct ConsumerBinding {
    pub(super) grant: Value,
    pub(super) selection_id: String,
    pub(super) adapter_id: String,
    pub(super) principal_id: String,
    pub(super) page_id: String,
    pub(super) generation: String,
    pub(super) stream_id: String,
    pub(super) prepared: Option<Value>,
    #[serde(default)]
    dispatch_started: bool,
    #[serde(default)]
    preparation_closed: bool,
    #[serde(default)]
    owner_launch_id: Option<String>,
    #[serde(default)]
    browser_instance: Option<String>,
    #[serde(default)]
    reservation: Option<BrowserLaunchReservation>,
    #[serde(default)]
    stream_cleanup: Option<BrowserStreamCleanup>,
    #[serde(default)]
    cleanup_pending: bool,
    #[serde(default)]
    retry_after: u64,
    #[serde(default)]
    terminal_retirement: bool,
}

pub(super) fn consumer_reservation_absent(
    data_dir: &FsPath,
    reservation: &BrowserLaunchReservation,
) -> Result<bool> {
    Ok(!consumer_records(data_dir)?
        .iter()
        .any(|binding| binding.page_id == reservation.page_id()))
}

pub(super) fn retire_consumer_reservation(
    data_dir: &FsPath,
    reservation: &BrowserLaunchReservation,
) -> Result<()> {
    if let Some(binding) = consumer_records(data_dir)?
        .into_iter()
        .find(|b| b.page_id == reservation.page_id())
    {
        retire_consumer(
            data_dir,
            &binding.principal_id,
            &binding.page_id,
            reservation.generation(),
        )?;
    }
    Ok(())
}

pub(super) fn retire_consumer_generation(
    data_dir: &FsPath,
    principal: &str,
    generation: &str,
) -> Result<()> {
    if let Some(binding) = consumer_binding(
        data_dir,
        &json!({"principal_id":principal,"lifecycle_generation":generation}),
    )? {
        retire_consumer(data_dir, principal, &binding.page_id, generation)?;
    }
    Ok(())
}

pub(super) fn selection_id(grant: &Value, adapter_id: &str) -> String {
    format!(
        "remote-engine-{}",
        hex::encode(&Sha256::digest(format!("{}\n{}", grant["id"], adapter_id))[..16])
    )
}

fn consumer_dir(data_dir: &FsPath) -> PathBuf {
    data_dir.join("Runtime/BrowserLifecycle/remote-engine")
}
fn consumer_path(data_dir: &FsPath, page_id: &str) -> PathBuf {
    consumer_dir(data_dir).join(format!("{}.json", hex::encode(Sha256::digest(page_id))))
}

const CONSUMER_LIMIT: usize = 128;
// Serializes short durable record mutations only; no network work holds this lock.
static CONSUMER_WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn consumer_records(data_dir: &FsPath) -> Result<Vec<ConsumerBinding>> {
    gateway_browser_sessions::read_bounded_browser_json_dir(
        data_dir,
        &consumer_dir(data_dir),
        CONSUMER_LIMIT,
        64 * 1024,
    )
    .map_err(anyhow::Error::msg)
}

fn consumer_capacity(data_dir: &FsPath) -> Result<()> {
    ensure!(
        consumer_records(data_dir)?.len() < CONSUMER_LIMIT,
        "Remote Engine pending ownership capacity unavailable"
    );
    Ok(())
}

fn save_consumer(data_dir: &FsPath, binding: &ConsumerBinding) -> Result<()> {
    write_consumer(data_dir, binding, false, false)
}
fn insert_consumer(data_dir: &FsPath, binding: &ConsumerBinding) -> Result<()> {
    write_consumer(data_dir, binding, true, false)
}
fn write_consumer(
    data_dir: &FsPath,
    binding: &ConsumerBinding,
    inserting: bool,
    admitting_launch: bool,
) -> Result<()> {
    let _guard = CONSUMER_WRITES.lock().unwrap();
    let records = consumer_records(data_dir)?;
    let existing = records.iter().find(|b| b.page_id == binding.page_id);
    ensure!(
        inserting == existing.is_none(),
        "Remote Engine preparation record changed"
    );
    ensure!(
        existing.is_some() || records.len() < CONSUMER_LIMIT,
        "Remote Engine pending ownership capacity unavailable"
    );
    if let Some(existing) = existing {
        ensure!(
            existing.generation == binding.generation
                && existing.principal_id == binding.principal_id,
            "Remote Engine durable owner changed"
        );
        if admitting_launch {
            // Admission checks the persisted owner under the same lock as
            // cancellation, including a fresh read or an already-dispatched replay.
            ensure!(
                !existing.cleanup_pending
                    && !existing.preparation_closed
                    && !existing.terminal_retirement,
                "Remote Engine preparation is retiring"
            );
        }
        ensure!(
            (!existing.cleanup_pending || binding.cleanup_pending)
                && (!existing.preparation_closed || binding.preparation_closed)
                && (!existing.dispatch_started || binding.dispatch_started)
                && (!existing.terminal_retirement || binding.terminal_retirement),
            "Remote Engine preparation is retiring"
        );
    }
    gateway_browser_sessions::write_browser_json_atomic(
        data_dir,
        &consumer_path(data_dir, &binding.page_id),
        binding,
        64 * 1024,
    )
    .map_err(anyhow::Error::msg)
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfilePlacement {
    principal_id: String,
    peer: String,
}

fn placement_dir(data_dir: &FsPath) -> PathBuf {
    data_dir.join("Runtime/BrowserLifecycle/remote-engine-profiles")
}

fn retain_profile_placement(data_dir: &FsPath, binding: &ConsumerBinding) -> Result<()> {
    let _guard = CONSUMER_WRITES.lock().unwrap();
    let records = gateway_browser_sessions::read_bounded_browser_json_dir::<ProfilePlacement>(
        data_dir,
        &placement_dir(data_dir),
        1024,
        4096,
    )
    .map_err(anyhow::Error::msg)?;
    let peer = binding.grant["peer_did"]
        .as_str()
        .context("Engine profile owner missing")?;
    if let Some(existing) = records
        .iter()
        .find(|p| p.principal_id == binding.principal_id)
    {
        ensure!(existing.peer == peer, "Browser profile placement changed");
        return Ok(());
    }
    ensure!(
        records.len() < 1024,
        "Browser profile placement capacity unavailable"
    );
    gateway_browser_sessions::write_browser_json_atomic(
        data_dir,
        &placement_dir(data_dir).join(format!(
            "{}.json",
            hex::encode(Sha256::digest(&binding.principal_id))
        )),
        &ProfilePlacement {
            principal_id: binding.principal_id.clone(),
            peer: peer.into(),
        },
        4096,
    )
    .map_err(anyhow::Error::msg)
}

pub(super) fn mark_consumer_terminal_retirement(
    data_dir: &FsPath,
    principal: &str,
    generation: &str,
) -> Result<()> {
    if let Some(mut binding) = consumer_binding(
        data_dir,
        &json!({"principal_id":principal,"lifecycle_generation":generation}),
    )? {
        if !binding.terminal_retirement {
            binding.terminal_retirement = true;
            save_consumer(data_dir, &binding)?;
        }
    }
    Ok(())
}

pub(super) fn consumer_terminal_retirement(
    data_dir: &FsPath,
    principal: &str,
    generation: &str,
) -> Result<bool> {
    Ok(consumer_binding(
        data_dir,
        &json!({"principal_id":principal,"lifecycle_generation":generation}),
    )?
    .is_some_and(|binding| binding.terminal_retirement))
}

pub(super) fn consumer_grant_expired(
    data_dir: &FsPath,
    principal: &str,
    generation: &str,
) -> Result<bool> {
    Ok(consumer_binding(
        data_dir,
        &json!({"principal_id":principal,"lifecycle_generation":generation}),
    )?
    .is_some_and(|binding| {
        binding.grant["expires_at"]
            .as_u64()
            .is_none_or(|expiry| expiry <= now_ts())
    }))
}

#[cfg(test)]
pub(in crate::api::gateway) fn consumer_binding_for_wallet_forward_test(
    principal_id: &str,
    page_id: &str,
    generation: &str,
    engine_peer: &iroh::PublicKey,
) -> ConsumerBinding {
    ConsumerBinding {
        grant: json!({"peer_did": engine_peer.to_string()}),
        selection_id: "remote-engine-selection".into(),
        adapter_id: "mock-browser-engine".into(),
        principal_id: principal_id.into(),
        page_id: page_id.into(),
        generation: generation.into(),
        stream_id: "stream:wallet-forward".into(),
        prepared: None,
        dispatch_started: true,
        preparation_closed: false,
        owner_launch_id: Some("launch:wallet-forward".into()),
        browser_instance: Some("window:wallet-forward".into()),
        reservation: None,
        stream_cleanup: None,
        cleanup_pending: false,
        retry_after: u64::MAX,
        terminal_retirement: false,
    }
}

#[cfg(test)]
pub(in crate::api::gateway) fn write_consumer_binding_for_test(
    data_dir: &FsPath,
    reservation: &BrowserLaunchReservation,
    principal_id: &str,
    stream_id: &str,
    expires_at: u64,
) -> Result<()> {
    insert_consumer(
        data_dir,
        &ConsumerBinding {
            grant: json!({
                "id": "grant:reconciliation-test",
                "grant_id": "grant:reconciliation-test",
                "expires_at": expires_at,
            }),
            selection_id: reservation.engine_route_provider().to_string(),
            adapter_id: "mock-browser-engine".into(),
            principal_id: principal_id.into(),
            page_id: reservation.page_id().to_string(),
            generation: reservation.generation().to_string(),
            stream_id: stream_id.into(),
            prepared: None,
            dispatch_started: true,
            preparation_closed: false,
            owner_launch_id: None,
            browser_instance: None,
            reservation: None,
            stream_cleanup: None,
            cleanup_pending: true,
            retry_after: 0,
            terminal_retirement: false,
        },
    )
}

pub(super) fn mark_consumer_reservation_terminal(
    data_dir: &FsPath,
    reservation: &BrowserLaunchReservation,
) -> Result<()> {
    if let Some(binding) = consumer_records(data_dir)?
        .into_iter()
        .find(|b| b.page_id == reservation.page_id())
    {
        ensure!(
            binding.generation == reservation.generation(),
            "Remote Engine terminal generation changed"
        );
        mark_consumer_terminal_retirement(data_dir, &binding.principal_id, &binding.generation)?;
    }
    Ok(())
}

// Called only after exact terminal native/stream cleanup has committed. Profile
// placement has its own durable lifetime, so completed pages do not fill this store.
pub(super) fn retire_consumer(
    data_dir: &FsPath,
    principal: &str,
    page: &str,
    generation: &str,
) -> Result<()> {
    let _guard = CONSUMER_WRITES.lock().unwrap();
    if let Some(binding) = consumer_records(data_dir)?
        .iter()
        .find(|b| b.page_id == page)
    {
        ensure!(
            binding.principal_id == principal && binding.generation == generation,
            "Remote Engine terminal owner changed"
        );
        if !binding.terminal_retirement {
            let mut terminal = binding.clone();
            terminal.terminal_retirement = true;
            gateway_browser_sessions::write_browser_json_atomic(
                data_dir,
                &consumer_path(data_dir, page),
                &terminal,
                64 * 1024,
            )
            .map_err(anyhow::Error::msg)?;
        }
        gateway_browser_sessions::remove_browser_durable_file(
            data_dir,
            consumer_path(data_dir, page),
        )
        .map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

pub(in crate::api::gateway) fn consumer_binding(
    data_dir: &FsPath,
    request: &Value,
) -> Result<Option<ConsumerBinding>> {
    let records = consumer_records(data_dir)?;
    let principal = request["principal_id"]
        .as_str()
        .context("Browser requester required")?;
    let mut matching = records.into_iter().filter(|binding| {
        binding.principal_id == principal
            && (request["page_id"].as_str() == Some(&binding.page_id)
                || request["lifecycle_generation"].as_str() == Some(&binding.generation))
    });
    let result = matching.next();
    ensure!(
        matching.next().is_none(),
        "Remote Engine route is ambiguous"
    );
    Ok(result)
}

pub(super) fn unresolved_preparation(
    data_dir: &FsPath,
    principal: &str,
    owner_launch_id: Option<&str>,
    browser_instance: Option<&str>,
) -> Result<bool> {
    let records = consumer_records(data_dir)?;
    Ok(records.iter().any(|binding| {
        binding.principal_id == principal
            && !binding.dispatch_started
            && (binding.owner_launch_id.is_none()
                || binding.owner_launch_id.as_deref() == owner_launch_id
                || binding.browser_instance.is_some()
                    && binding.browser_instance.as_deref() == browser_instance)
    }))
}

pub(super) fn require_profile_placement(
    data_dir: &FsPath,
    principal: &str,
    peer: Option<&str>,
) -> Result<()> {
    let records = consumer_records(data_dir)?;
    let placements = gateway_browser_sessions::read_bounded_browser_json_dir::<ProfilePlacement>(
        data_dir,
        &placement_dir(data_dir),
        1024,
        4096,
    )
    .map_err(anyhow::Error::msg)?;
    ensure!(
        !placements
            .iter()
            .any(|p| p.principal_id == principal && peer != Some(p.peer.as_str())),
        "This Browser profile requires an approved transfer from its owning Runtime"
    );
    ensure!(
        !records
            .iter()
            .any(|binding| binding.principal_id == principal
                && binding.dispatch_started
                && peer != binding.grant["peer_did"].as_str()),
        "This Browser profile requires an approved transfer from its owning Runtime"
    );
    Ok(())
}

pub(super) async fn select(
    gateway: &GatewayState,
    context: &HomeLaunchTokenContext,
    requested: Option<&str>,
    mode: BrowserDisplayMode,
    guarantee: BrowserGuaranteeLevel,
) -> Result<Option<(Value, String, String)>> {
    if !requested.is_some_and(|id| id.starts_with("remote-engine-")) {
        require_profile_placement(&gateway.data_dir, &context.principal_id, None)?;
        return Ok(None);
    }
    let registry = gateway
        .provider_registry
        .as_ref()
        .context("Runtime Engine transport unavailable")?;
    crate::carrier::browser_engine_media::configuration_ready(&gateway.data_dir)?;
    consumer_capacity(&gateway.data_dir)?;
    let grants = home_services_remote_engine_grants(
        &gateway.data_dir,
        context,
        gateway.collaboration_discovery_service.as_ref(),
    )?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    let (grant, inventory, adapter_id, id) =
        selected_inventory(registry, grants, requested.unwrap(), deadline).await?;
    require_profile_placement(
        &gateway.data_dir,
        &context.principal_id,
        grant["peer_did"].as_str(),
    )?;
    inventory.select(Some(&adapter_id), mode, guarantee)?;
    let readiness = tokio::time::timeout_at(
        deadline,
        crate::carrier::probe_browser_engine(registry, &grant, "readiness", Some(&adapter_id)),
    )
    .await??;
    ensure!(
        readiness["data"]["readiness"]["state"] == "ready",
        "Remote Engine is not ready"
    );
    ensure!(
        home_services_remote_engine_grants(
            &gateway.data_dir,
            context,
            gateway.collaboration_discovery_service.as_ref()
        )?
        .contains(&grant),
        "Remote Engine grant changed during selection"
    );
    // An existing local profile requires an approved transfer. Selecting
    // execution cannot silently fork its cookies onto another Runtime.
    let (local_profile, _) =
        browser_profile_launch_descriptor(&gateway.data_dir, &context.principal_id)?;
    ensure!(
        !local_profile.exists(),
        "This Browser profile requires an approved transfer before remote placement"
    );
    Ok(Some((grant, adapter_id, id)))
}

async fn selected_inventory(
    registry: &Arc<ProviderRegistry>,
    grants: Vec<Value>,
    requested: &str,
    deadline: tokio::time::Instant,
) -> Result<(Value, BrowserEngineInventory, String, String)> {
    let mut probes = tokio::task::JoinSet::new();
    for grant in grants.into_iter().take(4) {
        if grant["expires_at"]
            .as_u64()
            .is_none_or(|expiry| expiry <= now_ts())
            || grant["operations"]
                != browser_engine_binding::operations(Some(browser_engine_binding::EXECUTION_SCOPE))
        {
            continue;
        }
        let registry = registry.clone();
        probes.spawn(async move {
            let result = tokio::time::timeout_at(
                deadline,
                crate::carrier::probe_browser_engine(&registry, &grant, "status", None),
            )
            .await;
            let inventory = result
                .ok()
                .and_then(Result::ok)
                .and_then(|response| provider_response_data(&response))
                .and_then(|data| BrowserEngineInventory::from_status(&data).ok());
            (grant, inventory)
        });
    }
    while let Some(result) = probes.join_next().await {
        let Ok((grant, Some(inventory))) = result else {
            continue;
        };
        for adapter in &inventory.adapters {
            let id = selection_id(&grant, &adapter.id);
            if requested != id {
                continue;
            }
            let adapter_id = adapter.id.clone();
            return Ok((grant, inventory, adapter_id, id));
        }
    }
    Err(BrowserCompatibilityError::EngineNotFound.into())
}

pub(in crate::api::gateway) async fn prepare_consumer(
    gateway: &GatewayState,
    choice: &(Value, String, String),
    reservation: &BrowserLaunchReservation,
    principal: &str,
    profile: &Value,
    stream: &Value,
    input: &Value,
) -> Result<(BrowserVzTransportLaunch, Value)> {
    let registry = gateway
        .provider_registry
        .as_ref()
        .context("Runtime registry unavailable")?;
    let peer: iroh::PublicKey = choice.0["peer_did"]
        .as_str()
        .context("Engine peer required")?
        .parse()?;
    let stream_id = stream["stream_id"]
        .as_str()
        .context("Engine stream required")?;
    let mut binding = ConsumerBinding {
        grant: choice.0.clone(),
        adapter_id: choice.1.clone(),
        selection_id: choice.2.clone(),
        principal_id: principal.into(),
        page_id: reservation.page_id().into(),
        generation: reservation.generation().into(),
        stream_id: stream_id.into(),
        prepared: None,
        dispatch_started: false,
        preparation_closed: false,
        owner_launch_id: input["owner_launch_id"].as_str().map(str::to_owned),
        browser_instance: input["browser_instance"].as_str().map(str::to_owned),
        reservation: Some(reservation.clone()),
        stream_cleanup: browser_stream_cleanup(stream),
        cleanup_pending: false,
        retry_after: now_ts().saturating_add(120),
        terminal_retirement: false,
    };
    insert_consumer(&gateway.data_dir, &binding)?;
    ensure!(
        authenticated_engine_peer(&binding)? == peer,
        "Remote Wallet Engine peer differs from the selected grant"
    );
    browser_engine_binding::bind_egress(
        &gateway.data_dir,
        peer,
        reservation.page_id(),
        reservation.generation(),
        stream_id,
        FsPath::new(
            stream["adapter_ipc"]["runtime_stream_path"]
                .as_str()
                .context("Runtime Engine stream callback missing")?,
        ),
    )
    .await?;
    let request = json!({"lifecycle_generation":reservation.generation(),"page_id":reservation.page_id(),"vm_id":reservation.vm_id(),
        "profile_key":profile["profile_key"],"adapter_id":choice.1,"stream_id":stream_id,"target":stream["target"],
        "url":input["url"],"viewport":input["viewport"],"display_mode":input["display_mode"],"guarantee_level":input["guarantee_level"]});
    let warm = gateway.carrier_endpoint.clone().map(|endpoint| {
        let grant = choice.0.clone();
        tokio::spawn(async move {
            crate::carrier::browser_engine_media::preconnect_engine(&endpoint, &grant).await
        })
    });
    let response =
        crate::carrier::call_browser_engine(registry, &choice.0, "prepare_launch", request).await?;
    let data = provider_response_data(&response).context("Remote Engine preparation missing")?;
    ensure!(
        data["schema"] == "elastos.browser.remote-engine-preparation/v1"
            && data["page_id"] == binding.page_id
            && data["generation"] == binding.generation
            && data["adapter_id"] == binding.adapter_id,
        "Remote Engine preparation owner mismatch"
    );
    validate_browser_vz_viewer_turn_capability(
        &data["transport_authority"],
        &data["viewer_turn_capability"],
    )
    .map_err(anyhow::Error::msg)?;
    ensure!(
        data["viewer_turn_capability"]
            .get("viewer_ingress")
            .is_none(),
        "Engine cannot declare the consumer Runtime ingress"
    );
    let current = consumer_binding(
        &gateway.data_dir,
        &json!({"principal_id":principal,"page_id":binding.page_id}),
    )?
    .context("Engine preparation was retired")?;
    ensure!(
        !current.cleanup_pending && !current.preparation_closed,
        "Engine preparation was canceled"
    );
    let endpoint = gateway
        .carrier_endpoint
        .as_ref()
        .context("Runtime viewer ingress endpoint missing")?;
    let ingress = crate::carrier::browser_engine_media::start_ingress(
        &gateway.data_dir,
        endpoint,
        &choice.0,
        &data["transport_authority"],
        warm,
    )
    .await?;
    let mut viewer = data["viewer_turn_capability"].clone();
    viewer["turn_url"] = ingress["turn_url"].clone();
    viewer["ice_server"]["urls"] = json!([ingress["turn_url"]]);
    viewer["viewer_ingress"] = ingress;
    validate_browser_vz_viewer_turn_capability(&data["transport_authority"], &viewer)
        .map_err(anyhow::Error::msg)?;
    binding.prepared = Some(data.clone());
    if let Err(error) = save_consumer(&gateway.data_dir, &binding) {
        crate::carrier::browser_engine_media::close_ingress(
            &gateway.data_dir,
            &binding.page_id,
            &binding.generation,
        )
        .await?;
        return Err(error);
    }
    Ok((
        BrowserVzTransportLaunch {
            authority: data["transport_authority"].clone(),
            secret: Value::Null,
        },
        viewer,
    ))
}

pub(super) async fn dispatch_consumer(
    gateway: &GatewayState,
    binding: &ConsumerBinding,
    request: &Value,
) -> Result<Value> {
    let operation = request["op"]
        .as_str()
        .context("Engine operation required")?;
    if operation == "launch" {
        require_profile_placement(
            &gateway.data_dir,
            &binding.principal_id,
            binding.grant["peer_did"].as_str(),
        )?;
        retain_profile_placement(&gateway.data_dir, binding)?;
        let mut dispatched = binding.clone();
        dispatched.dispatch_started = true;
        write_consumer(&gateway.data_dir, &dispatched, false, true)?;
    }
    let mut local = if operation == "launch" {
        json!({})
    } else {
        request.clone()
    };
    local["page_id"] = json!(binding.page_id);
    local["lifecycle_generation"] = json!(binding.generation);
    if operation == "attach_stream" {
        let stream = &request["stream_session"];
        ensure!(
            stream["stream_id"] == binding.stream_id,
            "Engine transport is fixed for this launch generation"
        );
        local["stream_session"] =
            json!({"stream_id":stream["stream_id"],"target":stream["target"]});
    }
    crate::carrier::call_browser_engine(
        gateway
            .provider_registry
            .as_ref()
            .context("Runtime registry unavailable")?,
        &binding.grant,
        operation,
        local,
    )
    .await
}

pub(super) fn route_matches(
    gateway: &GatewayState,
    principal: &str,
    page: &str,
    route: &str,
) -> bool {
    consumer_binding(
        &gateway.data_dir,
        &json!({"principal_id":principal,"page_id":page}),
    )
    .ok()
    .flatten()
    .is_some_and(|binding| binding.selection_id == route)
}

pub(super) fn generation_route_matches(
    gateway: &GatewayState,
    principal: &str,
    generation: &str,
    route: &str,
) -> bool {
    consumer_binding(
        &gateway.data_dir,
        &json!({"principal_id":principal,"lifecycle_generation":generation}),
    )
    .ok()
    .flatten()
    .is_some_and(|binding| binding.selection_id == route)
}

pub(super) async fn close_transport_listener(
    gateway: &GatewayState,
    authority: &Value,
) -> std::result::Result<(), String> {
    let records = gateway_browser_sessions::read_bounded_browser_json_dir::<ConsumerBinding>(
        &gateway.data_dir,
        &consumer_dir(&gateway.data_dir),
        128,
        64 * 1024,
    )?;
    if let Some(binding) = records
        .into_iter()
        .find(|b| authority["generation"] == b.generation)
    {
        if binding.prepared.as_ref().map(|p| &p["transport_authority"]) != Some(authority) {
            return Err("Remote Engine terminal transport binding changed".into());
        }
        crate::carrier::browser_engine_media::close_ingress(
            &gateway.data_dir,
            &binding.page_id,
            &binding.generation,
        )
        .await
        .map_err(|error| error.to_string())?;
        return browser_engine_binding::remove_egress(
            &gateway.data_dir,
            &binding.page_id,
            &binding.generation,
        )
        .await
        .map_err(|e| e.to_string());
    }
    close_browser_vz_fixed_media_listener(authority).await
}

static PREPARATION_CLEANUPS: OnceLock<Mutex<BTreeSet<(PathBuf, String)>>> = OnceLock::new();

pub(in crate::api::gateway) async fn cancel_consumer_preparation(
    gateway: &GatewayState,
    reservation: &BrowserLaunchReservation,
) -> Result<bool> {
    let records = consumer_records(&gateway.data_dir)?;
    let Some(mut binding) = records.into_iter().find(|binding| {
        binding.page_id == reservation.page_id() && binding.generation == reservation.generation()
    }) else {
        return Ok(false);
    };
    ensure!(
        !binding.dispatch_started,
        "Engine native settlement is required"
    );
    binding.cleanup_pending = true;
    save_consumer(&gateway.data_dir, &binding)?;
    notify_browser_lifecycle_reconciler(&gateway.data_dir);
    settle_consumer_preparation(gateway, binding).await?;
    Ok(true)
}

async fn settle_consumer_preparation(
    gateway: &GatewayState,
    mut binding: ConsumerBinding,
) -> Result<()> {
    let key = (gateway.data_dir.clone(), binding.generation.clone());
    let claims = PREPARATION_CLEANUPS.get_or_init(Default::default);
    ensure!(
        claims.lock().await.insert(key.clone()),
        "Engine preparation cleanup is already pending"
    );
    // The claim is removed after a bounded attempt even when the caller drops its
    // wait. The durable record remains the source of truth for the next retry.
    let gateway = gateway.clone();
    let task = tokio::spawn(async move {
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            // Refresh under the cleanup claim, then commit cancellation intent
            // under CONSUMER_WRITES. A stale retry cannot undo launch admission.
            let Some(current) = consumer_binding(&gateway.data_dir,
                &json!({"principal_id":binding.principal_id,"page_id":binding.page_id}))? else { return Ok(()); };
            ensure!(current.generation == binding.generation, "Engine cleanup generation changed");
            binding = current;
            if binding.terminal_retirement { return retire_terminal_consumer(&gateway, &binding).await; }
            ensure!(!binding.dispatch_started, "Engine native settlement is required");
            binding.cleanup_pending = true;
            save_consumer(&gateway.data_dir, &binding)?;
            let reservation = binding.reservation.as_ref().context("Engine preparation reservation unavailable")?.clone();
            if !binding.preparation_closed {
                let response = crate::carrier::call_browser_engine(
                    gateway.provider_registry.as_ref().context("Runtime registry unavailable")?,
                    &binding.grant, "close_page",
                    json!({"page_id":binding.page_id,"lifecycle_generation":binding.generation,"cancel_preparation":true}),
                ).await?;
                let receipt = provider_response_data(&response).context("Engine preparation cleanup receipt missing")?;
                ensure!(response["status"] == "ok"
                    && receipt["schema"] == "elastos.browser.remote-engine-preparation-closed/v1"
                    && receipt["page_id"] == binding.page_id && receipt["generation"] == binding.generation
                    && receipt["principal_id"] == binding.principal_id && receipt["grant_id"] == binding.grant["grant_id"]
                    && receipt["requester_endpoint"].as_str() == gateway.carrier_endpoint.as_ref().map(|e| e.id().to_string()).as_deref()
                    && receipt["native_dispatch"] == false && receipt["cancellation_fenced"] == true,
                    "Engine preparation cleanup remains pending");
                binding.preparation_closed = true;
                save_consumer(&gateway.data_dir, &binding)?;
            }
            crate::carrier::browser_engine_media::close_ingress(&gateway.data_dir, &binding.page_id, &binding.generation).await?;
            browser_engine_binding::remove_egress(&gateway.data_dir, &binding.page_id, &binding.generation).await?;
            close_browser_runtime_stream_listener(&gateway.data_dir, &binding.stream_id).await.map_err(anyhow::Error::msg)?;
            // Stream failure already enters the existing durable stream retry
            // queue. This preparation stays retained until that same close wins.
            close_browser_stream_cleanup(&gateway, binding.stream_cleanup.clone()).await.map_err(anyhow::Error::msg)?;
            mark_consumer_terminal_retirement(&gateway.data_dir, &binding.principal_id, &binding.generation)?;
            discard_browser_vz_transport_preparation(&gateway.data_dir, &reservation).await.map_err(anyhow::Error::msg)?;
            release_browser_launch(&reservation).await;
            retire_consumer(&gateway.data_dir, &binding.principal_id, &binding.page_id, &binding.generation)?;
            Ok::<_, anyhow::Error>(())
        }).await.context("Engine preparation cleanup deadline").and_then(|result| result);
        PREPARATION_CLEANUPS
            .get()
            .unwrap()
            .lock()
            .await
            .remove(&key);
        result
    });
    task.await?
}

async fn retire_terminal_consumer(gateway: &GatewayState, binding: &ConsumerBinding) -> Result<()> {
    // Existing Engine/launch queues retain their route until their own durable
    // release commits. The marker lets each owner skip effects already settled.
    ensure!(
        !gateway_browser_sessions::browser_remote_cleanup_pending(
            &gateway.data_dir,
            &binding.generation
        )
        .await
        .map_err(anyhow::Error::msg)?,
        "Engine terminal ownership release remains pending"
    );
    if let Some(reservation) = &binding.reservation {
        discard_browser_vz_transport_preparation(&gateway.data_dir, reservation)
            .await
            .map_err(anyhow::Error::msg)?;
        release_browser_launch(reservation).await;
    }
    retire_consumer(
        &gateway.data_dir,
        &binding.principal_id,
        &binding.page_id,
        &binding.generation,
    )
}

pub(in crate::api::gateway) async fn retry_consumer_preparations(gateway: &GatewayState) -> bool {
    let records = match consumer_records(&gateway.data_dir) {
        Ok(records) => records,
        Err(error) => {
            tracing::warn!(%error, "Engine preparation cleanup inventory unavailable");
            return false;
        }
    };
    let mut settled = false;
    for binding in records
        .into_iter()
        .filter(|b| {
            b.terminal_retirement
                || !b.dispatch_started && (b.cleanup_pending || b.retry_after <= now_ts())
        })
        .take(BROWSER_LIFECYCLE_RECONCILIATION_BATCH_LIMIT)
    {
        if binding.terminal_retirement {
            if retire_terminal_consumer(gateway, &binding).await.is_ok() {
                settled = true;
            }
        } else if settle_consumer_preparation(gateway, binding).await.is_ok() {
            settled = true;
        }
    }
    settled
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pending_binding(serial: usize, principal: &str) -> ConsumerBinding {
        ConsumerBinding {
            grant: json!({"peer_did":"runtime-b"}),
            selection_id: "remote-engine-selection".into(),
            adapter_id: "adapter".into(),
            principal_id: principal.into(),
            page_id: format!("page:{serial}"),
            generation: format!("generation:{serial}"),
            stream_id: format!("stream:{serial}"),
            prepared: None,
            dispatch_started: false,
            preparation_closed: false,
            owner_launch_id: Some("launch:alice".into()),
            browser_instance: Some("window:alice".into()),
            reservation: None,
            stream_cleanup: None,
            cleanup_pending: false,
            retry_after: u64::MAX,
            terminal_retirement: false,
        }
    }

    fn test_engine_peer() -> iroh::PublicKey {
        iroh::SecretKey::from_bytes(&[9; 32]).public()
    }

    #[test]
    fn remote_wallet_consumer_mediation_keeps_wallet_bus_on_the_consumer() {
        let wallet = remote_wallet_consumer_mediation(
            "peer-alice",
            "page:vz-example",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        assert_eq!(wallet["schema"], REMOTE_WALLET_CONSUMER_MEDIATION_SCHEMA);
        assert_eq!(wallet["resolution"], "consumer_runtime");
        assert_eq!(wallet["bus"], "private");
        assert_eq!(wallet["requester_endpoint"], "peer-alice");
        assert!(wallet.get("principal_id").is_none());
        assert!(wallet.get("home_token").is_none());
        assert!(wallet.get("bridge_url").is_none());
    }

    #[test]
    fn remote_wallet_consumer_request_binds_authenticated_peer_page_and_generation() {
        let engine = test_engine_peer();
        let binding = consumer_binding_for_wallet_forward_test(
            "person:consumer",
            "page:7",
            "generation:7",
            &engine,
        );
        let mediation = remote_wallet_consumer_mediation(
            "peer-consumer",
            &binding.page_id,
            &binding.generation,
        );
        let request = remote_wallet_consumer_request(
            &mediation,
            "request_accounts",
            "https://ela.city/",
            "https://ela.city",
            json!({"method": "eth_requestAccounts"}),
        )
        .expect("consumer Wallet request");
        assert_eq!(request["schema"], REMOTE_WALLET_CONSUMER_REQUEST_SCHEMA);
        assert!(request.get("home_token").is_none());
        assert!(request.get("principal_id").is_none());
        admit_remote_wallet_consumer_request(&binding, &request, &engine).expect("matching page");
        let mut foreign_page = binding.clone();
        foreign_page.page_id = "page:other".into();
        assert!(admit_remote_wallet_consumer_request(&foreign_page, &request, &engine).is_err());
        let foreign_peer = iroh::SecretKey::from_bytes(&[23; 32]).public();
        assert!(admit_remote_wallet_consumer_request(&binding, &request, &foreign_peer).is_err());
        let mut tokenized = mediation.clone();
        tokenized["home_token"] = json!("launch-token");
        assert!(remote_wallet_consumer_request(
            &tokenized,
            "request_accounts",
            "https://ela.city/",
            "https://ela.city",
            json!({})
        )
        .is_err());
    }

    #[test]
    fn remote_engine_consumer_dispatch_and_retirement_reject_stale_writes() {
        let root = tempfile::tempdir().unwrap();
        for dispatch_first in [false, true] {
            let original = pending_binding(usize::from(dispatch_first), "alice");
            insert_consumer(root.path(), &original).unwrap();
            let mut dispatch = original.clone();
            dispatch.dispatch_started = true;
            let mut cancel = original.clone();
            cancel.cleanup_pending = true;
            let (winner, stale) = if dispatch_first {
                (&dispatch, &cancel)
            } else {
                (&cancel, &dispatch)
            };
            save_consumer(root.path(), winner).unwrap();
            assert!(save_consumer(root.path(), stale).is_err());
            let retained = consumer_records(root.path())
                .unwrap()
                .into_iter()
                .find(|b| b.page_id == original.page_id)
                .unwrap();
            assert_eq!(retained.dispatch_started, dispatch_first);
            assert_eq!(retained.cleanup_pending, !dispatch_first);
            mark_consumer_terminal_retirement(root.path(), "alice", &original.generation).unwrap();
            assert!(
                save_consumer(root.path(), winner).is_err(),
                "late completion cannot undo terminal retirement"
            );
            retire_consumer(
                root.path(),
                "alice",
                &original.page_id,
                &original.generation,
            )
            .unwrap();
        }
    }

    #[test]
    fn remote_engine_terminal_retirement_exceeds_128_lifetimes_and_preserves_profile_owner() {
        let root = tempfile::tempdir().unwrap();
        for serial in 0..140 {
            let mut binding = pending_binding(serial, "alice");
            consumer_capacity(root.path()).unwrap();
            insert_consumer(root.path(), &binding).unwrap();
            retain_profile_placement(root.path(), &binding).unwrap();
            binding.dispatch_started = true;
            save_consumer(root.path(), &binding).unwrap();
            // Terminal retirement is separate from profile placement and exact to
            // this generation. A foreign principal cannot compact a live owner.
            assert!(
                retire_consumer(root.path(), "bob", &binding.page_id, &binding.generation).is_err()
            );
            assert!(consumer_binding(
                root.path(),
                &json!({"principal_id":"alice","page_id":binding.page_id})
            )
            .unwrap()
            .is_some());
            retire_consumer(root.path(), "alice", &binding.page_id, &binding.generation).unwrap();
        }
        assert!(consumer_records(root.path()).unwrap().is_empty());
        consumer_capacity(root.path()).unwrap();
        assert!(consumer_binding(
            root.path(),
            &json!({"principal_id":"local","page_id":"local-page"})
        )
        .unwrap()
        .is_none());
        assert!(!unresolved_preparation(root.path(), "alice", Some("launch:alice"), None).unwrap());
        assert!(require_profile_placement(root.path(), "alice", None).is_err());
        require_profile_placement(root.path(), "alice", Some("runtime-b")).unwrap();
        require_profile_placement(root.path(), "bob", None).unwrap();
    }

    #[test]
    fn remote_engine_pending_capacity_rejects_before_write_and_keeps_exact_cleanup_available() {
        let root = tempfile::tempdir().unwrap();
        for serial in 0..CONSUMER_LIMIT {
            insert_consumer(root.path(), &pending_binding(serial, "alice")).unwrap();
        }
        assert!(consumer_capacity(root.path()).is_err());
        assert!(insert_consumer(root.path(), &pending_binding(CONSUMER_LIMIT, "bob")).is_err());
        assert_eq!(consumer_records(root.path()).unwrap().len(), CONSUMER_LIMIT);
        assert!(consumer_binding(
            root.path(),
            &json!({"principal_id":"local","page_id":"local-page"})
        )
        .unwrap()
        .is_none());
        assert!(consumer_binding(
            root.path(),
            &json!({"principal_id":"bob","page_id":"page:0"})
        )
        .unwrap()
        .is_none());
        let mut binding = consumer_binding(
            root.path(),
            &json!({"principal_id":"alice","page_id":"page:0"}),
        )
        .unwrap()
        .unwrap();
        binding.cleanup_pending = true;
        save_consumer(root.path(), &binding).unwrap();
        let stale = pending_binding(0, "alice");
        assert!(save_consumer(root.path(), &stale).is_err());
        retire_consumer(root.path(), "alice", &binding.page_id, &binding.generation).unwrap();
        assert!(
            save_consumer(root.path(), &stale).is_err(),
            "late completion cannot recreate a retired record"
        );
        consumer_capacity(root.path()).unwrap();
        insert_consumer(root.path(), &pending_binding(CONSUMER_LIMIT, "bob")).unwrap();
    }

    struct InventoryInvoker {
        failure: &'static str,
        calls: Mutex<Vec<Value>>,
    }
    #[async_trait::async_trait]
    impl elastos_runtime::provider::ProviderCarrierInvoker for InventoryInvoker {
        async fn invoke_carrier_provider(
            &self,
            _: &elastos_runtime::provider::ProviderCarrierRoute,
            _: &elastos_runtime::provider::ProviderInvocation,
            request: Value,
        ) -> std::result::Result<Value, elastos_runtime::provider::ProviderError> {
            self.calls.lock().await.push(request.clone());
            if request["grant_id"] == "unavailable" {
                match self.failure {
                    "stall" => {
                        std::future::pending::<()>().await;
                    }
                    "invalid" => return Ok(json!({"status":"ok","data":{}})),
                    _ => {
                        return Err(elastos_runtime::provider::ProviderError::Provider(
                            "offline".into(),
                        ))
                    }
                }
            }
            Ok(
                json!({"status":"ok","data":{"provider":"browser-engine-adapter","status":"configured","protocol_version":"2.1",
                "adapter_count":1,"direct_network":false,"wallet_injection":false,
                "adapters":[{"id":"adapter","default":true,"engine":"chromium_microvm","backing_substrate":"macos_vz",
                    "network_mode":"runtime_net_only","direct_network":false,"wallet_injection":false,
                    "supported_display_modes":["webrtc_remote_display"],"supported_guarantee_levels":["mechanism_microvm"]}]}}),
            )
        }
    }

    #[tokio::test]
    async fn remote_engine_launch_admission_rejects_fresh_and_stale_retiring_records() {
        let peer = iroh::SecretKey::from_bytes(&[43; 32]).public().to_string();
        let registry = Arc::new(ProviderRegistry::new());
        let invoker = Arc::new(InventoryInvoker {
            failure: "none",
            calls: Mutex::new(Vec::new()),
        });
        registry.set_carrier_invoker(invoker.clone()).await;
        let grant = json!({"grant_id":"healthy","principal_id":"alice","peer_did":peer,
            "connect_ticket":"test-ticket","operations":["launch"]});
        let root = tempfile::tempdir().unwrap();
        let mut gateway = GatewayState {
            data_dir: root.path().into(),
            cache_dir: root.path().join("cache"),
            provider_registry: Some(registry),
            carrier_endpoint: None,
            identity_manager: Arc::new(OnceLock::new()),
            collaboration_chat_product_port: None,
            collaboration_presence_product_port: None,
            collaboration_discovery_service: None,
        };
        let request = json!({"op":"launch"});
        let mut admitted = pending_binding(0, "alice");
        admitted.grant = grant.clone();
        insert_consumer(root.path(), &admitted).unwrap();
        dispatch_consumer(&gateway, &admitted, &request)
            .await
            .unwrap();
        let admitted = consumer_records(root.path()).unwrap().remove(0);
        assert!(admitted.dispatch_started);
        dispatch_consumer(&gateway, &admitted, &request)
            .await
            .unwrap();
        assert_eq!(
            invoker.calls.lock().await.len(),
            2,
            "active same-owner launch replay remains supported"
        );
        retire_consumer(
            root.path(),
            "alice",
            &admitted.page_id,
            &admitted.generation,
        )
        .unwrap();

        for flag in [
            "cleanup_pending",
            "preparation_closed",
            "terminal_retirement",
        ] {
            for previously_dispatched in [false, true] {
                let root = tempfile::tempdir().unwrap();
                gateway.data_dir = root.path().into();
                gateway.cache_dir = root.path().join("cache");
                let mut stale = pending_binding(1, "alice");
                stale.grant = grant.clone();
                stale.dispatch_started = previously_dispatched;
                insert_consumer(root.path(), &stale).unwrap();
                let mut retiring = stale.clone();
                match flag {
                    "cleanup_pending" => retiring.cleanup_pending = true,
                    "preparation_closed" => retiring.preparation_closed = true,
                    _ => retiring.terminal_retirement = true,
                }
                save_consumer(root.path(), &retiring).unwrap();
                // Match the provider response wrapper: read the CURRENT record
                // after cancellation, then invoke the actual launch dispatcher.
                let fresh = consumer_binding(
                    root.path(),
                    &json!({"principal_id":"alice","page_id":retiring.page_id}),
                )
                .unwrap()
                .unwrap();
                let before = std::fs::read(consumer_path(root.path(), &retiring.page_id)).unwrap();
                for candidate in [&fresh, &stale] {
                    let error = dispatch_consumer(&gateway, candidate, &request)
                        .await
                        .unwrap_err();
                    assert!(
                        error.to_string().contains("retiring"),
                        "{flag}/{previously_dispatched}: {error}"
                    );
                    assert_eq!(
                        std::fs::read(consumer_path(root.path(), &retiring.page_id)).unwrap(),
                        before,
                        "rejected admission preserves the exact durable owner"
                    );
                    assert_eq!(
                        invoker.calls.lock().await.len(),
                        2,
                        "retirement blocks the Carrier launch call"
                    );
                }
                retiring.terminal_retirement = true;
                save_consumer(root.path(), &retiring).unwrap();
                retire_consumer(
                    root.path(),
                    "alice",
                    &retiring.page_id,
                    &retiring.generation,
                )
                .unwrap();
            }
        }
    }

    #[tokio::test]
    async fn remote_engine_selection_isolates_unselected_failure_and_never_falls_back() {
        let peer = iroh::SecretKey::from_bytes(&[41; 32]).public().to_string();
        let grant = |id: &str| {
            json!({"id":id,"grant_id":id,"principal_id":"alice","peer_did":peer,
            "connect_ticket":"test-ticket","expires_at":now_ts()+3600,
            "operations":browser_engine_binding::operations(Some(browser_engine_binding::EXECUTION_SCOPE))})
        };
        let unavailable = grant("unavailable");
        let healthy = grant("healthy");
        for failure in ["offline", "invalid", "stall"] {
            let registry = Arc::new(ProviderRegistry::new());
            let invoker = Arc::new(InventoryInvoker {
                failure,
                calls: Mutex::new(Vec::new()),
            });
            registry.set_carrier_invoker(invoker.clone()).await;
            let selected = selected_inventory(
                &registry,
                vec![unavailable.clone(), healthy.clone()],
                &selection_id(&healthy, "adapter"),
                tokio::time::Instant::now() + Duration::from_millis(250),
            )
            .await
            .unwrap();
            assert_eq!(selected.0, healthy);
            assert_eq!(selected.2, "adapter");
            let error = selected_inventory(
                &registry,
                vec![unavailable.clone(), healthy.clone()],
                &selection_id(&unavailable, "adapter"),
                tokio::time::Instant::now() + Duration::from_millis(40),
            )
            .await
            .unwrap_err();
            assert_eq!(
                error.downcast_ref::<BrowserCompatibilityError>(),
                Some(&BrowserCompatibilityError::EngineNotFound)
            );
            assert!(
                invoker
                    .calls
                    .lock()
                    .await
                    .iter()
                    .all(|request| request["op"] == "status"),
                "selection has no launch or transport effects"
            );
        }
    }

    #[test]
    fn remote_engine_profile_placement_does_not_silently_fork_or_reset() {
        let root = tempfile::tempdir().unwrap();
        let mut binding = ConsumerBinding {
            grant: json!({"peer_did":"runtime-b"}),
            selection_id: "selection".into(),
            adapter_id: "adapter".into(),
            principal_id: "alice".into(),
            page_id: "page:owned".into(),
            generation: "generation".into(),
            stream_id: "stream:owned".into(),
            prepared: None,
            dispatch_started: false,
            preparation_closed: false,
            owner_launch_id: Some("launch:alice".into()),
            browser_instance: Some("browser:alice".into()),
            reservation: None,
            stream_cleanup: None,
            cleanup_pending: false,
            retry_after: 0,
            terminal_retirement: false,
        };
        insert_consumer(root.path(), &binding).unwrap();
        require_profile_placement(root.path(), "alice", None).unwrap();
        assert!(unresolved_preparation(root.path(), "alice", Some("launch:alice"), None).unwrap());
        assert!(unresolved_preparation(
            root.path(),
            "alice",
            Some("launch:reload"),
            Some("browser:alice")
        )
        .unwrap());
        assert!(!unresolved_preparation(
            root.path(),
            "alice",
            Some("launch:other"),
            Some("browser:other")
        )
        .unwrap());
        assert!(!unresolved_preparation(root.path(), "bob", Some("launch:alice"), None).unwrap());
        binding.preparation_closed = true;
        save_consumer(root.path(), &binding).unwrap();
        assert!(
            unresolved_preparation(root.path(), "alice", Some("launch:alice"), None).unwrap(),
            "remote cancellation alone does not settle retained local cleanup"
        );
        binding.dispatch_started = true;
        save_consumer(root.path(), &binding).unwrap();
        require_profile_placement(root.path(), "alice", Some("runtime-b")).unwrap();
        require_profile_placement(root.path(), "bob", None).unwrap();
        assert!(require_profile_placement(root.path(), "alice", None).is_err());
        assert!(require_profile_placement(root.path(), "alice", Some("runtime-c")).is_err());
        // Private placement survives process-local maps and retained grant changes.
        binding.grant["grant_id"] = json!("renewed");
        save_consumer(root.path(), &binding).unwrap();
        require_profile_placement(root.path(), "alice", Some("runtime-b")).unwrap();
    }
}
