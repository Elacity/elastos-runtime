#!/usr/bin/env node

import { readFileSync } from "node:fs";

const repoRoot = new URL("../", import.meta.url);

function read(path) {
  return readFileSync(new URL(path, repoRoot), "utf8");
}

function readAll(paths) {
  return paths.map((path) => read(path)).join("\n");
}

function assert(condition, message, details = undefined) {
  if (!condition) {
    const suffix = details ? `\n${JSON.stringify(details, null, 2)}` : "";
    throw new Error(`${message}${suffix}`);
  }
}

function sourceBlock(source, needle, label) {
  const start = source.indexOf(needle);
  assert(start >= 0, `${label} must exist`);
  const open = source.indexOf("{", start);
  assert(open >= 0, `${label} must have a body`);
  let depth = 0;
  for (let index = open; index < source.length; index += 1) {
    const char = source[index];
    if (char === "{") {
      depth += 1;
    } else if (char === "}") {
      depth -= 1;
      if (depth === 0) {
        return source.slice(start, index + 1);
      }
    }
  }
  throw new Error(`${label} body is not balanced`);
}

function assertNoForbidden(source, label, forbidden) {
  const hits = forbidden.filter((needle) => source.includes(needle));
  assert(
    hits.length === 0,
    `${label} contains forbidden Browser entropy`,
    hits,
  );
}

const browserManifest = read("capsules/browser/capsule.json");
const browser = read("capsules/browser/index.html");
const browserJs = readAll([
  "capsules/browser/browser.js",
  "capsules/browser/browser-clipboard.js",
  "capsules/browser/browser-history.js",
  "capsules/browser/browser-input.js",
  "capsules/browser/browser-input-surface.js",
  "capsules/browser/browser-location.js",
  "capsules/browser/browser-remote-display.js",
  "capsules/browser/browser-runtime-api.js",
  "capsules/browser/browser-status.js",
  "capsules/browser/browser-webrtc.js",
]);
const browserStyle = read("capsules/browser/style.css");
const homeShellWindows = read("capsules/home/browser/shell-windows.js");
const netProvider = read("capsules/net-provider/src/main.rs");
const exitProvider = read("capsules/exit-provider/src/main.rs");
const browserEngineAdapter = readAll([
  "capsules/browser-engine-adapter/src/main.rs",
  "capsules/browser-engine-adapter/src/display.rs",
  "capsules/browser-engine-adapter/src/ids.rs",
  "capsules/browser-engine-adapter/src/supervisor.rs",
  "capsules/browser-engine-adapter/src/validation.rs",
  "capsules/browser-engine-adapter/src/tests.rs",
]);
const browserEngineSupervisor = read(
  "elastos/tools/browser-engine-supervisor/src/main.rs",
);
const browserNativeOperatorConfig = read(
  "scripts/browser-native-operator-config.mjs",
);
const browserDisplayModeSmoke = read("scripts/browser-display-mode-smoke.mjs");
const browserObjectiveAudit = read("scripts/browser-objective-audit.mjs");
const browserProviderDecisionReport = read(
  "scripts/browser-provider-decision-report.mjs",
);
const browserHostedProviderBakeoff = read(
  "scripts/browser-hosted-provider-bakeoff.sh",
);
const browserNativeTargetPreflight = read(
  "scripts/browser-native-target-preflight.sh",
);
const browserSelkiesControlService = read(
  "scripts/browser-selkies-control-service.mjs",
);
const browserSelkiesRuntimeExitTarget = read(
  "scripts/browser-selkies-runtime-exit-target.sh",
);
const browserPerLaunchSelkiesSupervisor = read(
  "scripts/browser-per-launch-selkies-supervisor.mjs",
);
const browserPerLaunchSelkiesSupervisorSmoke = read(
  "scripts/browser-per-launch-selkies-supervisor-smoke.sh",
);
const browserSessionCapacitySmoke = read(
  "scripts/browser-session-capacity-smoke.sh",
);
const browserSelkiesServiceWrapper = read(
  "scripts/system/elastos-browser-selkies.sh",
);
const browserSelkiesServiceEnv = read(
  "scripts/system/elastos-browser-selkies.env.example",
);
const browserHostedProductWalletSmoke = read(
  "scripts/browser-hosted-product-wallet-smoke.sh",
);
const browserHostedProductNavigationSmoke = read(
  "scripts/browser-hosted-product-navigation-smoke.mjs",
);
const browserCapsuleDoc = read("docs/BROWSER_CAPSULE.md");
const browserBakeoffDoc = read("docs/BROWSER_PROVIDER_BAKEOFF.md");
const roadmap = read("ROADMAP.md");
const tasks = read("TASKS.md");
const state = read("state.md");
const components = read("components.json");
const gatewayBrowserApi = readAll([
  "elastos/crates/elastos-server/src/api/gateway_browser.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_engine.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_response.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_stream.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_validation.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_wallet.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_wallet_bridge.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_wallet_reads.rs",
]);
const gatewayApi = readAll([
  "elastos/crates/elastos-server/src/api/gateway.rs",
  "elastos/crates/elastos-server/src/api/gateway_home_runtime.rs",
  "elastos/crates/elastos-server/src/api/gateway_home_token.rs",
  "elastos/crates/elastos-server/src/api/gateway_models.rs",
  "elastos/crates/elastos-server/src/api/gateway_provider_proxy.rs",
  "elastos/crates/elastos-server/src/api/gateway_server.rs",
]);
const shellWindows = read("capsules/home/browser/shell-windows.js");

