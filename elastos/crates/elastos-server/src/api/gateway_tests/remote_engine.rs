use super::home_system::configured_discovery_network_profile_for_test;
use super::*;
use crate::api::gateway::gateway_browser::{
    browser_attach_runtime_stream_path, browser_gateway_session_status,
    cleanup_stale_browser_pages, gateway_browser_remote, reserve_browser_launch,
    set_browser_durable_delete_failure, BrowserLaunchLifecycle,
};
use crate::carrier::browser_engine_binding::RemoteEngineOwner;
use elastos_runtime::provider::{ProviderCarrierInvoker, ProviderCarrierRoute, ProviderInvocation};
use elastos_runtime::signature::generate_keypair;

#[derive(Default)]
pub(super) struct RemoteEngineFixture {
    calls: TokioMutex<Vec<Value>>,
    hold_launch: std::sync::atomic::AtomicBool,
    hold_input: std::sync::atomic::AtomicBool,
    bad_close: std::sync::atomic::AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl Provider for RemoteEngineFixture {
    fn name(&self) -> &'static str {
        "remote-engine-fixture"
    }
    fn schemes(&self) -> Vec<&'static str> {
        vec!["browser-engine"]
    }
    async fn handle(&self, _: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider("raw only".into()))
    }
    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        use std::sync::atomic::Ordering::SeqCst;
        self.calls.lock().await.push(request.clone());
        self.entered.notify_one();
        if request["op"] == "launch" && self.hold_launch.load(SeqCst)
            || request["op"] == "input" && self.hold_input.load(SeqCst)
        {
            self.release.notified().await;
        }
        let mut response = if request["op"] == "close_page" {
            assert_browser_close_request_contract(request);
            mock_browser_terminal_cleanup_response(request)
        } else {
            MockBrowserEngineProvider.send_raw(request).await?
        };
        if request["op"] == "status" && request.get("lifecycle_generation").is_none() {
            response["data"]["adapters"][0]["supported_guarantee_levels"] =
                json!(["mechanism_microvm"]);
            response["data"]["adapters"][0]["backing_substrate"] = json!("macos_vz");
        }
        if request["op"] == "launch" {
            response["data"]["engine"] = json!("chromium_microvm");
            response["data"]["runtime_cleanup"]["engine"] = json!("chromium_microvm");
            response["data"]["isolation"] = json!({"kind":"per_launch_vm_target"});
            response["data"]["display_session"]["offerer"] = json!("engine");
            // Use the same strict request fields as the native adapter. Runtime
            // metadata, remote tickets and consumer file paths have been consumed.
            let keys = request
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            assert_eq!(
                keys,
                BTreeSet::from([
                    "op",
                    "url",
                    "stream_session",
                    "lifecycle_generation",
                    "principal_id",
                    "profile",
                    "wallet",
                    "viewport",
                    "display_mode",
                    "guarantee_level",
                    "adapter_id",
                    "page_id",
                    "vm_id",
                    "transport_authority",
                    "transport_secret"
                ])
            );
            serde_json::from_value::<elastos_common::browser_protocol::BrowserProfileDescriptor>(
                request["profile"].clone(),
            )
            .unwrap();
            let authority = &request["transport_authority"];
            let receipt = json!({"schema":"elastos.browser.vz-transport-effect-receipt/v1",
                "binding_hash":authority["binding_hash"],"generation":authority["generation"],"page_id":authority["page_id"],
                "vm_id":authority["vm_id"],"expires_at_unix_ms":authority["expires_at_unix_ms"],"terminal":true,
                "effects":{"vz_network_devices_zero":true,"guest_bootstrap_validated":true,"guest_loopback_only":true,
                    "guest_interfaces":["lo"],"guest_default_route_absent":true,"guest_direct_network_absent":true,
                    "ordinary_stream_fixed_target":true,"media_stream_fixed_target":true,"turn_launch_owned":true,
                    "turn_listener_loopback":true,"hibernation_disabled":true}});
            response["data"]["page_id"] = request["page_id"].clone();
            response["data"]["runtime_cleanup"]["page_id"] = request["page_id"].clone();
            response["data"]["runtime_cleanup"]["transport_authority"] = authority.clone();
            response["data"]["runtime_cleanup"]["transport_receipt"] = receipt.clone();
            response["data"]["transport_receipt"] = receipt;
            response["data"]["display_session"]
                .as_object_mut()
                .unwrap()
                .remove("ice_servers");
            response["data"]["display_session"]["ice_connection_policy"] =
                json!("engine_relay_only");
        }
        if request["op"] == "close_page" {
            for effect in [
                "transport_session_absent",
                "turn_process_absent",
                "turn_listener_absent",
                "turn_relay_ports_absent",
                "ordinary_vsock_bridge_absent",
                "media_vsock_bridge_absent",
                "bootstrap_vsock_bridge_absent",
                "hibernation_state_absent",
            ] {
                response["data"]["effects"][effect] = json!(true);
            }
            if self.bad_close.load(SeqCst) {
                response["data"]["effects"]["turn_process_absent"] = json!(false);
            }
        }
        Ok(response)
    }
}

