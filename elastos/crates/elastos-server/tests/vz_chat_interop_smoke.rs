//! Phase 5 Day 3 — Mac-only synthetic chat↔WASM interop contract.
//!
//! Lock-in goal: the bidirectional `ProviderRegistry::send_raw`
//! contract that the shell-level
//! `scripts/chat-wasm-native-interop-smoke.sh` depends on stays
//! correct under the Phase 4 Day 3 cross-VM dispatch graph. If
//! either direction (native → WASM or WASM → native) regresses
//! at the contract layer, this test surfaces it BEFORE the
//! shell smoke reaches it through `curl install.sh | bash`.
//!
//! No real Vz VMs are launched. Two synthetic `Provider`
//! implementations share an in-memory bus; round-trip messages
//! within a 5 s wall-clock budget validate the dispatch path
//! `ProviderRegistry::send_raw` →
//! `Provider::send_raw` → bus write → other side's read.
//! Phase 5 Day 5 runs the same contract against real Vz VMs
//! under concurrent load; Day 3 is the contract-stability
//! tripwire that runs in <50 ms on any host.
//!
//! Why Mac-only despite no Vz call. The contract is the
//! cross-VM RPC plumbing that ONLY matters when the substrate
//! is Vz (Mac). On Linux the equivalent flow runs through
//! crosvm and is already covered by the existing Linux smoke
//! suite. Gating to macOS keeps the Phase-5 work focused on
//! the substrate the project is delivering this phase.
//!
//! Anchored in: `docs/vz-backend/PHASE_5_DAY_3_NOTES.md` and
//! the Day-3 block of `docs/vz-backend/PHASE_5_PLAN.md` L60-L75.

#![cfg(target_os = "macos")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use elastos_runtime::provider::{
    Provider, ProviderError, ProviderRegistry, ResourceRequest, ResourceResponse,
};

/// Wall-clock upper bound. Same shape as the Day-2
/// `LAUNCH_BUDGET` — exceeding this is a contract-stability
/// regression, not a flake.
const ROUND_TRIP_BUDGET: Duration = Duration::from_secs(5);

/// Shared in-memory "carrier bus" between the synthetic
/// native-chat and WASM-chat providers. Models the
/// peer-to-peer message-passing that the real chat capsules
/// would do through the Carrier bridge; this Phase-5-Day-3
/// test asserts the contract WITHOUT requiring the Carrier
/// daemon, the cross-VM socketpair, or any Apple framework.
#[derive(Default)]
struct ChatBus {
    /// Messages sent BY the native side, to be read by WASM.
    native_to_wasm: Vec<String>,
    /// Messages sent BY the WASM side, to be read by native.
    wasm_to_native: Vec<String>,
}

/// Synthetic provider modelling the native chat capsule.
///
/// `send_raw({"op":"send","text":T})` → appends T to
/// `native_to_wasm`, returns `{"status":"ok"}`.
///
/// `send_raw({"op":"recv"})` → drains `wasm_to_native`, returns
/// `{"status":"ok","messages":[...]}`.
struct NativeChatProvider {
    bus: Arc<Mutex<ChatBus>>,
}

#[async_trait]
impl Provider for NativeChatProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        // The chat smoke contract only exercises `send_raw`,
        // not the hierarchical resource path. Returning a
        // typed error here is the honest signal that the
        // request used the wrong API surface.
        Err(ProviderError::Provider(
            "NativeChatProvider does not implement resource-path handle".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["chat-native"]
    }

    fn name(&self) -> &'static str {
        "synthetic-native-chat"
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        let op = request
            .get("op")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::Provider("missing op".into()))?;
        let mut bus = self.bus.lock().await;
        match op {
            "send" => {
                let text = request
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProviderError::Provider("missing text".into()))?;
                bus.native_to_wasm.push(text.to_string());
                Ok(json!({ "status": "ok" }))
            }
            "recv" => {
                let drained: Vec<Value> = bus.wasm_to_native.drain(..).map(Value::String).collect();
                Ok(json!({ "status": "ok", "messages": drained }))
            }
            other => Err(ProviderError::Provider(format!("unknown op: {other}"))),
        }
    }
}