assert(
  browserManifest.includes('"name": "browser"') &&
    browserManifest.includes('"elastos://wallet/*"') &&
    browserManifest.includes('"elastos://net/stream"') &&
    !browserManifest.includes("guest_network") &&
    !browserManifest.includes('"provides"'),
  "Browser capsule manifest must declare wallet/net intent without provider or guest-network authority",
);

assert(
  browser.includes("https://ela.city/") &&
    browser.includes('id="browser-back"') &&
    browser.includes('id="browser-forward"') &&
    browser.includes('id="browser-refresh"') &&
    browser.includes('id="browser-url"') &&
    !browser.includes("Runtime boundary") &&
    !browser.includes("Last request") &&
    !browser.includes('id="browser-frame"') &&
    !browser.includes("Open outside ElastOS"),
  "Browser capsule must keep compact browser chrome without proof panels, host iframe browsing, or external escape hatches",
);

assertNoForbidden(browserJs, "Browser UI", [
  "/api/provider/net/stream",
  "/api/provider/net/http",
  "frame.src",
  "window.open",
  "window.ethereum",
  "eth_requestAccounts",
  "personal_sign",
  "Remote video path is unavailable",
  "Runtime frame preview",
  "showing Runtime frame",
]);

assert(
  browserJs.includes("normalizeUrl") &&
    browserJs.includes("streamTargetForUrl") &&
    browserJs.includes("browserInstanceId") &&
    browserJs.includes(
      "elastos.browser.current_page_id:${browserInstanceId}",
    ) &&
    browserJs.includes("Only http and https addresses") &&
    browserJs.includes("/api/apps/browser/open") &&
    browserJs.includes("elastos.browser.open-result/v1") &&
    browserJs.includes("Browser failed closed") &&
    browserJs.includes("Blocked by Browser Exit policy") &&
    browserJs.includes("historyEntries"),
  "Browser UI must use the high-level Browser open route and fail closed instead of direct provider routes",
);

assert(
  browserJs.includes("isMissingRuntimePageError") &&
    browserJs.includes("scheduleRemoteReconnect") &&
    browserJs.includes("Browser Runtime page heartbeat was lost.") &&
    browserJs.includes("Remote display reconnected through Runtime.") &&
    browserJs.includes("track.addEventListener(\"mute\"") &&
    browserJs.includes("track.addEventListener(\"ended\"") &&
    !browserJs.includes("Browser session ended. Open the address again") &&
    !browserJs.includes("Reopen the page to start a clean Runtime session") &&
    browserSelkiesControlService.includes("crypto.randomBytes(8)") &&
    !browserSelkiesControlService.includes("update(`${url}\\0${streamId}`)"),
  "Browser sessions must use launch-unique provider page ids and reconnect missing pages instead of reusing stale deterministic ids or requiring manual reopen",
);