fn config(root: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(root.join("config")).unwrap();
    let path = root.join("config/browser-vz-vsock-transport.json");
    std::fs::write(&path, serde_json::to_vec(&json!({"schema":"elastos.browser.vz-transport-config/v1","enabled":true,
        "turn_listen_host":"127.0.0.1","turn_advertised_host":"127.0.0.1","turn_relay_host":"127.0.0.1",
        "turn_port_start":43000,"turn_port_end":43031,"turn_relay_port_start":43100,"turn_relay_port_end":43227,
        "turn_relay_block_size":8,"guest_turn_host":"127.0.0.1","guest_turn_port":3478,
        "bootstrap_vsock_port":19090,"egress_vsock_port":19091,"media_vsock_port":19093,"ttl_secs":300})).unwrap()).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

pub(super) struct ConsumerExitFixture {
    pub(super) stream_id: String,
    pub(super) calls: TokioMutex<Vec<Value>>,
}

#[async_trait::async_trait]
impl Provider for ConsumerExitFixture {
    fn name(&self) -> &'static str {
        "consumer-exit-fixture"
    }
    fn schemes(&self) -> Vec<&'static str> {
        vec!["exit"]
    }
    async fn handle(&self, _: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider("raw only".into()))
    }
    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        self.calls.lock().await.push(request.clone());
        let mut response = MockAttachedExitProvider {
            relay_ipc_path: None,
            stream_id: self.stream_id.clone(),
        }
        .send_raw(request)
        .await?;
        if request["op"] == "open_stream" {
            // Match the actual Exit provider's principal-bound reservation. The
            // legacy attached mock omits this required cleanup ownership field.
            response["data"]["principal_id"] = request["principal_id"].clone();
        }
        Ok(response)
    }
}