/// Synthetic provider modelling the WASM chat capsule.
///
/// `send_raw({"op":"send","text":T})` → appends T to
/// `wasm_to_native`, returns `{"status":"ok"}`.
///
/// `send_raw({"op":"recv"})` → drains `native_to_wasm`, returns
/// `{"status":"ok","messages":[...]}`.
struct WasmChatProvider {
    bus: Arc<Mutex<ChatBus>>,
}

#[async_trait]
impl Provider for WasmChatProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "WasmChatProvider does not implement resource-path handle".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["chat-wasm"]
    }

    fn name(&self) -> &'static str {
        "synthetic-wasm-chat"
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        let op = request
            .get("op")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::Provider("missing op".into()))?;
        let mut bus = self.bus.lock().await;
        match op {
            "send" => {
                let text = request
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ProviderError::Provider("missing text".into()))?;
                bus.wasm_to_native.push(text.to_string());
                Ok(json!({ "status": "ok" }))
            }
            "recv" => {
                let drained: Vec<Value> = bus.native_to_wasm.drain(..).map(Value::String).collect();
                Ok(json!({ "status": "ok", "messages": drained }))
            }
            other => Err(ProviderError::Provider(format!("unknown op: {other}"))),
        }
    }
}

/// Extract the first `messages[]` entry as a String. Returns
/// `None` if the response shape is unexpected, the messages
/// list is empty, or the entry isn't a JSON string.
fn first_message(response: &Value) -> Option<String> {
    response
        .get("messages")?
        .as_array()?
        .first()?
        .as_str()
        .map(str::to_string)
}