assert(
  browserStyle.includes(".browser-stage") &&
    browserStyle.includes("@media (max-width: 640px)") &&
    browserStyle.includes("--accent: #d46f24") &&
    browserStyle.includes("overflow: hidden") &&
    browserStyle.includes("height: 100%") &&
    browserStyle.includes("min-height: 0") &&
    !browserStyle.includes(".browser-hero") &&
    !browserStyle.includes(".browser-card"),
  "Browser UI must stay compact and responsive without old proof/debug card chrome",
);

assert(
  netProvider.includes("exit_unavailable") &&
    netProvider.includes("private_network_blocked") &&
    netProvider.includes("direct host networking") &&
    netProvider.includes("deny_unknown_fields") &&
    read("capsules/net-provider/capsule.json").includes(
      '"provides": "elastos://net/*"',
    ),
  "Net provider must be a fail-closed Browser/Net boundary, not raw host networking",
);

assert(
  exitProvider.includes("exit_policy_blocked") &&
    exitProvider.includes("private_network_blocked") &&
    exitProvider.includes("direct host networking") &&
    exitProvider.includes("deny_unknown_fields") &&
    exitProvider.includes("allowed_hosts") &&
    exitProvider.includes("max_body_bytes") &&
    exitProvider.includes("elastos.exit.http-fetch.result/v1") &&
    exitProvider.includes("elastos.exit.stream-session/v1") &&
    exitProvider.includes("elastos.adapter-ipc/v1") &&
    exitProvider.includes("elastos.exit.relay-ipc/v1") &&
    !exitProvider.includes("runtime_stream_path"),
  "Exit provider must expose typed HTTP/stream exits without raw host networking or public Runtime stream-path authority",
);

assert(
  browserEngineAdapter.includes("elastos.browser.engine.page/v1") &&
    browserEngineAdapter.includes("elastos.adapter-ipc/v1") &&
    browserEngineAdapter.includes("runtime_stream_path") &&
    browserEngineAdapter.includes("elastos.browser.engine.launch-request/v1") &&
    browserEngineAdapter.includes(
      "elastos.browser.engine.supervisor-result/v1",
    ) &&
    browserEngineAdapter.includes("byte_transport_unavailable") &&
    browserEngineAdapter.includes("engine_process_unavailable") &&
    browserEngineAdapter.includes("validate_supervisor_result") &&
    browserEngineAdapter.includes("display_modes") &&
    browserEngineAdapter.includes("webrtc_signal") &&
    browserEngineAdapter.includes("direct_network") &&
    browserEngineAdapter.includes("wallet_injection"),
  "Browser Engine Adapter must be an explicit fail-closed adapter contract, not host browser authority",
);

assert(
  browserEngineAdapter.includes(
    "webrtc_remote_display audio requires a product compositor backend",
  ) &&
    browserEngineAdapter.includes(
      "webrtc_proof_surface_cannot_advertise_audio",
    ) &&
    browserEngineAdapter.includes(
      "webrtc_product_compositor_can_advertise_audio",
    ),
  "Browser Engine Adapter must reject proof-surface audio claims while allowing product compositor audio",
);

assert(
  browserEngineSupervisor.includes("display_capabilities: DisplayCapabilities") &&
    browserEngineSupervisor.includes("config.display_capabilities.audio") &&
    browserNativeOperatorConfig.includes("nativeAudio: false") &&
    browserNativeOperatorConfig.includes("nativeVideo: false") &&
    browserNativeOperatorConfig.includes("--native-audio") &&
    browserNativeOperatorConfig.includes("--native-video") &&
    browserNativeTargetPreflight.includes(
      "--require-native-media requires both --native-audio and --native-video",
    ) &&
    browserNativeTargetPreflight.includes(
      "native media readiness requires the target proof to report native_audio_proven=true and native_video_proven=true",
    ),
  "Native Browser namespace/proxy smokes must not pretend fake browser processes prove native audio or video",
);

assert(
  browserJs.includes(
    "Diagnostic Browser display mode requires debug=1 or metrics=1.",
  ) &&
    browserJs.includes(
      'if (value === "diagnostic" || value === "diagnostic_frame")',
    ) &&
    !browserJs.includes(
      '["webrtc_remote_display", "native_surface", "diagnostic_frame"].includes(value)',
    ) &&
    browserDisplayModeSmoke.includes("diagnostic_requires_debug") &&
    browserCapsuleDoc.includes(
      "`diagnostic_frame` is accepted only when Browser is opened with explicit",
    ),
  "Browser diagnostic display mode must remain debug-only and never become a normal product fallback",
);