#[tokio::test]
async fn remote_engine_preparation_replay_close_and_late_input_keep_exact_native_owner() {
    use std::sync::atomic::Ordering::SeqCst;
    let root = tempfile::tempdir().unwrap();
    config(root.path());
    let provider = Arc::new(RemoteEngineFixture::default());
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("browser-engine", provider.clone())
        .await
        .unwrap();
    let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .bind()
        .await
        .unwrap();
    let (trusted, _) = generate_keypair();
    let network = configured_discovery_network_profile_for_test(&trusted, "remote-engine-binding");
    let grant = crate::carrier::BrowserEngineGrant {
        provider_principal_id: "provider".into(),
        requester_principal_id: "consumer".into(),
        requester_endpoint: iroh::SecretKey::from_bytes(&[45; 32]).public(),
        grant_id: "grant:remote-engine".into(),
        revision: now_ts(),
        expires_at: now_ts() + 3600,
        execution_allowed: true,
    };
    let generation = format!("sha256:{}", "c".repeat(64));
    let page_id = format!(
        "page:vz-{}",
        hex::encode(Sha256::digest(format!("{generation}\npage")))
    );
    let prepare = json!({"op":"prepare_launch","principal_id":grant.requester_principal_id,"grant_id":grant.grant_id,
        "lifecycle_generation":generation,"page_id":page_id,
        "vm_id":format!("browser-vm-{}",hex::encode(Sha256::digest(format!("{generation}\nvm")))),
        "profile_key":elastos_common::browser_profile_key_from_value(&grant.requester_principal_id),"adapter_id":"mock-browser-engine",
        "stream_id":"stream:remote-engine-test","target":"tls://example.com:443","url":"https://example.com/",
        "viewport":{"width":1280,"height":720},"display_mode":"webrtc_remote_display","guarantee_level":"mechanism_microvm"});
    let owner = RemoteEngineOwner::from_launch(&grant, &prepare).unwrap();
    let call = |request: Value| {
        let root = root.path().to_owned();
        let registry = registry.clone();
        let endpoint = endpoint.clone();
        let network = network.clone();
        let grant = grant.clone();
        async move {
            gateway_browser_remote::invoke(
                &root,
                registry,
                endpoint,
                grant.requester_endpoint,
                network,
                Some(grant),
                &request,
            )
            .await
        }
    };
    let mut injected = prepare.clone();
    injected["profile"] = json!({"disk_path":"/private/foreign"});
    assert!(call(injected).await.is_err());
    assert!(provider.calls.lock().await.is_empty());
    let prepared = call(prepare.clone()).await.unwrap();
    assert_eq!(prepared["data"]["generation"], generation);
    assert!(prepared["data"]["transport_secret"].is_null());
    assert!(prepared["data"]["transport_authority"]["turn"]["auth_secret"].is_null());
    assert!(prepared["data"]["transport_authority"]["turn"]["auth_secret_hash"].is_string());
    assert_eq!(call(prepare.clone()).await.unwrap(), prepared);
    let mut changed = prepare.clone();
    changed["url"] = json!("https://foreign.invalid/");
    assert!(call(changed).await.is_err());
    let base = json!({"op":"launch","principal_id":grant.requester_principal_id,"grant_id":grant.grant_id,
        "page_id":page_id,"lifecycle_generation":generation});
    provider.hold_launch.store(true, SeqCst);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), call(base.clone()))
            .await
            .is_err(),
        "a lost waiter retains the actual launch task"
    );
    assert_eq!(
        provider
            .calls
            .lock()
            .await
            .iter()
            .filter(|r| r["op"] == "launch")
            .count(),
        1
    );
    provider.hold_launch.store(false, SeqCst);
    provider.release.notify_one();
    let launched = call(base.clone()).await.unwrap();
    assert_eq!(launched["status"], "ok", "{launched}");
    assert_eq!(call(base.clone()).await.unwrap(), launched);
    let mut premature_cancel = base.clone();
    premature_cancel["op"] = json!("close_page");
    premature_cancel["cancel_preparation"] = json!(true);
    assert!(
        call(premature_cancel).await.is_err(),
        "preparation absence cannot replace a native cleanup receipt"
    );
    let requests = provider.calls.lock().await.clone();
    let native = requests.iter().find(|r| r["op"] == "launch").unwrap();
    assert_eq!(native["principal_id"], owner.storage_principal());
    assert!(native["profile"]["disk_path"]
        .as_str()
        .unwrap()
        .starts_with(root.path().to_str().unwrap()));
    assert_eq!(native["profile"]["profile_key"], prepare["profile_key"]);
    assert_eq!(native["wallet"], json!({}));
    assert_eq!(native["transport_authority"]["generation"], generation);
    assert_eq!(requests.iter().filter(|r| r["op"] == "launch").count(), 1);
    let mut stale = base.clone();
    stale["lifecycle_generation"] = json!(format!("sha256:{}", "d".repeat(64)));
    assert!(call(stale).await.is_err());
    let before = provider.calls.lock().await.len();
    let mut changed_stream = base.clone();
    changed_stream["op"] = json!("attach_stream");
    changed_stream["stream_session"] =
        json!({"stream_id":"stream:substituted","target":"tls://foreign.invalid:443"});
    assert!(call(changed_stream).await.is_err());
    assert_eq!(provider.calls.lock().await.len(), before);
    for channel in ["video", "audio"] {
        let mut signal = base.clone();
        signal["op"] = json!("webrtc_signal");
        signal["channel"] = json!(channel);
        signal["signal"] = json!({"schema":"elastos.browser.webrtc-answer/v1","type":"answer","sdp":"v=0\r\n","display_generation":"display:owned"});
        assert_eq!(call(signal).await.unwrap()["data"]["page_id"], page_id);
        let calls = provider.calls.lock().await;
        let native = calls.last().unwrap();
        assert_eq!(native["principal_id"], owner.storage_principal());
        assert_eq!(native["channel"], channel);
        assert_eq!(native["signal"]["display_generation"], "display:owned");
        assert!(native.get("grant_id").is_none());
        assert!(native.get("lifecycle_generation").is_none());
    }
    provider.hold_input.store(true, SeqCst);
    let mut input = base.clone();
    input["op"] = json!("input");
    input["event"] = json!({"type":"paste_text","text":"same page"});
    let pending = tokio::spawn(call(input));
    loop {
        let wake = provider.entered.notified();
        if provider
            .calls
            .lock()
            .await
            .iter()
            .any(|r| r["op"] == "input")
        {
            break;
        }
        wake.await;
    }
    let mut close = base.clone();
    close["op"] = json!("close_page");
    close["runtime_cleanup"] = launched["data"]["runtime_cleanup"].clone();
    provider.bad_close.store(true, SeqCst);
    assert!(call(close.clone()).await.is_err());
    provider.bad_close.store(false, SeqCst);
    let terminal = call(close.clone()).await.unwrap();
    assert_eq!(terminal["data"]["generation"], generation);
    assert_eq!(terminal["data"]["effects"].as_object().unwrap().len(), 13);
    assert_eq!(call(close).await.unwrap(), terminal);
    provider.release.notify_one();
    assert!(
        pending.await.unwrap().is_err(),
        "close fences a late input acknowledgement"
    );
    assert!(call(base).await.is_err());
    // Both wire orderings use a durable cancellation fence. Losing an old
    // prepare response cannot recreate the serving owner after cancellation.
    for prepare_first in [false, true] {
        let generation = format!(
            "sha256:{}",
            hex::encode(Sha256::digest(format!("cancel-order-{prepare_first}")))
        );
        let mut request = prepare.clone();
        request["lifecycle_generation"] = json!(generation);
        request["page_id"] = json!(format!(
            "page:vz-{}",
            hex::encode(Sha256::digest(format!("{generation}\npage")))
        ));
        request["vm_id"] = json!(format!(
            "browser-vm-{}",
            hex::encode(Sha256::digest(format!("{generation}\nvm")))
        ));
        request["stream_id"] = json!(format!("stream:cancel-{prepare_first}"));
        let before = provider.calls.lock().await.len();
        if prepare_first {
            call(request.clone()).await.unwrap();
        }
        let cancel = json!({"op":"close_page","principal_id":grant.requester_principal_id,"grant_id":grant.grant_id,
            "page_id":request["page_id"],"lifecycle_generation":generation,"cancel_preparation":true});
        let terminal = call(cancel.clone()).await.unwrap();
        assert_eq!(terminal["data"]["native_dispatch"], false);
        assert_eq!(terminal["data"]["cancellation_fenced"], true);
        assert_eq!(
            terminal["data"]["requester_endpoint"],
            grant.requester_endpoint.to_string()
        );
        assert_eq!(call(cancel).await.unwrap(), terminal);
        assert!(call(request.clone()).await.is_err());
        assert!(!browser_runtime_stream_socket_path(
            root.path(),
            request["stream_id"].as_str().unwrap()
        )
        .unwrap()
        .exists());
        let calls = provider.calls.lock().await;
        assert!(calls[before..]
            .iter()
            .all(|r| r["op"] != "launch" && r["op"] != "close_page"));
    }
    // Pause cancellation exactly between its undispatched check and durable
    // fence. A launch that captured the same page must wait for that decision.
    let generation = format!(
        "sha256:{}",
        hex::encode(Sha256::digest("cancel-launch-race"))
    );
    let race_page = format!(
        "page:vz-{}",
        hex::encode(Sha256::digest(format!("{generation}\npage")))
    );
    let mut race_prepare = prepare.clone();
    race_prepare["lifecycle_generation"] = json!(generation);
    race_prepare["page_id"] = json!(race_page);
    race_prepare["vm_id"] = json!(format!(
        "browser-vm-{}",
        hex::encode(Sha256::digest(format!("{generation}\nvm")))
    ));
    race_prepare["stream_id"] = json!("stream:cancel-launch-race");
    call(race_prepare).await.unwrap();
    let launch_pause = gateway_browser_remote::hold_remote_admission_for_test(
        root.path(),
        &race_page,
        "launch_captured",
    )
    .await;
    let cancel_pause = gateway_browser_remote::hold_remote_admission_for_test(
        root.path(),
        &race_page,
        "cancel_checked",
    )
    .await;
    let race_base = json!({"op":"launch","principal_id":grant.requester_principal_id,"grant_id":grant.grant_id,"page_id":race_page,"lifecycle_generation":generation});
    let before = provider.calls.lock().await.len();
    let mut launching = tokio::spawn(call(race_base.clone()));
    tokio::time::timeout(Duration::from_secs(2), launch_pause.entered.notified())
        .await
        .unwrap();
    let mut cancel = race_base;
    cancel["op"] = json!("close_page");
    cancel["cancel_preparation"] = json!(true);
    let cancelling = tokio::spawn(call(cancel));
    tokio::time::timeout(Duration::from_secs(2), cancel_pause.entered.notified())
        .await
        .unwrap();
    launch_pause.resume.notify_one();
    let early = tokio::time::timeout(Duration::from_millis(30), &mut launching).await;
    let dispatched = provider.calls.lock().await[before..]
        .iter()
        .any(|r| r["op"] == "launch");
    cancel_pause.resume.notify_one();
    let cancelled = tokio::time::timeout(Duration::from_secs(2), cancelling)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        early.is_err(),
        "captured launch crossed the cancellation admission lock"
    );
    assert!(
        !dispatched,
        "native dispatch crossed the check-to-persist boundary"
    );
    assert!(launching.await.unwrap().is_err());
    assert_eq!(cancelled["data"]["native_dispatch"], false);
    assert!(provider.calls.lock().await[before..]
        .iter()
        .all(|r| r["op"] != "launch" && r["op"] != "close_page"));
    endpoint.close().await;
}