/// Phase 5 Day 3 — bidirectional chat-interop contract
/// against the `ProviderRegistry::send_raw` dispatch path.
///
/// Sequence:
///   1. Register `NativeChatProvider` (scheme `chat-native`)
///      and `WasmChatProvider` (scheme `chat-wasm`) sharing a
///      `ChatBus`.
///   2. Native → WASM: `send_raw("chat-native", {"op":"send","text":"hello-from-native"})`
///      then `send_raw("chat-wasm", {"op":"recv"})` reads
///      `hello-from-native`.
///   3. WASM → native: `send_raw("chat-wasm", {"op":"send","text":"hello-from-wasm"})`
///      then `send_raw("chat-native", {"op":"recv"})` reads
///      `hello-from-wasm`.
///   4. Both directions complete within `ROUND_TRIP_BUDGET`
///      (5 s) — exceeding is a contract regression, not a
///      flake.
///
/// This is the Phase-5 contract guard for the shell-level
/// `scripts/chat-wasm-native-interop-smoke.sh`. If this test
/// passes, the dispatch graph the shell smoke depends on
/// works at the API layer. If the shell smoke fails on Mac
/// post-Phase-6 while this test passes, the bug is in the
/// substrate (install.sh, Vz boot path, or Carrier bridge) —
/// NOT in the cross-VM RPC plumbing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_native_and_wasm_round_trip_via_provider_registry() {
    let bus = Arc::new(Mutex::new(ChatBus::default()));
    let registry = ProviderRegistry::new();

    let native_provider = Arc::new(NativeChatProvider {
        bus: Arc::clone(&bus),
    });
    let wasm_provider = Arc::new(WasmChatProvider {
        bus: Arc::clone(&bus),
    });
    registry.register(native_provider.clone()).await;
    registry.register(wasm_provider.clone()).await;

    let started = Instant::now();

    // Native → WASM direction.
    let send_native = registry
        .send_raw(
            "chat-native",
            &json!({"op":"send","text":"hello-from-native"}),
        )
        .await
        .expect("native send_raw must succeed");
    assert_eq!(
        send_native.get("status").and_then(|v| v.as_str()),
        Some("ok"),
        "native send_raw response missing status:ok: {send_native}"
    );

    let recv_wasm = registry
        .send_raw("chat-wasm", &json!({"op":"recv"}))
        .await
        .expect("wasm recv send_raw must succeed");
    assert_eq!(
        first_message(&recv_wasm).as_deref(),
        Some("hello-from-native"),
        "wasm side did not see 'hello-from-native': {recv_wasm}"
    );

    // WASM → native direction.
    let send_wasm = registry
        .send_raw("chat-wasm", &json!({"op":"send","text":"hello-from-wasm"}))
        .await
        .expect("wasm send_raw must succeed");
    assert_eq!(
        send_wasm.get("status").and_then(|v| v.as_str()),
        Some("ok"),
        "wasm send_raw response missing status:ok: {send_wasm}"
    );

    let recv_native = registry
        .send_raw("chat-native", &json!({"op":"recv"}))
        .await
        .expect("native recv send_raw must succeed");
    assert_eq!(
        first_message(&recv_native).as_deref(),
        Some("hello-from-wasm"),
        "native side did not see 'hello-from-wasm': {recv_native}"
    );

    let elapsed = started.elapsed();
    assert!(
        elapsed < ROUND_TRIP_BUDGET,
        "round-trip exceeded {ROUND_TRIP_BUDGET:?}: took {elapsed:?}"
    );

    // Drained-bus invariant: both directions should be empty
    // after `recv` calls drained them. A non-empty bus would
    // mean drain semantics regressed (e.g. recv started
    // copying instead of moving), which would silently
    // duplicate messages on the shell smoke's retry loop.
    let bus_state = bus.lock().await;
    assert!(
        bus_state.native_to_wasm.is_empty() && bus_state.wasm_to_native.is_empty(),
        "bus state must be drained after round-trip; got native_to_wasm={:?}, wasm_to_native={:?}",
        bus_state.native_to_wasm,
        bus_state.wasm_to_native
    );
}

/// Phase 5 Day 3 — unknown-scheme `send_raw` calls surface a
/// typed `NoProvider` error. Locks in the failure mode the
/// shell smoke would otherwise see as a cryptic timeout if
/// the registry silently swallowed unknown schemes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_scheme_send_raw_returns_no_provider_error() {
    let registry = ProviderRegistry::new();

    let result = registry
        .send_raw(
            "definitely-not-registered",
            &json!({"op":"send","text":"x"}),
        )
        .await;

    let err = result.expect_err("unknown scheme must return Err");
    let rendered = format!("{err}");
    assert!(
        rendered.contains("no provider for scheme"),
        "unknown scheme must surface NoProvider error: got '{rendered}'"
    );
    assert!(
        rendered.contains("definitely-not-registered"),
        "error message must name the failing scheme: got '{rendered}'"
    );
}

/// Phase 5 Day 3 — `send_raw` errors from the registered
/// provider propagate as typed `ProviderError` up to the
/// caller (the shell smoke's PTY-controlled chat process).
/// The Phase 4 Day 3 cross-VM dispatch path established this
/// contract; Day 3 locks it in at the API layer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_send_raw_error_propagates_up_through_registry() {
    let bus = Arc::new(Mutex::new(ChatBus::default()));
    let registry = ProviderRegistry::new();
    let native = Arc::new(NativeChatProvider {
        bus: Arc::clone(&bus),
    });
    registry.register(native).await;

    // Send a malformed request — no `op` field.
    let result = registry.send_raw("chat-native", &json!({})).await;
    let err = result.expect_err("malformed request must return Err");
    let rendered = format!("{err}");
    assert!(
        rendered.contains("missing op"),
        "malformed request must surface the provider's error verbatim: got '{rendered}'"
    );
}