assert(
  browserJs.includes("const expectsAudio = displaySession.audio === true") &&
    browserJs.includes("prepareAudio(expectsAudio)") &&
    browserJs.includes("unlockRemoteAudioFromGesture") &&
    browserJs.includes('nextPeerConnection.addTransceiver("audio"') &&
    browserJs.includes('event?.type === "resize"') &&
    browserJs.includes('event?.type === "paste_text"') &&
    browserJs.includes("Remote audio enabled.") &&
    browserDisplayModeSmoke.includes("audio_invariants_checked"),
  "Browser UI must keep WebRTC audio explicit, user-gesture unlocked, resize protocol-aware, and covered by display-mode smoke",
);

assert(
  gatewayBrowserApi.includes("authority_false_proof_missing") &&
    gatewayBrowserApi.includes("invalid_provider_summary") &&
    gatewayBrowserApi.includes("invalid_provider_status") &&
    gatewayBrowserApi.includes(
      "Browser Engine Adapter status omitted direct_network=false proof",
    ) &&
    gatewayBrowserApi.includes(
      "Runtime Net provider status omitted direct_network=false proof",
    ) &&
    gatewayBrowserApi.includes(
      "Browser Exit provider status omitted direct_network=false proof",
    ),
  "Browser summaries must not default missing authority proofs to safe-looking status",
);

assert(
  gatewayBrowserApi.includes("BrowserProviderResourceCall") &&
    gatewayBrowserApi.includes("BrowserOpenRequest") &&
    gatewayBrowserApi.includes("browser_app_open") &&
    gatewayBrowserApi.includes("browser.open.requested") &&
    gatewayBrowserApi.includes("browser.open.completed") &&
    gatewayBrowserApi.includes("browser.chain_read.requested") &&
    gatewayBrowserApi.includes("browser.chain_read.completed") &&
    gatewayBrowserApi.includes("runtime_net_exit_policy") &&
    gatewayBrowserApi.includes("standing_read_policy") &&
    gatewayBrowserApi.includes("create_browser_wallet_transaction_request") &&
    gatewayBrowserApi.includes("browser_engine_summary") &&
    gatewayBrowserApi.includes("browser_net_summary") &&
    !gatewayApi.includes("struct BrowserProviderResourceCall") &&
    !gatewayApi.includes("fn browser_app_open(") &&
    !gatewayApi.includes("fn create_browser_wallet_transaction_request(") &&
    !gatewayApi.includes("fn browser_engine_summary("),
  "Browser DTOs, provider-envelope helpers, and wallet bridge flows must stay out of the shared gateway module",
);

assert(
  gatewayBrowserApi.includes("browser_attach_runtime_stream_path") &&
    gatewayBrowserApi.includes("browser_stream_relay") &&
    gatewayBrowserApi.includes("elastos.exit.relay-open/v1") &&
    gatewayBrowserApi.includes("copy_bidirectional") &&
    gatewayBrowserApi.includes("spawn_browser_runtime_stream_listener") &&
    gatewayBrowserApi.includes("validate_browser_stream_receipt") &&
    gatewayBrowserApi.includes('object.remove("adapter_ipc")') &&
    gatewayBrowserApi.includes('object.remove("relay_ipc")'),
  "Browser open route must relay through private Runtime/Exit IPC and strip private descriptors from UI responses",
);

const openTargetBlock = sourceBlock(
  shellWindows,
  "export function openTarget",
  "Home openTarget",
);
assert(
  gatewayApi.includes('const BROWSER_CAPSULE_ID: &str = "browser"') &&
    gatewayApi.includes("fn is_home_visible_target") &&
    !sourceBlock(
      gatewayApi,
      "fn is_home_visible_target(name: &str)",
      "Home visible target filter",
    ).includes("BROWSER_CAPSULE_ID") &&
    shellWindows.includes("function iframeSandboxForLaunch(launched)") &&
    shellWindows.includes('launched?.target === "browser"') &&
    shellWindows.includes("function withBrowserInstanceQuery(options)") &&
    shellWindows.includes("browser_instance") &&
    !openTargetBlock.includes(
      'targetId === "browser" && browserWindowCount(targetId) > 0',
    ) &&
    shellWindows.includes("BROWSER_IFRAME_SANDBOX_EXTRAS"),
  "Home must open Browser as independent windows while keeping Browser networking Runtime/Exit mediated and iframe privileges scoped",
);