// Called with a grant issued through real Services approval and consumed through
// the Runtime Carrier invoker; no synthetic caller is passed to the receiver.
pub(super) async fn exercise_consumer_http_remote_engine(
    app: &axum::Router,
    token: &str,
    consumer_root: &std::path::Path,
    adapter: &Value,
    principal: &str,
    exit: &ConsumerExitFixture,
    gateway: &GatewayState,
    native_calls: &TokioMutex<Vec<Value>>,
) {
    // Only the consumer Runtime owns the public TURN ingress. The serving
    // Engine retains its loopback native TURN authority and exact cleanup owner.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    std::fs::create_dir_all(consumer_root.join("config")).unwrap();
    std::fs::write(
        consumer_root.join("config/browser-viewer-ingress.json"),
        json!({"schema":"elastos.browser.viewer-ingress-config/v1",
            "listen_host":"127.0.0.1","advertised_host":"127.0.0.1",
            "port_start":address.port(),"port_end":address.port()})
        .to_string(),
    )
    .unwrap();
    let request = |method: &str, path: String, body: Value| {
        let app = app.clone();
        let token = token.to_owned();
        let method = method.to_owned();
        async move {
            let response = app
                .oneshot(
                    test_browser_request("localhost:61180", "null")
                        .method(method.as_str())
                        .uri(path)
                        .header("x-elastos-home-token", token)
                        .header(CONTENT_TYPE, "application/json")
                        .body(if body.is_null() {
                            Body::empty()
                        } else {
                            Body::from(body.to_string())
                        })
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            assert_eq!(
                status,
                StatusCode::OK,
                "{}",
                String::from_utf8_lossy(&bytes)
            );
            serde_json::from_slice::<Value>(&bytes).unwrap()
        }
    };
    let opened = request(
        "POST",
        "/api/apps/browser/open".into(),
        json!({
            "url":"https://example.com/", "adapter_id":adapter,
            "viewport":{"width":1280,"height":720}, "display_mode":"webrtc_remote_display",
            "guarantee_level":"mechanism_microvm"
        }),
    )
    .await;
    let page = opened["engine_page"]["page_id"].as_str().unwrap();
    let cleanup_id = opened["runtime_cleanup"]["id"].as_str().unwrap().to_owned();
    let stream_id = opened["stream_session"]["stream_id"].as_str().unwrap();
    let socket = browser_runtime_stream_socket_path(consumer_root, stream_id).unwrap();
    assert!(socket.exists());
    let summary = request("GET", "/api/apps/browser/summary".into(), Value::Null).await;
    assert_eq!(summary["sessions"]["active_sessions"], 1);
    let retained = &summary["sessions"]["recoverable_page"];
    assert_eq!(retained["service_selection"]["engine_id"], *adapter);
    assert_eq!(retained["service_selection"]["exit_id"], "");
    assert_eq!(retained["engine_page"]["page_id"], page);
    let display = &opened["engine_page"]["display_session"];
    let turn = &display["runtime_turn"];
    assert_eq!(turn["viewer_ingress"]["page_id"], page, "{opened}");
    assert_eq!(
        turn["turn_url"],
        format!("turn:127.0.0.1:{}?transport=tcp", address.port())
    );
    assert_eq!(turn["viewer_ingress"]["turn_url"], turn["turn_url"]);
    let profile_key = elastos_common::browser_profile_key_from_value(principal).unwrap();
    assert_eq!(
        summary["sessions"]["lifecycle"]["sessions"][0]["profile_key_hash"],
        format!("sha256:{}", hex::encode(&Sha256::digest(profile_key)[..8]))
    );
    for channel in ["video", "audio"] {
        let signal = request(
            "POST",
            format!("/api/apps/browser/pages/{page}/webrtc"),
            json!({"type":"answer","channel":channel,"sdp":"v=0\r\ns=Runtime remote Engine\r\n"}),
        )
        .await;
        assert_eq!(signal["schema"], "elastos.browser.webrtc-signal-ack/v1");
        assert_eq!(signal["page_id"], page);
        assert_eq!(signal["type"], "answer");
    }
    let terminal = request(
        "POST",
        format!("/api/apps/browser/pages/{page}/close"),
        json!({"schema":"elastos.browser.close-request/v2","cleanup_id":cleanup_id}),
    )
    .await;
    assert_eq!(terminal["page_id"], page);
    assert_eq!(terminal["cleanup_id"], cleanup_id);
    assert_eq!(terminal["closed"], true);
    let effects = terminal["terminal_effects"].as_object().unwrap();
    assert_eq!(effects.len(), 13, "{terminal}");
    assert!(effects.values().all(|value| value == true));
    let summary = request("GET", "/api/apps/browser/summary".into(), Value::Null).await;
    for count in [
        "active_sessions",
        "total_sessions",
        "principal_sessions",
        "launch_reconciliation_obligations",
        "engine_cleanup_obligations",
    ] {
        assert_eq!(summary["sessions"][count], 0, "{summary}");
    }
    assert!(summary["sessions"]["recoverable_page"].is_null());
    assert!(
        gateway_browser_remote::consumer_binding(
            consumer_root,
            &json!({"principal_id":principal,"page_id":page})
        )
        .unwrap()
        .is_none(),
        "actual HTTP terminal cleanup retires its per-generation consumer record"
    );
    let calls = exit.calls.lock().await;
    let closes = calls
        .iter()
        .filter(|call| call["op"] == "close_stream")
        .collect::<Vec<_>>();
    assert_eq!(closes.len(), 1);
    assert_eq!(closes[0]["principal_id"], principal);
    assert_eq!(closes[0]["stream_id"], stream_id);
    drop(calls);
    assert!(!socket.exists());
    let listener =
        std::net::TcpListener::bind(address).expect("consumer ingress closed with owner");
    drop(listener);

    // Native and Exit close succeed, but the terminal consumer route cannot be
    // deleted. The real lifecycle sweep must retry only that durable retirement.
    let opened = request(
        "POST",
        "/api/apps/browser/open".into(),
        json!({
            "url":"https://example.com/", "adapter_id":adapter,
            "viewport":{"width":1280,"height":720}, "display_mode":"webrtc_remote_display",
            "guarantee_level":"mechanism_microvm"
        }),
    )
    .await;
    let page = opened["engine_page"]["page_id"].as_str().unwrap();
    let cleanup_id = opened["runtime_cleanup"]["id"].as_str().unwrap();
    let path = consumer_root
        .join("Runtime/BrowserLifecycle/remote-engine")
        .join(format!("{}.json", hex::encode(Sha256::digest(page))));
    let native_before = native_calls
        .lock()
        .await
        .iter()
        .filter(|r| r["op"] == "close_page")
        .count();
    let exit_before = exit
        .calls
        .lock()
        .await
        .iter()
        .filter(|r| r["op"] == "close_stream")
        .count();
    set_browser_durable_delete_failure(&path, true);
    let response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page}/close"))
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"schema":"elastos.browser.close-request/v2","cleanup_id":cleanup_id})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    set_browser_durable_delete_failure(&path, false);
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        path.exists(),
        "terminal route remains a durable retry owner"
    );
    let retained: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(retained["terminal_retirement"], true);
    assert_eq!(
        native_calls
            .lock()
            .await
            .iter()
            .filter(|r| r["op"] == "close_page")
            .count(),
        native_before + 1
    );
    assert_eq!(
        exit.calls
            .lock()
            .await
            .iter()
            .filter(|r| r["op"] == "close_stream")
            .count(),
        exit_before + 1
    );
    assert!(cleanup_stale_browser_pages(gateway).await);
    assert!(
        !path.exists(),
        "actual reconciler retires the terminal route"
    );
    assert!(!cleanup_stale_browser_pages(gateway).await);
    assert_eq!(
        native_calls
            .lock()
            .await
            .iter()
            .filter(|r| r["op"] == "close_page")
            .count(),
        native_before + 1,
        "native close is not repeated"
    );
    assert_eq!(
        exit.calls
            .lock()
            .await
            .iter()
            .filter(|r| r["op"] == "close_stream")
            .count(),
        exit_before + 1,
        "Exit close is not repeated"
    );
    let summary = request("GET", "/api/apps/browser/summary".into(), Value::Null).await;
    for count in [
        "total_sessions",
        "active_sessions",
        "engine_cleanup_obligations",
        "launch_reconciliation_obligations",
    ] {
        assert_eq!(summary["sessions"][count], 0, "{summary}");
    }
    assert_eq!(
        summary["sessions"]["fresh_start_allowed"], true,
        "terminal retirement returns scoped capacity"
    );
    assert!(gateway_browser_remote::consumer_binding(
        consumer_root,
        &json!({"principal_id":principal,"page_id":page})
    )
    .unwrap()
    .is_none());
    assert!(!socket.exists());
    drop(
        std::net::TcpListener::bind(address)
            .expect("ingress remains closed after retirement retry"),
    );
}