assert(
  browserSelkiesControlService.includes("readIceServersConfig") &&
    browserSelkiesControlService.includes(
      "ice_servers may contain at most 8 entries",
    ) &&
    browserSelkiesControlService.includes("display_session") &&
    browserSelkiesControlService.includes(
      "ice_servers: this.config.iceServers",
    ),
  "Hosted Browser WebRTC path must expose explicit operator STUN/TURN configuration through typed display sessions",
);

assert(
  browserSelkiesControlService.includes("eip6963:announceProvider") &&
    browserSelkiesControlService.includes("wallet_getPermissions") &&
    browserSelkiesControlService.includes("eth_coinbase") &&
    browserSelkiesControlService.includes("provider.providers = [provider]") &&
    browserSelkiesControlService.includes("provider.sendAsync") &&
    !browserSelkiesControlService.includes("preferredChainNamespaceForUrl") &&
    browserSelkiesControlService.includes("wallet_addEthereumChain") &&
    browserSelkiesControlService.includes(
      "No ElastOS Wallet EVM account is available for this Runtime principal",
    ) &&
    browserSelkiesControlService.includes(
      "default_chain_namespace: page.wallet?.default_chain_namespace || null",
    ) &&
    browserSelkiesControlService.includes(
      "No ElastOS Wallet account is available for eip155:",
    ) &&
    browserSelkiesControlService.includes("approval_required") &&
    browserSelkiesControlService.includes("personal_sign") &&
    browserSelkiesControlService.includes("eth_sendTransaction") &&
    browserSelkiesControlService.includes("runtimePost(state.approvalUrl") &&
    browserSelkiesControlService.includes(
      "if (status.transaction_hash) return status.transaction_hash",
    ) &&
    browserSelkiesControlService.includes(
      "runtimePost(state.transactionBroadcastUrl",
    ) &&
    browserSelkiesControlService.includes("waitForApproval") &&
    browserSelkiesControlService.includes(
      "__elastosBrowserNavigationPolicyInstalled",
    ) &&
    browserSelkiesControlService.includes("window.open = (url)") &&
    browserSelkiesControlService.includes(
      'event.target.closest("a[target]")',
    ) &&
    browserHostedProductNavigationSmoke.includes(
      "popup policy created hidden page target",
    ) &&
    browserHostedProductNavigationSmoke.includes(
      'window.open(url, "_blank")',
    ) &&
    !browserSelkiesControlService.includes("if (wallet.accounts.length > 0)") &&
    browserHostedProductWalletSmoke.includes("eip6963") &&
    browserHostedProductWalletSmoke.includes("wallet_getPermissions") &&
    browserHostedProductWalletSmoke.includes("approval_required") &&
    browserHostedProductWalletSmoke.includes("addedEscChain"),
  "Hosted Browser wallet bridge must be fail-present and expose modern injected-wallet discovery, Runtime approval routing, and permission compatibility without giving pages raw wallet or node authority",
);

assert(
  browserSelkiesRuntimeExitTarget.includes('selkies_encoder="x264enc"') &&
    browserSelkiesRuntimeExitTarget.includes('selkies_framerate="30"') &&
    browserSelkiesRuntimeExitTarget.includes('selkies_width="1920"') &&
    browserSelkiesRuntimeExitTarget.includes('selkies_height="1080"') &&
    browserSelkiesRuntimeExitTarget.includes(
      '\\"--force-device-scale-factor=1.5\\"',
    ) &&
    browserSelkiesRuntimeExitTarget.includes(
      "ELASTOS_SELKIES_INITIAL_RESOLUTION",
    ) &&
    browserSelkiesRuntimeExitTarget.includes(
      "needle = 'resize_display(' + quote + '1920x1080' + quote + ')'",
    ) &&
    browserSelkiesRuntimeExitTarget.includes(
      'selkies_resolution_mode="dynamic"',
    ) &&
    browserSelkiesRuntimeExitTarget.includes("--selkies-resolution-mode") &&
    browserSelkiesRuntimeExitTarget.includes(
      "--is-manual-resolution-mode=false",
    ) &&
    browserSelkiesRuntimeExitTarget.includes("--enable-resize=true") &&
    browserSelkiesRuntimeExitTarget.includes("--clipboard-enabled=true") &&
    browserSelkiesRuntimeExitTarget.includes(
      "browser-selkies-cargo-target",
    ) &&
    browserSelkiesServiceWrapper.includes(
      "ELASTOS_BROWSER_SELKIES_RESOLUTION_MODE:-dynamic",
    ) &&
    browserSelkiesServiceWrapper.includes(
      '--selkies-resolution-mode "$resolution_mode"',
    ) &&
    browserSelkiesServiceEnv.includes(
      "ELASTOS_BROWSER_SELKIES_RESOLUTION_MODE=dynamic",
    ) &&
    browserCapsuleDoc.includes(
      "1920x1080 stream with a stable 1280x720 CSS viewport",
    ),
  "Canonical hosted Browser launcher must default to tunable H.264 with an explicit normal-browser viewport scale and remote-resize gating, not fixed-compositor manual mode with zoomed-out CSS",
);

assert(
  browserPerLaunchSelkiesSupervisor.includes(
    'const BROWSER_PROGRAM_ENV = "ELASTOS_BROWSER_SELKIES_BROWSER_PROGRAM"',
  ) &&
    browserPerLaunchSelkiesSupervisor.includes("--browser-program") &&
    browserPerLaunchSelkiesSupervisor.includes("result.control_socket_path = controlSocket") &&
    browserPerLaunchSelkiesSupervisor.includes("result.isolated_session = true") &&
    browserPerLaunchSelkiesSupervisor.includes("killProcessGroup(target)") &&
    browserPerLaunchSelkiesSupervisorSmoke.includes(
      "ELASTOS_BROWSER_SERVICE_HOME",
    ) &&
    browserPerLaunchSelkiesSupervisorSmoke.includes(
      "stream:per-launch-smoke:a",
    ) &&
    browserPerLaunchSelkiesSupervisorSmoke.includes(
      "stream:per-launch-smoke:b",
    ) &&
    browserPerLaunchSelkiesSupervisorSmoke.includes(
      "a.control_socket_path !== b.control_socket_path",
    ) &&
    browserPerLaunchSelkiesSupervisorSmoke.includes(
      'CARGO_TARGET_DIR="$cargo_target_dir" cargo build',
    ) &&
    browserPerLaunchSelkiesSupervisorSmoke.includes(
      "POST\", a.control_socket_path, \"/shutdown\"",
    ),
  "Per-launch Browser supervisor must use explicit executable discovery, return page-scoped control sockets, prove two isolated sessions, and shut them down",
);

assert(
  browserJs.includes("clipboard_write") &&
    browserJs.includes("clipboard_read") &&
    browserJs.includes("paste_text") &&
    browserSelkiesControlService.includes("Input.insertText") &&
    browserSelkiesControlService.includes("pasteTextIntoBrowserPage") &&
    browserJs.includes("handleSelkiesClipboardMessage") &&
    browserJs.includes("pasteHostClipboardIntoRemote") &&
    browserJs.includes("copyRemoteClipboardToHost") &&
    browserJs.includes("focusRemoteInput") &&
    browserJs.includes("focusKeyboardCapture") &&
    browserJs.includes("handlePasteChord") &&
    browserJs.includes('event.getModifierState?.("Control")') &&
    browserJs.includes("hostModifierState.control") &&
    browser.includes("browser-keyboard-capture") &&
    browserStyle.includes(".browser-keyboard-capture") &&
    browserJs.includes("cw,") &&
    browserJs.includes('"cr"'),
  "Browser UI must bridge copy through Selkies clipboard messages and paste through a Runtime/provider CDP insertText command instead of simulated Ctrl+V",
);
assert(
  homeShellWindows.includes("function iframeAllowForLaunch") &&
    homeShellWindows.includes('launched?.target === "browser"') &&
    homeShellWindows.includes('"clipboard-read"') &&
    homeShellWindows.includes('"clipboard-write"') &&
    homeShellWindows.includes('allow="${iframeAllowForLaunch(launched)}"'),
  "Home Browser iframe must explicitly grant clipboard-read/write so render-surface paste can use the Runtime clipboard bridge",
);