pub(super) async fn exercise_signed_remote_engine_carrier(
    registry: &ProviderRegistry,
    grant: &Value,
    engine_root: &std::path::Path,
) {
    config(engine_root);
    let generation = format!("sha256:{}", "e".repeat(64));
    let page_id = format!(
        "page:vz-{}",
        hex::encode(Sha256::digest(format!("{generation}\npage")))
    );
    let intent = json!({"lifecycle_generation":generation,"page_id":page_id,
        "vm_id":format!("browser-vm-{}",hex::encode(Sha256::digest(format!("{generation}\nvm")))),
        "profile_key":elastos_common::browser_profile_key_from_value(grant["principal_id"].as_str().unwrap()),
        "adapter_id":"mock-browser-engine","url":"https://example.com/","target":"tls://example.com:443",
        "stream_id":"stream:carrier-execution","viewport":{"width":1280,"height":720},
        "display_mode":"webrtc_remote_display","guarantee_level":"mechanism_microvm"});
    let call = |op, request| crate::carrier::call_browser_engine(registry, grant, op, request);
    let prepared = call("prepare_launch", intent.clone()).await.unwrap();
    assert_eq!(prepared["data"]["page_id"], page_id);
    assert_eq!(call("prepare_launch", intent).await.unwrap(), prepared);
    let base = json!({"page_id":page_id,"lifecycle_generation":generation});
    let launched = call("launch", base.clone()).await.unwrap();
    assert_eq!(launched["status"], "ok", "{launched}");
    assert_eq!(call("launch", base.clone()).await.unwrap(), launched);
    for channel in ["video", "audio"] {
        for signal in [
            json!({"schema":"elastos.browser.webrtc-answer/v1","type":"answer","sdp":"v=0\r\n"}),
            json!({"schema":"elastos.browser.webrtc-candidate/v1","type":"candidate","candidate":{"candidate":"candidate:1 1 udp 1 127.0.0.1 50000 typ relay"}}),
        ] {
            let mut request = base.clone();
            request["channel"] = json!(channel);
            request["signal"] = signal;
            let response = call("webrtc_signal", request).await.unwrap();
            assert_eq!(response["data"]["page_id"], page_id);
        }
    }
    let mut input = base.clone();
    input["event"] = json!({"type":"paste_text","text":"Carrier owner"});
    assert_eq!(
        call("input", input).await.unwrap()["data"]["accepted"],
        true
    );
    let mut close = base.clone();
    close["runtime_cleanup"] = launched["data"]["runtime_cleanup"].clone();
    let terminal = call("close_page", close.clone()).await.unwrap();
    assert_eq!(terminal["data"]["effects"].as_object().unwrap().len(), 13);
    assert!(terminal["data"]["effects"]
        .as_object()
        .unwrap()
        .values()
        .all(|value| value == true));
    assert_eq!(call("close_page", close).await.unwrap(), terminal);
    assert!(call("launch", base).await.is_err());
    // A second reservation is canceled before native launch and can only replay
    // its exact terminal receipt. It cannot consume or close the first owner.
    let generation = format!("sha256:{}", "f".repeat(64));
    let mut canceled = json!({"lifecycle_generation":generation,
        "page_id":format!("page:vz-{}",hex::encode(Sha256::digest(format!("{generation}\npage")))),
        "vm_id":format!("browser-vm-{}",hex::encode(Sha256::digest(format!("{generation}\nvm")))),
        "profile_key":elastos_common::browser_profile_key_from_value(grant["principal_id"].as_str().unwrap()),
        "adapter_id":"mock-browser-engine","url":"https://example.com/","target":"tls://example.com:443",
        "stream_id":"stream:carrier-cancel","viewport":{"width":1280,"height":720},
        "display_mode":"webrtc_remote_display","guarantee_level":"mechanism_microvm"});
    call("prepare_launch", canceled.clone()).await.unwrap();
    canceled = json!({"page_id":canceled["page_id"],"lifecycle_generation":generation});
    let receipt = call("close_page", canceled.clone()).await.unwrap();
    assert_eq!(
        receipt["data"]["schema"],
        "elastos.browser.remote-engine-preparation-closed/v1"
    );
    assert_eq!(receipt["data"]["native_dispatch"], false);
    assert_eq!(call("close_page", canceled.clone()).await.unwrap(), receipt);
    assert!(call("launch", canceled).await.is_err());
}

struct FailFirstPreparationCancel {
    inner: crate::carrier::CarrierProviderInvoker,
    remaining: std::sync::atomic::AtomicBool,
}
#[async_trait::async_trait]
impl ProviderCarrierInvoker for FailFirstPreparationCancel {
    async fn invoke_carrier_provider(
        &self,
        route: &ProviderCarrierRoute,
        invocation: &ProviderInvocation,
        request: Value,
    ) -> Result<Value, ProviderError> {
        if request["cancel_preparation"] == true
            && self
                .remaining
                .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(ProviderError::Provider(
                "test lost cancellation request".into(),
            ));
        }
        self.inner
            .invoke_carrier_provider(route, invocation, request)
            .await
    }
}

// Runs after a real signed approval has been denied on B. Readiness was proven
// earlier in the same Services test; cancellation still uses authenticated Carrier.
pub(super) async fn exercise_revoked_remote_preparation_cleanup(
    gateway: &GatewayState,
    grant: &Value,
    exit: &ConsumerExitFixture,
    prepare_admitted: bool,
    engine_root: &std::path::Path,
) {
    let endpoint = gateway.carrier_endpoint.as_ref().unwrap().clone();
    gateway
        .provider_registry
        .as_ref()
        .unwrap()
        .set_carrier_invoker(Arc::new(FailFirstPreparationCancel {
            inner: crate::carrier::CarrierProviderInvoker::with_carrier_endpoint(endpoint.clone()),
            remaining: std::sync::atomic::AtomicBool::new(true),
        }))
        .await;
    let principal = grant["principal_id"].as_str().unwrap();
    let reservation = reserve_browser_launch(
        &gateway.data_dir,
        principal,
        BrowserLaunchLifecycle {
            owner_launch_id: "launch:revoked-prepare".into(),
            browser_instance: Some("browser:123456781234123412341234567890ab".into()),
            url: "https://example.com/".into(),
            exit_id: "local-runtime".into(),
            engine_route_provider: "remote-engine-test".into(),
            selected_engine_adapter: Some("mock-browser-engine".into()),
            service_selection: None,
            profile_key_hash: None,
            vm_key_hash: None,
        },
    )
    .await
    .unwrap();
    let stream = exit
        .send_raw(
            &json!({"op":"open_stream","principal_id":principal,"target":"tls://example.com:443"}),
        )
        .await
        .unwrap()["data"]
        .clone();
    let stream = browser_attach_runtime_stream_path(
        &gateway.data_dir,
        stream,
        gateway.carrier_endpoint.as_ref(),
    )
    .await
    .unwrap();
    let socket = browser_runtime_stream_socket_path(&gateway.data_dir, &exit.stream_id).unwrap();
    assert!(socket.exists());
    let before = exit
        .calls
        .lock()
        .await
        .iter()
        .filter(|r| r["op"] == "close_stream")
        .count();
    let choice = (
        grant.clone(),
        "mock-browser-engine".into(),
        "remote-engine-test".into(),
    );
    let profile = json!({"profile_key":elastos_common::browser_profile_key_from_value(principal)});
    let input = json!({"owner_launch_id":"launch:revoked-prepare","browser_instance":"browser:123456781234123412341234567890ab",
        "url":"https://example.com/","viewport":{"width":1280,"height":720},
        "display_mode":"webrtc_remote_display","guarantee_level":"mechanism_microvm"});
    let preparation = gateway_browser_remote::prepare_consumer(
        gateway,
        &choice,
        &reservation,
        principal,
        &profile,
        &stream,
        &input,
    )
    .await;
    assert_eq!(
        preparation.is_ok(),
        prepare_admitted,
        "{}",
        preparation.err().map(|e| e.to_string()).unwrap_or_default()
    );
    assert!(
        gateway_browser_remote::cancel_consumer_preparation(gateway, &reservation)
            .await
            .is_err(),
        "the first lost cancellation retains its durable cleanup obligation"
    );
    assert!(socket.exists());
    assert!(gateway_browser_remote::consumer_binding(
        &gateway.data_dir,
        &json!({"principal_id":principal,"page_id":reservation.page_id()})
    )
    .unwrap()
    .is_some());
    assert_eq!(
        exit.calls
            .lock()
            .await
            .iter()
            .filter(|r| r["op"] == "close_stream")
            .count(),
        before
    );
    if prepare_admitted {
        let path = gateway
            .data_dir
            .join("Runtime/BrowserLifecycle/remote-engine")
            .join(format!(
                "{}.json",
                hex::encode(Sha256::digest(reservation.page_id()))
            ));
        set_browser_durable_delete_failure(&path, true);
        let settled = gateway_browser_remote::retry_consumer_preparations(gateway).await;
        let retained: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        set_browser_durable_delete_failure(&path, false);
        assert!(!settled);
        assert_eq!(retained["terminal_retirement"], true);
        // The direct cancellation retry shares the terminal-retirement path.
        assert!(
            gateway_browser_remote::cancel_consumer_preparation(gateway, &reservation)
                .await
                .unwrap()
        );
    } else {
        assert!(gateway_browser_remote::retry_consumer_preparations(gateway).await);
    }
    assert!(
        !gateway_browser_remote::retry_consumer_preparations(gateway).await,
        "terminal cleanup is retired"
    );
    assert!(!socket.exists());
    assert!(gateway_browser_remote::consumer_binding(
        &gateway.data_dir,
        &json!({"principal_id":principal,"page_id":reservation.page_id()})
    )
    .unwrap()
    .is_none());
    let status = browser_gateway_session_status(
        &gateway.data_dir,
        principal,
        Some("launch:revoked-prepare"),
        Some("browser:123456781234123412341234567890ab"),
    )
    .await;
    assert_eq!(status["total_sessions"], 0);
    assert_eq!(status["engine_cleanup_obligations"], 0);
    assert_eq!(status["launch_reconciliation_obligations"], 0);
    assert_eq!(
        exit.calls
            .lock()
            .await
            .iter()
            .filter(|r| r["op"] == "close_stream")
            .count(),
        before + 1
    );
    let serving_principal = format!(
        "remote-engine-{}",
        hex::encode(Sha256::digest(format!("{}\n{principal}", endpoint.id())))
    );
    let serving = browser_gateway_session_status(engine_root, &serving_principal, None, None).await;
    assert_eq!(serving["total_sessions"], 0, "{serving}");
    assert_eq!(serving["launch_reconciliation_obligations"], 0, "{serving}");
    assert_eq!(serving["engine_cleanup_obligations"], 0, "{serving}");
    assert!(
        !browser_runtime_stream_socket_path(engine_root, &exit.stream_id)
            .unwrap()
            .exists()
    );
    gateway
        .provider_registry
        .as_ref()
        .unwrap()
        .set_carrier_invoker(Arc::new(
            crate::carrier::CarrierProviderInvoker::with_carrier_endpoint(endpoint),
        ))
        .await;
}