assert(
  browserJs.includes("browser-status-copy") &&
    browserJs.includes("Copy Browser status message") &&
    browserJs.includes("navigator.clipboard.writeText(message)") &&
    browserStyle.includes('.browser-status[data-visible="true"][data-copyable="true"]') &&
    browserStyle.includes(".browser-status-copy") &&
    browser.includes("browser-20260524d"),
  "Browser sticky status/errors must be copyable so live product failures can produce actionable evidence",
);

assert(
  browserJs.includes('event?.type === "resize"') &&
    browserJs.includes('currentDisplayMode === "webrtc_remote_display"') &&
    browserJs.includes("lastViewport = viewport;") &&
    !browserJs.includes('event?.type === "resize" && event.viewport') &&
    browserSelkiesControlService.includes(
      "Emulation.setDeviceMetricsOverride",
    ) &&
    browserSelkiesControlService.includes(
      "deviceScaleFactor: config.displaySurface.deviceScaleFactor",
    ) &&
    browserSelkiesControlService.includes('body?.event?.type === "resize"'),
  "Browser viewport changes must never go through the Selkies pointer datachannel, and stable WebRTC must not send resize commands that freeze the current fixed-compositor stream",
);

assert(
  browserHostedProviderBakeoff.includes("--artifact-out") &&
    browserNativeTargetPreflight.includes("--artifact-out") &&
    browserObjectiveAudit.includes("manual UX evidence") &&
    browserObjectiveAudit.includes("audio_product_proven") &&
    browserObjectiveAudit.includes("manual_user_acceptance") &&
    !browserObjectiveAudit.includes("TODAY.md"),
  "Browser completion gates must produce durable machine artifacts and must not depend on ignored TODAY.md evidence",
);

assert(
  browserProviderDecisionReport.includes("provision_kasm_workspaces_first") &&
    browserProviderDecisionReport.includes(
      "hosted_provider_product_accepted",
    ) &&
    browserProviderDecisionReport.includes("native_product_media_accepted") &&
    browserProviderDecisionReport.includes("Record manual UX evidence"),
  "Browser provider decision report must keep product audio/manual blockers visible",
);

const planningSurface = [
  tasks,
  roadmap,
  state,
  browserCapsuleDoc,
  browserBakeoffDoc,
  browserObjectiveAudit,
  browserProviderDecisionReport,
].join("\n");

assert(
  planningSurface.includes("Kasm Workspaces") &&
    planningSurface.includes("BrowserBox") &&
    planningSurface.includes("Selkies") &&
    planningSurface.includes("not product audio acceptance"),
  "Browser planning surface must preserve hosted/native comparison and avoid pretending Selkies proof is final product acceptance",
);

assert(
  browserSessionCapacitySmoke.includes("HOME_VIRTUAL_AUTH_BROWSER_SUMMARY=1") &&
    browserSessionCapacitySmoke.includes("HOME_VIRTUAL_AUTH_BROWSER_OPEN=0") &&
    browserSessionCapacitySmoke.includes("node scripts/home-passkey-virtual-auth-smoke.mjs") &&
    read("scripts/home-passkey-virtual-auth-smoke.mjs").includes(
      "HOME_VIRTUAL_AUTH_BROWSER_SUMMARY",
    ) &&
    read("scripts/README.md").includes("browser-session-capacity-smoke.sh"),
  "Browser session-capacity proof must have a lightweight summary-only operator gate before heavy Browser opens",
);

for (const component of [
  "browser",
  "net-provider",
  "exit-provider",
  "browser-engine-adapter",
  "browser-engine-supervisor",
  "browser-native-proxy-engine",
  "browser-stream-bridge",
  "browser-local-exit",
]) {
  assert(
    components.includes(`"${component}"`),
    `components.json must include ${component}`,
  );
}

console.log(
  JSON.stringify({
    schema: "elastos.browser.entropy-check/v1",
    ok: true,
    dedicated_browser_entropy: true,
  }),
);
