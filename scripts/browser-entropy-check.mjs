#!/usr/bin/env node

import { existsSync, readFileSync } from "node:fs";

const repoRoot = new URL("../", import.meta.url);

function read(path) {
  return readFileSync(new URL(path, repoRoot), "utf8");
}

function exists(path) {
  return existsSync(new URL(path, repoRoot));
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
const browserCapsuleManifest = JSON.parse(browserManifest);
const browser = read("capsules/browser/browser/index.html");
const browserMain = read("capsules/browser/browser/browser.js");
const browserJs = readAll([
  "capsules/browser/browser/browser.js",
  "capsules/browser/browser/browser-clipboard.js",
  "capsules/browser/browser/browser-history.js",
  "capsules/browser/browser/browser-input.js",
  "capsules/browser/browser/browser-input-surface.js",
  "capsules/browser/browser/browser-location.js",
  "capsules/browser/browser/browser-page-cleanup.js",
  "capsules/browser/browser/browser-remote-display.js",
  "capsules/browser/browser/browser-runtime-api.js",
  "capsules/browser/browser/browser-status.js",
  "capsules/browser/browser/browser-webrtc.js",
]);
const browserInputSurface = read("capsules/browser/browser/browser-input-surface.js");
const browserRemoteDisplay = read("capsules/browser/browser/browser-remote-display.js");
const browserStyle = read("capsules/browser/browser/style.css");
const homeGuiWindowsSource = read("capsules/home-gui/browser/shell-windows.js");
const homeClipboardHost = read(
  "capsules/home/browser/home-clipboard-host.js",
);
const homeClipboardClient = read(
  "capsules/home/browser/home-clipboard-client.js",
);
const homeClipboardProtocol = read(
  "capsules/home/browser/home-clipboard-protocol.js",
);
const homeShellHostSource = read("capsules/home/browser/home-shell-host.js");
const homeShellHostContract = read("docs/HOME_SHELL_HOST_CONTRACT.md");
const homeClipboardHeadlessSmoke = read(
  "scripts/home-clipboard-headless-smoke.mjs",
);
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
const browserPlaywrightEngine = read(
  "elastos/tools/browser-playwright-engine/src/supervisor.mjs",
);
const browserPlaywrightWalletApproval = read(
  "elastos/tools/browser-playwright-engine/src/wallet-approval.mjs",
);
const browserLocalExit = read("elastos/tools/browser-local-exit/src/main.rs");
const browserRuntimeProxySmoke = read("scripts/browser-runtime-proxy-smoke.sh");
const browserNativeOperatorConfig = read(
  "scripts/browser-native-operator-config.mjs",
);
const browserDisplayModeSmoke = read("scripts/browser-display-mode-smoke.mjs");
const browserFixedProductRasterChromiumTest = read(
  "scripts/browser-fixed-product-raster-chromium.test.mjs",
);
const browserRuntimeTurnCapabilitySmoke = read(
  "scripts/browser-runtime-turn-capability-smoke.mjs",
);
const browserObjectiveAudit = read("scripts/browser-objective-audit.mjs");
const browserProviderDecisionReport = read(
  "scripts/browser-provider-decision-report.mjs",
);
const browserHostedProviderBakeoff = read(
  "scripts/browser-hosted-provider-bakeoff.sh",
);
const browserHostedProductWebrtcSmoke = read(
  "scripts/browser-hosted-product-webrtc-smoke.mjs",
);
const browserHostedProductSupervisor = read(
  "scripts/browser-hosted-product-supervisor.mjs",
);
const browserNativeTargetPreflight = read(
  "scripts/browser-native-target-preflight.sh",
);
const browserSelkiesControlService = read(
  "scripts/browser-selkies-control-service.mjs",
);
const browserSelkiesControlServiceSmoke = read(
  "scripts/browser-selkies-control-service-smoke.sh",
);
const browserWalletApprovalDeadlineSmoke = read(
  "scripts/browser-wallet-approval-deadline-smoke.mjs",
);
const browserSelkiesRuntimeExitTarget = read(
  "scripts/browser-selkies-runtime-exit-target.sh",
);
const browserPerLaunchSelkiesSupervisor = read(
  "scripts/browser-per-launch-selkies-supervisor.mjs",
);
const browserVmEngineSupervisor = read(
  "scripts/browser-vm-engine-supervisor.mjs",
);
const browserVmLocalCrosvmLauncher = read(
  "scripts/browser-vm-local-crosvm-launcher.mjs",
);
const browserVmEnginePreflight = read(
  "scripts/browser-vm-engine-preflight.sh",
);
const browserVmArtifactPreflight = read(
  "scripts/browser-vm-artifact-preflight.sh",
);
const browserVmArtifactPreflightSmoke = read(
  "scripts/browser-vm-artifact-preflight-smoke.sh",
);
const browserVmControlService = read("scripts/browser-vm-control-service.mjs");
const browserVmRemoteVzLauncher = read("scripts/browser-vm-remote-vz-launcher.mjs");
const browserVmRemoteVzLauncherIntegration = read(
  "scripts/browser-vm-remote-vz-launcher.integration.mjs",
);
const browserVzSupervisorProcessTest = read(
  "elastos/crates/elastos-vz/tests/browser_vz_engine_supervisor_process.rs",
);
const browserVmControlServiceSmoke = read(
  "scripts/browser-vm-control-service-smoke.sh",
);
const browserVmControlServicePersistentSmoke = read(
  "scripts/browser-vm-control-service-persistent-smoke.sh",
);
const browserVmControlServiceSettlementSmoke = read(
  "scripts/browser-vm-control-service-settlement-smoke.sh",
);
const remoteCarrierExitArtifactReadiness = read(
  "scripts/remote-carrier-exit-artifact-readiness.mjs",
);
const remoteCarrierExitArtifactReadinessSmoke = read(
  "scripts/remote-carrier-exit-artifact-readiness-smoke.sh",
);
const remoteCarrierExitReadiness = read(
  "scripts/remote-carrier-exit-readiness.mjs",
);
const remoteCarrierExitReadinessSmoke = read(
  "scripts/remote-carrier-exit-readiness-smoke.sh",
);
const browserVmEngineContractSmoke = read(
  "scripts/browser-vm-engine-contract-smoke.sh",
);
const browserVmRemoteControlPreflightSmoke = read(
  "scripts/browser-vm-remote-control-preflight-smoke.sh",
);
const browserVmTargetPreflight = read(
  "scripts/browser-vm-target-preflight.sh",
);
const browserVmTargetPreflightSmoke = read(
  "scripts/browser-vm-target-preflight-smoke.sh",
);
const browserVmTargetRefresh = read("scripts/browser-vm-target-refresh.sh");
const browserVmRuntimeRelay = read(
  "elastos/tools/browser-vm-runtime-relay/src/main.rs",
);
const browserVmRuntimeRelaySmoke = read(
  "scripts/browser-vm-runtime-relay-smoke.sh",
);
const browserVmGuestControlBridge = read(
  "elastos/tools/browser-vm-guest-control-bridge/src/main.rs",
);
const browserVmGuestControlBridgeSmoke = read(
  "scripts/browser-vm-guest-control-bridge-smoke.sh",
);
const browserVzEngineSupervisor = read(
  "elastos/crates/elastos-vz/src/bin/browser-vz-engine-supervisor.rs",
);
const browserVmTargetStage = read("scripts/build/stage-browser-vm-target.sh");
const browserVmTargetStageSmoke = read(
  "scripts/build/stage-browser-vm-target-smoke.sh",
);
const browserVmRootfsBuild = read("scripts/build/build-browser-vm-rootfs.sh");
const browserVmTargetDoc = read("docs/BROWSER_VM_TARGET.md");
const architectureDoc = read("docs/ARCHITECTURE.md");
const installDoc = read("docs/INSTALL.md");
const browserMacVmProof = read("scripts/browser-mac-vm-proof.sh");
const macDoc = read("docs/MAC.md");
const homePasskeyVirtualAuthSmoke = read("scripts/home-passkey-virtual-auth-smoke.mjs");
const elastosCommon = read("elastos/crates/elastos-common/src/lib.rs");
const browserSourceHomeConfig = read("scripts/browser-source-home-config.mjs");
const browserSourceHomeConfigSmoke = read("scripts/browser-source-home-config-smoke.sh");
const browserRuntimeTurn = read("scripts/browser-runtime-turn.mjs");
const browserRuntimeTurnSmoke = read("scripts/browser-runtime-turn-smoke.mjs");
const setupSourceHome = read("scripts/setup-source-home.sh");
const setupSourceHomeBrowserArtifacts = read(
  "scripts/setup-source-home-browser-artifacts.sh",
);
const setupSourceHomeBrowserConfigSmoke = read(
  "scripts/setup-source-home-browser-config-smoke.sh",
);
const setupSourceHomeBrowserArtifactsSmoke = read(
  "scripts/setup-source-home-browser-artifacts-smoke.sh",
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
  "elastos/crates/elastos-server/src/api/gateway_browser_sessions.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_stream.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_validation.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_wallet.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_wallet_bridge.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_wallet_reads.rs",
]);
const gatewayBrowserRouteTests = read(
  "elastos/crates/elastos-server/src/api/gateway_browser_route_tests.rs",
);
const homeBrowserRestoredLifecycleHeadlessSmoke = read(
  "scripts/home-browser-restored-lifecycle-headless-smoke.mjs",
);
const browserProfileResetRoute = sourceBlock(
  gatewayBrowserApi,
  "pub(super) async fn browser_app_profile_reset",
  "Browser profile reset route",
);
const gatewayApi = readAll([
  "elastos/crates/elastos-server/src/api/gateway.rs",
  "elastos/crates/elastos-server/src/api/gateway_home_runtime.rs",
  "elastos/crates/elastos-server/src/api/gateway_home_token.rs",
  "elastos/crates/elastos-server/src/api/gateway_models.rs",
  "elastos/crates/elastos-server/src/api/gateway_provider_proxy.rs",
  "elastos/crates/elastos-server/src/api/gateway_server.rs",
]);
assert(
  gatewayApi.includes("legacy /api/provider/net/stream is disabled") &&
    !gatewayApi.includes(
      "return gateway_browser::gateway_browser_net_stream(registry.as_ref(), &request).await;",
    ),
  "Legacy /api/provider/net/stream must fail closed instead of delegating to Browser stream reservation",
);
const gatewayBrowserProfileTests = read(
  "elastos/crates/elastos-server/src/api/gateway_tests/browser_profile.rs",
);
const gatewayHomeSystemTests = read(
  "elastos/crates/elastos-server/src/api/gateway_tests/home_system.rs",
);
const shellWindows = read("capsules/home-gui/browser/shell-windows.js");
const browserSettingsPanelIndex = browser.indexOf('id="browser-settings-panel"');
const browserProfileResetIndex = browser.indexOf('id="browser-profile-reset"');

assert(
  browserManifest.includes('"name": "browser"') &&
    browserCapsuleManifest.runtime_abi === "elastos.runtime-projection/v1" &&
    browserCapsuleManifest.bus_contract === "elastos.runtime-projection/v1" &&
    browserCapsuleManifest.execution === "web-projection" &&
    browserCapsuleManifest.entrypoint === "browser/index.html" &&
    !Object.hasOwn(browserCapsuleManifest, "wit_world_sha256") &&
    browserCapsuleManifest.capabilities.includes("elastos://browser/page") &&
    browserCapsuleManifest.capabilities.includes("elastos://browser/display") &&
    browserCapsuleManifest.capabilities.includes("elastos://browser/exit") &&
    browserCapsuleManifest.capabilities.includes("elastos://browser/profile") &&
    browserCapsuleManifest.capabilities.includes("elastos://browser/wallet-bridge") &&
    browserCapsuleManifest.requires.some((req) => req.name === "browser-engine-adapter") &&
    browserCapsuleManifest.requires.some((req) => req.name === "exit-provider") &&
    browserCapsuleManifest.requires.some((req) => req.name === "net-provider") &&
    browserCapsuleManifest.requires.some((req) => req.name === "wallet-provider") &&
    !browserManifest.includes('"elastos://wallet/*"') &&
    !browserManifest.includes('"elastos://net/stream"') &&
    !browserManifest.includes('"elastos://exit/*"') &&
    !browserManifest.includes('"elastos://browser-engine/*"') &&
    !browserManifest.includes("guest_network") &&
    !browserManifest.includes('"provides"'),
  "Browser capsule manifest must declare a Browser-scoped Runtime projection without raw Browser Engine, Exit, Net, Wallet, or guest-network authority",
);

assert(
  browserSettingsPanelIndex >= 0 &&
    browserProfileResetIndex > browserSettingsPanelIndex,
  "Browser profile reset control must live inside Browser settings, not the main navigation toolbar",
);

assert(
  browser.includes("https://ela.city/") &&
    browser.includes('id="browser-back"') &&
    browser.includes('id="browser-forward"') &&
    browser.includes('id="browser-refresh"') &&
    browser.includes('id="browser-profile-reset"') &&
    browser.includes('id="browser-settings"') &&
    browser.includes('id="browser-settings-panel"') &&
    browser.includes('id="browser-exit"') &&
    browser.includes('class="browser-exit-select"') &&
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
    browserJs.includes("recoverableRuntimePage") &&
    browserJs.includes("elastos.browser.cleanup-handle/v1") &&
    browserJs.includes("elastos.browser.close-request/v2") &&
    !browserJs.includes("runtime_close") &&
    !browserJs.includes("sessionStorage") &&
    browserJs.includes("Only http and https addresses") &&
    browserJs.includes("/api/apps/browser/open") &&
    browserJs.includes("/api/apps/browser/summary") &&
    browserJs.includes("visibleRemoteCarrierExits") &&
    browserJs.includes("This device") &&
    browserJs.includes("Seed Exit Node") &&
    browserJs.includes("Shared Exit Node") &&
    browserJs.includes("remote_exit_id") &&
    browserJs.includes('requestRuntimeOpen(nextUrl, { history: "replace" })') &&
    browserJs.includes("exitSelect.disabled = loading") &&
    browserJs.includes("selectedRemoteExitId = currentRemoteExitId") &&
    !browserJs.includes("Exit changed. Open the address again to use it.") &&
    browserJs.includes("Browser could not use the selected Exit Node.") &&
    browserInputSurface.includes('target.closest?.("#browser-settings-panel")') &&
    browserInputSurface.includes('target.id === "browser-exit"') &&
    browserJs.includes("elastos.browser.open-result/v1") &&
    browserJs.includes("Browser is temporarily unavailable") &&
    browserJs.includes("blocked by your Exit Node settings") &&
    browserJs.includes("historyEntries"),
  "Browser UI must use the high-level Browser open route and fail closed instead of direct provider routes",
);

assert(
  browserJs.includes("isMissingRuntimePageError") &&
    browserJs.includes("settleRemoteDisplayFailure") &&
    browserJs.includes("RUNTIME_OWNED_PAGE_FAILURE_KINDS") &&
    browserJs.includes("runtimePageCleanup.fail") &&
    browserJs.includes("retry: false") &&
    !browserJs.includes("scheduleRemoteReplacementAfterTerminal") &&
    !browserJs.includes("scheduleRemoteReconnect") &&
    browserJs.includes(
      "Runtime cleanup is pending; the existing page remains owned and no replacement will open until Runtime confirms a terminal close.",
    ) &&
    browserRemoteDisplay.includes('"no_first_frame" ? "no_first_frame" : "signaling"') &&
    browserRemoteDisplay.includes(
      "Runtime must close this session before another Browser Engine or Exit Node can open.",
    ) &&
    !browserRemoteDisplay.includes("Refresh Browser") &&
    browserJs.includes("recoverMissingRuntimePage") &&
    browserJs.includes(
      "Runtime confirmed the failed Browser session closed. You can open the address again or choose another Browser Engine.",
    ) &&
    !browserJs.includes("Browser session reconnected.") &&
    browserJs.includes("track.addEventListener(\"mute\"") &&
    browserJs.includes("track.addEventListener(\"ended\"") &&
    browserSelkiesControlService.includes("crypto.randomBytes(8)") &&
    !browserSelkiesControlService.includes("update(`${url}\\0${streamId}`)"),
  "Browser sessions must use launch-unique provider page ids, retain cleanup ownership, and stop after terminal display failure until an explicit user open",
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
    exitProvider.includes("allowed_private_targets") &&
    exitProvider.includes("allows_private_target") &&
    exitProvider.includes("stream_backend_can_allow_exact_runtime_gateway_private_target_only") &&
    exitProvider.includes("remote_carrier_exits") &&
    exitProvider.includes("elastos.exit.remote-carrier.discovery/v1") &&
    exitProvider.includes("elastos.exit.remote-carrier.quote/v1") &&
    exitProvider.includes("elastos.exit.remote-carrier-session/v1") &&
    exitProvider.includes("grant_id") &&
    exitProvider.includes("expires_at") &&
    exitProvider.includes('"state": exit.state(now)') &&
    exitProvider.includes("remote Carrier Exit grant_id must be a safe identifier") &&
    exitProvider.includes("Remote Carrier Exit grant is expired") &&
    exitProvider.includes("exit_permission_denied") &&
    exitProvider.includes("exit_quota_exceeded") &&
    exitProvider.includes("remote_carrier_exit_discovery_is_principal_scoped_and_policy_filtered") &&
    exitProvider.includes("remote_carrier_exit_expired_grant_is_diagnosable_but_not_usable") &&
    exitProvider.includes("remote_carrier_exit_enforces_active_stream_quota") &&
    exitProvider.includes("max_active_streams_per_principal") &&
    exitProvider.includes("remote_carrier_exit_enforces_principal_stream_quota_on_shared_grant") &&
    exitProvider.includes('"byte_transport": "carrier_stream"') &&
    remoteCarrierExitArtifactReadiness.includes("browser_exit_stream") &&
    remoteCarrierExitArtifactReadiness.includes("elastos.browser.carrier-stream/v1") &&
    remoteCarrierExitArtifactReadiness.includes("elastos.exit.remote-carrier-session/v1") &&
    remoteCarrierExitArtifactReadinessSmoke.includes("stale_gateway_rejected") &&
    remoteCarrierExitArtifactReadinessSmoke.includes("stale_exit_provider_rejected") &&
    remoteCarrierExitReadiness.includes("config_sha256") &&
    remoteCarrierExitReadiness.includes("sha256File(args.sourceConfig)") &&
    remoteCarrierExitReadinessSmoke.includes("hash-bound to source and exit configs") &&
    gatewayBrowserApi.includes("browser_visible_remote_carrier_exits") &&
    gatewayBrowserApi.includes("scrub_exit_authority_fields") &&
    gatewayBrowserApi.includes('"remote_carrier_exit_count"') &&
    gatewayBrowserApi.includes('"remote_carrier_exits"') &&
    gatewayBrowserApi.includes('"allowed_principals"') &&
    gatewayBrowserApi.includes('"connect_ticket"') &&
    exitProvider.includes("max_body_bytes") &&
    exitProvider.includes("elastos.exit.http-fetch.result/v1") &&
    exitProvider.includes("elastos.exit.stream-session/v1") &&
    exitProvider.includes("elastos.adapter-ipc/v1") &&
    exitProvider.includes("elastos.exit.relay-ipc/v1") &&
    !exitProvider.includes("runtime_stream_path"),
  "Exit provider must expose typed HTTP/stream/remote-Carrier exits with permission/accounting and without raw host networking or public Runtime stream-path authority",
);

assert(
  browserEngineAdapter.includes("elastos.browser.engine.page/v1") &&
    browserEngineAdapter.includes(
      'const BROWSER_ENGINE_PROTOCOL_VERSION: &str = "2.0"',
    ) &&
    browserEngineAdapter.includes(
      "elastos.browser.engine-cleanup-binding/v2",
    ) &&
    browserEngineAdapter.includes(
      "elastos.browser.engine-cleanup-result/v2",
    ) &&
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
    browserEngineAdapter.includes("diagnostics") &&
    browserEngineAdapter.includes("/pages/{page_id}/diagnostics") &&
    browserEngineAdapter.includes("direct_network") &&
    browserEngineAdapter.includes("wallet_injection") &&
    read("capsules/browser-engine-adapter/capsule.json").includes("diagnostics"),
  "Browser Engine Adapter must be an explicit fail-closed adapter contract, not host browser authority",
);

assert(
  browserPlaywrightEngine.includes("state.runtimeProxy.activePrincipalId = normalizePrincipalId(request.principal_id)") &&
    browserPlaywrightEngine.includes("principal_id: normalizePrincipalId(runtimeProxy?.activePrincipalId)") &&
    browserPlaywrightEngine.includes("function normalizePrincipalId") &&
    browserLocalExit.includes("elastos.browser.local-exit.relay-open/v1") &&
    browserLocalExit.includes("relay_open_log_preserves_principal_attribution") &&
    browserRuntimeProxySmoke.includes("smoke_principal_id=") &&
    browserRuntimeProxySmoke.includes("principal_id: process.env.BROWSER_SMOKE_PRINCIPAL_ID") &&
    browserRuntimeProxySmoke.includes("localExitRelayOpenLogs") &&
    browserRuntimeProxySmoke.includes("local Exit relay-open did not preserve the launch principal_id"),
  "Playwright proof proxy must preserve launch principal attribution in local Exit relay-open proofs",
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
    browserEngineSupervisor.includes('"view": {') &&
    browserEngineSupervisor.includes('"mode": "native_surface"') &&
    browserEngineSupervisor.includes('"width": viewport.width') &&
    browserEngineAdapter.includes("validate_native_surface_geometry") &&
    browserEngineAdapter.includes("native_surface_supervisor_result_requires_view_geometry") &&
    browserEngineAdapter.includes(
      "native_surface display dimensions must match Runtime view geometry",
    ) &&
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
  !browserJs.includes(["diagnostic", "frame"].join("_")) &&
    !browserJs.includes(["runtime", "frame"].join("_")) &&
    !browserDisplayModeSmoke.includes(["diagnostic", "frame"].join("_")) &&
    !browserDisplayModeSmoke.includes(["runtime", "frame"].join("_")) &&
    !browserCapsuleDoc.includes(["diagnostic", "frame"].join("_")) &&
    !browserCapsuleDoc.includes("/api/apps/browser/pages/:page_id/frame") &&
    !browserCapsuleDoc.includes("Playwright Chromium frame/input"),
  "Browser diagnostic/runtime frame display paths must be removed from product Browser code",
);

assert(
  browserJs.includes("const expectsAudio = displaySession.audio === true") &&
    browserJs.includes("prepareAudio(expectsAudio)") &&
    browserJs.includes("unlockRemoteAudioFromGesture") &&
    browserJs.includes('nextPeerConnection.addTransceiver("audio"') &&
    browserJs.includes('event?.type === "paste_text"') &&
    browserJs.includes('event?.type === "file_upload"') &&
    browserJs.includes("Remote audio enabled.") &&
    browserDisplayModeSmoke.includes("audio_invariants_checked"),
  "Browser UI must keep WebRTC audio explicit, user-gesture unlocked, and covered by display-mode smoke",
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
    gatewayBrowserApi.includes("owner_launch_id") &&
    gatewayBrowserApi.includes("BrowserOpenJobReservation") &&
    gatewayBrowserApi.includes("Browser lifecycle is already active or launching for this verified launch") &&
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
  gatewayBrowserApi.includes("browser_open_job_for_owner") &&
    gatewayBrowserApi.includes("job.owner_launch_id == owner_launch_id") &&
    gatewayBrowserApi.includes("Browser lifecycle already owns a different open intent") &&
    gatewayBrowserApi.includes("pending_engine_cleanups") &&
    gatewayBrowserApi.includes("claim_pending_browser_engine_cleanups") &&
    gatewayBrowserApi.includes("record_browser_engine_cleanup_obligation") &&
    gatewayBrowserApi.includes("browser_terminal_close_receipt") &&
    gatewayBrowserApi.includes("BrowserCleanupHandle") &&
    gatewayBrowserApi.includes("BrowserDurableOwnership") &&
    gatewayBrowserApi.includes("write_browser_durable_ownership") &&
    gatewayBrowserApi.includes("secure_browser_lifecycle_dir") &&
    gatewayBrowserApi.includes("claim_pending_browser_stream_cleanups") &&
    gatewayBrowserApi.includes("browser_page_cleanup_for_principal") &&
    gatewayBrowserApi.includes("require_browser_engine_provider_binding") &&
    gatewayBrowserApi.includes(
      "no replacement page may open before terminal provider closure",
    ) &&
    browserJs.includes("elastos.browser.cleanup-handle/v1") &&
    browserJs.includes("elastos.browser.close-request/v2") &&
    !browserJs.includes("engine_protocol_version") &&
    !browserJs.includes("runtime_close") &&
    gatewayBrowserRouteTests.includes(
      "test_browser_async_duplicate_open_coalesces_by_verified_launch_owner",
    ) &&
    gatewayBrowserRouteTests.includes(
      "test_browser_open_job_and_page_routes_require_exact_verified_launch_owner",
    ) &&
    gatewayBrowserRouteTests.includes(
      "test_browser_close_transport_and_exit_failures_retry_independent_cleanup_obligations",
    ) &&
    gatewayBrowserRouteTests.includes(
      "test_browser_no_first_frame_cleanup_retries_exact_effect_without_replacement",
    ) &&
    gatewayBrowserRouteTests.includes(
      "test_browser_pending_cleanup_rejects_foreign_authority_and_effect_substitution",
    ) &&
    gatewayBrowserApi.includes(
      "runtime_restart_recovers_exact_durable_cleanup_ownership",
    ) &&
    gatewayBrowserApi.includes(
      "status_reads_preserve_90_second_launch_and_four_hour_active_owner",
    ) &&
    homeBrowserRestoredLifecycleHeadlessSmoke.includes(
      "fixture_duplicate_open=1",
    ) &&
    homeBrowserRestoredLifecycleHeadlessSmoke.includes(
      "state.browserOpenRequests === 2",
    ) &&
    homeBrowserRestoredLifecycleHeadlessSmoke.includes(
      "state.browserOpenEffects === 1",
    ) &&
    homeBrowserRestoredLifecycleHeadlessSmoke.includes(
      "state.browserCleanupEffects === 1",
    ) &&
    homeBrowserRestoredLifecycleHeadlessSmoke.includes(
      "state.browserPageCount === 1",
    ) &&
    homeBrowserRestoredLifecycleHeadlessSmoke.includes(
      "state.browserVmCount === 1",
    ),
  "Browser lifecycle must coalesce matching pending/completed owner intent, retain an exact provider/engine/stream cleanup binding after the active route retires, reject substituted authority or effects, block replacement before terminal closure, and keep a real Home-refresh regression",
);

assert(
  gatewayBrowserApi.includes("browser_attach_runtime_stream_path") &&
    gatewayBrowserApi.includes("browser_stream_relay") &&
    gatewayBrowserApi.includes("read_browser_relay_open_line") &&
    gatewayBrowserApi.includes("BROWSER_RUNTIME_RELAY_OPEN_MAX_BYTES") &&
    gatewayBrowserApi.includes("write_all(&open_line)") &&
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
  browserSelkiesControlService.includes('config.signaling_protocol || "auto"') &&
    browserSelkiesControlServiceSmoke.includes('response.signaling_protocol !== "auto"') &&
    browserSelkiesControlServiceSmoke.includes('signaling_protocol: "auto"') &&
    browserSelkiesControlServiceSmoke.includes("fake-selkies-force-legacy"),
  "Selkies signaling must default to auto while keeping explicit legacy fallback coverage",
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
    browserSelkiesControlService.includes("eth_signTypedData_v4") &&
    browserSelkiesControlService.includes("eth_sendTransaction") &&
    browserSelkiesControlService.includes("Runtime.addBinding") &&
    browserSelkiesControlService.includes("__elastosBrowserWalletRuntime") &&
    browserSelkiesControlService.includes('const WALLET_RUNTIME_ORIGIN = "null"') &&
    browserSelkiesControlService.includes("Origin: WALLET_RUNTIME_ORIGIN") &&
    browserPlaywrightEngine.includes('const WALLET_RUNTIME_ORIGIN = "null"') &&
    (browserPlaywrightEngine.match(/Origin: WALLET_RUNTIME_ORIGIN/g) || []).length === 5 &&
    browserSelkiesControlServiceSmoke.includes('origin: req.headers.origin || ""') &&
    browserSelkiesControlServiceSmoke.includes('req.headers.origin !== "null"') &&
    browserSelkiesControlServiceSmoke.includes('request.origin !== "null"') &&
    browserSelkiesControlService.includes('runtimePost("approval"') &&
    browserSelkiesControlService.includes("walletApprovalPending") &&
    browserSelkiesControlService.includes("waitForCachedWalletApproval") &&
    browserSelkiesControlService.includes("approval_reuse") &&
    browserSelkiesControlService.includes("request_suffix") &&
    !browserSelkiesControlService.includes("runtimePost(state.approvalUrl") &&
    browserSelkiesControlService.includes(
      'typeof status.transaction_hash === "string"',
    ) &&
    browserSelkiesControlService.includes('runtimePost("transactionBroadcast"') &&
    browserSelkiesControlService.includes(
      "Runtime transaction broadcast did not return a transaction hash.",
    ) &&
    browserSelkiesControlService.includes("transaction_broadcast") &&
    !browserSelkiesControlService.includes("bridgeUrl:") &&
    !browserSelkiesControlService.includes("approvalUrl:") &&
    !browserSelkiesControlService.includes("transactionUrl:") &&
    !browserSelkiesControlService.includes("readUrl:") &&
    !browserSelkiesControlService.includes("transactionBroadcastUrl:") &&
    !browserSelkiesControlService.includes("approvalStatusUrl:") &&
    !browserSelkiesControlService.includes("runtimePost(state.transactionBroadcastUrl") &&
    browserSelkiesControlService.includes("waitForWalletApprovalStatus") &&
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
    browserHostedProductWalletSmoke.includes("addedEscChain") &&
    browserSelkiesControlServiceSmoke.includes("wallet:smoke-eth-sign") &&
    browserSelkiesControlServiceSmoke.includes("wallet:smoke-typed-data") &&
    browserSelkiesControlServiceSmoke.includes("eth_signTypedData_v4") &&
    browserSelkiesControlServiceSmoke.includes("Runtime wallet eth_sign request was not normalized to personal_sign") &&
    browserCapsuleDoc.includes("The product Browser control service exposes a constrained `window.ethereum`") &&
    browserCapsuleDoc.includes("Playwright proof remains a") &&
    browserCapsuleDoc.includes("diagnostic/account-chain/personal-sign surface"),
  "Hosted Browser wallet bridge must be fail-present, coalesce duplicate in-flight signature approvals, and expose modern injected-wallet discovery, Runtime approval routing, and permission compatibility without giving pages raw wallet or node authority",
);

assert(
  browserSelkiesControlService.includes(
    "approval?.approval_request?.expires_at",
  ) &&
    browserPlaywrightEngine.includes(
      "body?.approval_request?.expires_at",
    ) &&
    browserSelkiesControlService.includes("walletApprovalDeadlineMs") &&
    browserPlaywrightWalletApproval.includes("walletApprovalDeadlineMs") &&
    browserSelkiesControlService.includes("30 * 60 * 1000") &&
    browserPlaywrightWalletApproval.includes("30 * 60 * 1000") &&
    browserSelkiesControlService.includes(
      "withWalletApprovalStatusTimeout",
    ) &&
    browserPlaywrightWalletApproval.includes(
      "withWalletApprovalStatusTimeout",
    ) &&
    browserSelkiesControlService.includes("statusIoTimeoutMs = 3000") &&
    browserPlaywrightWalletApproval.includes("statusIoTimeoutMs = 3000") &&
    browserSelkiesControlService.includes("observeWalletApprovalStatus") &&
    browserPlaywrightWalletApproval.includes("observeWalletApprovalStatus") &&
    browserSelkiesControlService.includes("timeout_ms: timeoutMs") &&
    browserSelkiesControlService.includes("enforceWallClockTimeout") &&
    browserSelkiesControlService.includes("? setTimeout(() =>") &&
    browserPlaywrightEngine.includes("new AbortController()") &&
    browserPlaywrightEngine.includes("signal: controller.signal") &&
    !browserSelkiesControlService.includes(
      "Date.now() + 5 * 60 * 1000",
    ) &&
    !browserPlaywrightEngine.includes(
      "Date.now() + 5 * 60 * 1000",
    ) &&
    browserWalletApprovalDeadlineSmoke.includes("after-five-minutes") &&
    browserWalletApprovalDeadlineSmoke.includes("provider-expiry") &&
    browserWalletApprovalDeadlineSmoke.includes("final-status-race") &&
    browserWalletApprovalDeadlineSmoke.includes("already-broadcast") &&
    browserWalletApprovalDeadlineSmoke.includes("exact-request") &&
    browserWalletApprovalDeadlineSmoke.includes("hanging-status") &&
    browserWalletApprovalDeadlineSmoke.includes(
      "transient-exact-request",
    ) &&
    browserWalletApprovalDeadlineSmoke.includes(
      "deadline-final-observation-failure",
    ) &&
    browserWalletApprovalDeadlineSmoke.includes(
      "baseline_assertions: baselineAssertions",
    ) &&
    browserWalletApprovalDeadlineSmoke.includes("real_sleep_ms: 0"),
  "Trusted Browser adapters must use bounded status I/O through Runtime expiry while retaining exact-request caches across indeterminate observations",
);

assert(
  gatewayBrowserApi.includes('"eth_getTransactionByHash" =>') &&
    gatewayBrowserApi.includes('.get("transaction")') &&
    gatewayBrowserApi.includes(
      "chain provider transaction response is missing transaction",
    ) &&
    gatewayBrowserApi.includes('"eth_getTransactionReceipt" =>') &&
    gatewayBrowserApi.includes('.get("receipt")') &&
    gatewayBrowserApi.includes(
      "chain provider receipt response is missing receipt",
    ),
  "Browser wallet reads must return raw EVM transaction and receipt objects, not chain-provider wrapper receipts",
);

assert(
  browserSelkiesRuntimeExitTarget.includes('selkies_encoder="x264enc"') &&
    browserSelkiesRuntimeExitTarget.includes('selkies_framerate="30"') &&
    browserSelkiesRuntimeExitTarget.includes('selkies_width="1920"') &&
    browserSelkiesRuntimeExitTarget.includes('selkies_height="1080"') &&
    browserSelkiesRuntimeExitTarget.includes(
      '\\"--force-device-scale-factor=1\\"',
    ) &&
    !browserSelkiesRuntimeExitTarget.includes(
      "ELASTOS_SELKIES_INITIAL_RESOLUTION",
    ) &&
    !browserSelkiesRuntimeExitTarget.includes("--selkies-resolution-mode") &&
    !browserSelkiesRuntimeExitTarget.includes("--selkies-width") &&
    !browserSelkiesRuntimeExitTarget.includes("--selkies-height") &&
    browserSelkiesRuntimeExitTarget.includes(
      "--is-manual-resolution-mode=true",
    ) &&
    browserSelkiesRuntimeExitTarget.includes("--enable-resize=false") &&
    browserSelkiesRuntimeExitTarget.includes("--clipboard-enabled=true") &&
    browserSelkiesRuntimeExitTarget.includes(
      "browser-selkies-cargo-target",
    ) &&
    !browserSelkiesServiceWrapper.includes("ELASTOS_BROWSER_SELKIES_RESOLUTION_MODE") &&
    !browserSelkiesServiceWrapper.includes("ELASTOS_BROWSER_SELKIES_WIDTH") &&
    !browserSelkiesServiceWrapper.includes("ELASTOS_BROWSER_SELKIES_HEIGHT") &&
    !browserSelkiesServiceEnv.includes("ELASTOS_BROWSER_SELKIES_RESOLUTION_MODE") &&
    !browserSelkiesServiceEnv.includes("ELASTOS_BROWSER_SELKIES_WIDTH") &&
    !browserSelkiesServiceEnv.includes("ELASTOS_BROWSER_SELKIES_HEIGHT") &&
    browserCapsuleDoc.includes(
      "fixed 1920x1080 stream/page raster at DPR 1",
    ),
  "Canonical hosted Browser launcher must keep one fixed 1920x1080 DPR-1 compositor/capture/page raster and expose only codec tuning",
);

assert(
  browserPerLaunchSelkiesSupervisor.includes(
    'const BROWSER_PROGRAM_ENV = "ELASTOS_BROWSER_SELKIES_BROWSER_PROGRAM"',
  ) &&
    browserPerLaunchSelkiesSupervisor.includes("--browser-program") &&
    browserPerLaunchSelkiesSupervisor.includes("PROFILE_ROOT_ENV") &&
    browserPerLaunchSelkiesSupervisor.includes("DEFAULT_STARTUP_TIMEOUT_MS = 90000") &&
    browserPerLaunchSelkiesSupervisor.includes("readinessDiagnostics(outDir)") &&
    browserPerLaunchSelkiesSupervisor.includes("`profile-${digest}`") &&
    !browserPerLaunchSelkiesSupervisor.includes("`principal-${digest}`") &&
    browserPerLaunchSelkiesSupervisor.includes("--profile-dir") &&
    browserPerLaunchSelkiesSupervisor.includes("result.control_socket_path = controlSocket") &&
    browserPerLaunchSelkiesSupervisor.includes("result.isolated_session = true") &&
    browserPerLaunchSelkiesSupervisor.includes("killProcessGroup(target)") &&
    browserSelkiesRuntimeExitTarget.includes("--profile-dir") &&
    browserSelkiesRuntimeExitTarget.includes("/var/lib/elastos-browser-profile") &&
    browserSelkiesRuntimeExitTarget.includes(".elastos-profile.lock") &&
    browserSelkiesRuntimeExitTarget.includes("profile_persistent: true") &&
    !browserSelkiesRuntimeExitTarget.includes("/tmp/chromium-profile") &&
    browserPerLaunchSelkiesSupervisorSmoke.includes(
      "ELASTOS_BROWSER_SERVICE_HOME",
    ) &&
    browserPerLaunchSelkiesSupervisorSmoke.includes(
      "ELASTOS_BROWSER_PROFILE_ROOT",
    ) &&
    browserPerLaunchSelkiesSupervisorSmoke.includes(
      "profile_persistent === true",
    ) &&
    browserPerLaunchSelkiesSupervisorSmoke.includes(
      "/^profile-[0-9a-f]{64}$/",
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
  "Per-launch Browser supervisor must use explicit executable discovery, return page-scoped control sockets, persist per-principal Browser profiles, fail fast with diagnostics, prove two isolated sessions, and shut them down",
);

assert(
  browserSourceHomeConfig.includes("SUPPORTED_PLATFORMS") &&
    browserSourceHomeConfig.includes("browser-vm-product") &&
    browserSourceHomeConfig.includes("chromium_microvm") &&
    browserSourceHomeConfig.includes("browser-vm-engine-supervisor") &&
    browserSourceHomeConfig.includes("browser-vm-local-crosvm-launcher") &&
    browserSourceHomeConfig.includes('path.join(args.dataDir, "bvm")') &&
    browserSourceHomeConfig.includes("ELASTOS_BROWSER_VM_ROOTFS_POOL_DIR") &&
    browserSourceHomeConfig.includes("ELASTOS_BROWSER_VM_ROOTFS_COPY_MODE") &&
    browserSourceHomeConfig.includes("ELASTOS_BROWSER_VM_ROOTFS_POOL_REFILL_COUNT") &&
    browserSourceHomeConfig.includes("ELASTOS_BROWSER_VM_ROOTFS_POOL_REFILL_SCRIPT") &&
    browserVmLocalCrosvmLauncher.includes("refillPreparedRootfsPoolSync") &&
    browserVmLocalCrosvmLauncher.includes("function rootfsPoolRefillCommand") &&
    browserVmLocalCrosvmLauncher.includes("return { command: refillScript, args: [] }") &&
    browserVmLocalCrosvmLauncher.includes("rootfs-pool-refill-sync.log") &&
    browserVmLocalCrosvmLauncher.includes("prepared rootfs pool synchronous refill completed") &&
    browserSourceHomeConfig.includes('"pool-required"') &&
    browserSourceHomeConfig.includes('display_modes: ["webrtc_remote_display"]') &&
    !browserSourceHomeConfig.includes("preferred_display_mode") &&
    browserSourceHomeConfig.includes("relay_ipc") &&
    browserSourceHomeConfig.includes("relay_ipc: true") &&
    browserSourceHomeConfig.includes("-relay.sock") &&
    browserSourceHomeConfig.includes("browser-local-exit.json") &&
    browserSourceHomeConfig.includes("elastos.browser.local-exit.config/v1") &&
    browserSourceHomeConfig.includes("runtimeGatewayPrivateTargets") &&
    browserSourceHomeConfig.includes("ELASTOS_BROWSER_RUNTIME_GATEWAY_PORTS") &&
    browserSourceHomeConfig.includes('host: "localhost"') &&
    browserSourceHomeConfig.includes('ports: [80, 443]') &&
    browserSourceHomeConfig.includes("relay_ipc_path") &&
    browserSourceHomeConfig.includes("control_socket_path") &&
    browserSourceHomeConfig.includes("/tmp/elastos-browser-vm-control-${args.platform}.sock") &&
    browserSourceHomeConfig.includes("ELASTOS_BROWSER_VM_CONTROL_SOCKET") &&
    !browserSourceHomeConfig.includes("ELASTOS_BROWSER_VM_PROFILE_DISK_ROOT") &&
    browserSourceHomeConfig.includes("ELASTOS_BROWSER_VM_EGRESS_MAX_SESSIONS") &&
    browserSourceHomeConfig.includes('const VM_ADAPTER_MAX_ACTIVE_SESSIONS = "4"') &&
    browserSourceHomeConfig.includes('const VM_CONTROL_MAX_ACTIVE_PAGES = "1"') &&
    browserSourceHomeConfigSmoke.includes("multiple isolated Browser VM sessions") &&
    browserSourceHomeConfigSmoke.includes("single active Browser VM page") &&
    browserSourceHomeConfig.includes('const VM_IDLE_KEEPALIVE_MS = "300000"') &&
    browserSourceHomeConfig.includes('const VM_LINUX_IDLE_KEEPALIVE_MS = "0"') &&
    browserSourceHomeConfig.includes('const VM_REUSE_IDLE_VMS = "1"') &&
    browserSourceHomeConfig.includes('const VM_LINUX_REUSE_IDLE_VMS = "0"') &&
    browserSourceHomeConfig.includes("ELASTOS_BROWSER_VM_REUSE_IDLE_VMS") &&
    browserSourceHomeConfig.includes("ELASTOS_BROWSER_REMOTE_VZ_LAUNCH_TIMEOUT_MS") &&
    browserSourceHomeConfig.includes('const VM_EGRESS_MAX_SESSIONS = "16"') &&
    browserSourceHomeConfigSmoke.includes("bound pre-opened Runtime egress streams") &&
    browserSourceHomeConfigSmoke.includes("Runtime launch descriptors, not a global profile root env") &&
    browserSourceHomeConfigSmoke.includes("must not use the old hosted Browser profile root env") &&
    browserSourceHomeConfigSmoke.includes("Linux VM adapter must not retain warm crosvm sessions") &&
    browserSourceHomeConfigSmoke.includes("Mac VZ source-home Browser config may keep same-principal Browser VMs warm briefly") &&
    browserSourceHomeConfigSmoke.includes("pin the remote launcher budget instead of inheriting ambient env") &&
    browserSourceHomeConfig.includes("ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4") &&
    browserSourceHomeConfigSmoke.includes("Linux source-home Browser config must default to the local crosvm VM control launcher") &&
    browserSourceHomeConfigSmoke.includes("Linux source-home Browser config must use a prepared rootfs pool") &&
    browserSourceHomeConfigSmoke.includes("VM Browser source-home config must advertise only the WebRTC product display mode") &&
    browserSourceHomeConfig.includes("deriveGuestIpv4") &&
    browserSourceHomeConfig.includes("turnIpv4HostFromUrl") &&
    browserSourceHomeConfig.includes("runtimeTurnEnvCandidates") &&
    browserSourceHomeConfig.includes("turn-credentials.env") &&
    browserSourceHomeConfig.includes("ELASTOS_BROWSER_RUNTIME_TURN_ENV") &&
    browserSourceHomeConfig.includes("ELASTOS_BROWSER_VM_CONTROL_STATUS_PROBE_TIMEOUT_MS") &&
    browserSourceHomeConfig.includes("ELASTOS_BROWSER_VM_DEBUG_HOLD_ON_OPEN_ERROR_MS") &&
    browserSourceHomeConfig.includes("isRemoteVzControlLauncher") &&
    browserSourceHomeConfig.includes("remoteVzControlLauncher") &&
    browserSourceHomeConfig.includes('? "/tmp/evzs"') &&
    browserSourceHomeConfig.includes("ELASTOS_BROWSER_VM_CONTROL_READY_TIMEOUT_MS") &&
    browserSourceHomeConfigSmoke.includes("Linux remote VZ source-home Browser config must use the remote VZ VM root") &&
    browserSourceHomeConfigSmoke.includes("Linux remote VZ source-home Browser config must not inherit local crosvm rootfs-pool env") &&
    browserSourceHomeConfigSmoke.includes("Linux remote VZ source-home Browser config must let the remote launcher derive an inner guest-ready margin") &&
    browserSourceHomeConfigSmoke.includes("assertNoRemoteVzLocalTurnEnv") &&
    browserSourceHomeConfigSmoke.includes("Remote VZ source-home Browser config") &&
    browserSourceHomeConfigSmoke.includes("Linux remote VZ source-home Browser config") &&
    browserSourceHomeConfigSmoke.includes("must not inherit local") &&
    browserSourceHomeConfigSmoke.includes("Remote VZ source-home Browser config must let the remote launcher derive an inner guest-ready margin") &&
    browserSourceHomeConfigSmoke.includes("Local VZ source-home Browser config must keep the guest control readiness budget bounded") &&
    browserRuntimeTurn.includes("elastos.browser.runtime-turn/v1") &&
    browserRuntimeTurn.includes("turnserver") &&
    browserRuntimeTurn.includes("ELASTOS_BROWSER_VM_TURNSERVER_BIN") &&
    setupSourceHome.includes("ELASTOS_BROWSER_VM_TURNSERVER_BIN") &&
    browserSourceHomeConfig.includes("ELASTOS_BROWSER_VM_TURNSERVER_BIN") &&
    browserVmLocalCrosvmLauncher.includes("ELASTOS_BROWSER_VM_TURNSERVER_BIN") &&
    browserRuntimeTurn.includes("DEFAULT_DARWIN_MEDIA_HOST_IPV4") &&
    browserRuntimeTurn.includes("DEFAULT_DARWIN_MEDIA_GUEST_IPV4") &&
    browserRuntimeTurn.includes("ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY=relay") &&
    browserRuntimeTurn.includes("ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4") &&
    browserRuntimeTurn.includes("ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4") &&
    browserRuntimeTurn.includes("ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX") &&
    browserRuntimeTurn.includes("detectDefaultRouteIpv4") &&
    browserRuntimeTurn.includes('"route", "get", "1.1.1.1"') &&
    browserRuntimeTurn.includes("pushTurnUrls") &&
    browserRuntimeTurn.includes("runtime TURN did not become reachable") &&
    browserRuntimeTurnSmoke.includes("elastos.browser.runtime-turn-smoke/v1") &&
    browserRuntimeTurnSmoke.includes("turn:10.44.0.10:3478?transport=udp") &&
    browserRuntimeTurnSmoke.includes("ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4=10.44.0.2") &&
    browserRuntimeTurnSmoke.includes("runtime TURN env must use only credentialed ICE_SERVERS_JSON") &&
    setupSourceHome.includes("start_browser_runtime_turn") &&
    setupSourceHome.includes("skip Browser runtime TURN relay: remote Browser VM control is preserved") &&
    setupSourceHome.includes("has_remote_browser_vm_control_config") &&
    setupSourceHome.includes("use existing Browser runtime TURN env") &&
    setupSourceHome.includes("browser-runtime-turn.mjs") &&
    setupSourceHomeBrowserConfigSmoke.includes("shared-runtime-turn-secret") &&
    setupSourceHomeBrowserConfigSmoke.includes("use existing Browser runtime TURN env") &&
    browserSourceHomeConfigSmoke.includes("runtime-turn-user") &&
    browserSourceHomeConfigSmoke.includes("guest-control status probe diagnostics") &&
    browserSourceHomeConfigSmoke.includes("open-error debug hold diagnostics") &&
    browserSourceHomeConfigSmoke.includes("Local Mac source-home Browser config must use no-NIC VZ transport by default") &&
    browserSourceHomeConfig.includes('engine_mode: "vm"') &&
    browserSourceHomeConfig.includes("source-home-browser-exit") &&
    !browserSourceHomeConfig.includes("--engine-mode") &&
    !browserSourceHomeConfig.includes("hosted-proof") &&
    !browserSourceHomeConfig.includes("allow-insecure-hosted-proof") &&
    !browserSourceHomeConfig.includes("insecure_hosted_proof") &&
    !browserSourceHomeConfig.includes("mac-container-product") &&
    !browserSourceHomeConfig.includes("hosted-product") &&
    browserCapsuleDoc.includes("The source-home Browser config is VM-only") &&
    browserCapsuleDoc.includes("does not expose a hosted-proof") &&
    browserVmTargetDoc.includes("Source-home config is VM-only") &&
    setupSourceHome.includes("browser-source-home-config.mjs") &&
    setupSourceHome.includes("browser-vm-engine-supervisor.mjs") &&
    setupSourceHome.includes("browser-vm-control-service.mjs") &&
    setupSourceHome.includes("browser-vm-remote-vz-launcher.mjs") &&
    setupSourceHome.includes("browser-vm-local-crosvm-launcher.mjs") &&
    setupSourceHome.includes("browser-vm-prepare-rootfs-pool.mjs") &&
    setupSourceHome.includes("browser-vz-engine-supervisor") &&
    setupSourceHome.includes("build Browser VZ engine supervisor") &&
    setupSourceHome.includes("-p elastos-vz --bin browser-vz-engine-supervisor") &&
    setupSourceHome.includes("browser-vm-engine-preflight.sh") &&
    setupSourceHome.includes("browser-vm-artifact-preflight.sh") &&
    setupSourceHome.includes("browser-vm-target-preflight.sh") &&
    setupSourceHome.includes("setup-source-home-browser-artifacts.sh") &&
    setupSourceHomeBrowserArtifacts.includes("elastos.setup-source-home.browser-artifacts/v1") &&
    setupSourceHomeBrowserArtifacts.includes("managed-runtimes") &&
    setupSourceHomeBrowserArtifacts.includes("browser-vm/rootfs.ext4") &&
    setupSourceHomeBrowserArtifacts.includes("bin/crosvm") &&
    setupSourceHomeBrowserArtifacts.includes("browser-vm/initrd") &&
    setupSourceHomeBrowserArtifacts.includes("bin/initrd") &&
    setupSourceHomeBrowserArtifactsSmoke.includes("elastos.setup-source-home.browser-artifacts-smoke/v1") &&
    setupSourceHomeBrowserArtifactsSmoke.includes("existing real kernel file must not be replaced") &&
    setupSourceHomeBrowserArtifactsSmoke.includes("Linux managed setup must not create the Mac VZ initrd path") &&
    setupSourceHomeBrowserArtifactsSmoke.includes("Mac managed setup must not create a crosvm link") &&
    setupSourceHome.includes("browser-selkies-control-service.mjs") &&
    setupSourceHome.includes("browser-vm-selkies-start") &&
    setupSourceHome.includes("browser-vm-init") &&
    setupSourceHome.includes("extract_browser_vm_init") &&
    setupSourceHome.includes("extract_browser_vm_selkies_start") &&
    setupSourceHome.includes("write_browser_vm_target_manifest") &&
    setupSourceHome.includes('"guarantee_level": "mechanism_microvm"') &&
    setupSourceHome.includes("/etc/elastos/browser-vm-target.json") &&
    setupSourceHome.includes("resolve_browser_vm_native_proxy_source") &&
    setupSourceHome.includes("validate_linux_guest_binary") &&
    setupSourceHome.includes("/opt/elastos/bin/browser-native-proxy-engine") &&
    setupSourceHome.includes("refresh_browser_vm_initrd_control_service") &&
    setupSourceHome.includes("refresh_browser_vm_rootfs_files") &&
    setupSourceHome.includes("ELASTOS_DEBUGFS_BIN") &&
    setupSourceHome.includes("debugfs") &&
    setupSourceHome.includes("ELASTOS_NODE_BIN") &&
    setupSourceHome.includes("ELASTOS_BROWSER_VM_CONTROL_LAUNCHER") &&
    setupSourceHome.includes("existing_remote_browser_vm_config") &&
    setupSourceHome.includes("preserve existing remote Browser VM control config") &&
    setupSourceHome.includes("SETUP_SOURCE_HOME_CONFIG_ONLY") &&
    setupSourceHomeBrowserConfigSmoke.includes("elastos.setup-source-home.browser-config-smoke/v1") &&
    setupSourceHomeBrowserConfigSmoke.includes("did not preserve the existing remote control socket") &&
    setupSourceHomeBrowserConfigSmoke.includes("remote VZ setup must not inherit local crosvm rootfs pool env") &&
    setupSourceHomeBrowserConfigSmoke.includes("remote VZ setup must not inherit local") &&
    !setupSourceHome.includes("ELASTOS_BROWSER_ENGINE_MODE") &&
    !setupSourceHome.includes("ELASTOS_BROWSER_ALLOW_INSECURE_HOSTED_PROOF") &&
    !setupSourceHome.includes("--mac-supervisor") &&
    !setupSourceHome.includes("browser-mac-container-supervisor") &&
    !setupSourceHome.includes("browser-per-launch-selkies-supervisor"),
  "Source-home Browser setup must be VM-only and must not expose hosted Browser runtimes as product config",
);

assert(
  gatewayHomeSystemTests.includes('"display_modes": ["webrtc_remote_display"]') &&
    !gatewayHomeSystemTests.includes('"runtime_frame"') &&
    !gatewayHomeSystemTests.includes('"diagnostic_frame"'),
  "Home/System Browser fixtures must not advertise removed Browser display modes",
);

assert(
  !exists("scripts/browser-mac-container-preflight.sh") &&
    !exists("scripts/browser-mac-container-supervisor.mjs") &&
    !browserEngineAdapter.includes("per_launch_mac_container_target") &&
    !browserEngineAdapter.includes("cleanup_mac_container_session") &&
    !browserEngineAdapter.includes("apple_container_selkies_webrtc"),
  "Removed Mac container Browser product path must not remain wired into scripts or adapter code",
);

assert(
  browserVmRemoteVzLauncher.includes("ELASTOS_BROWSER_REMOTE_VZ_SSH") &&
    browserVmRemoteVzLauncher.includes("ELASTOS_BROWSER_REMOTE_VZ_SSH must name") &&
    !browserVmRemoteVzLauncher.includes(["elastos", "mac", "staging"].join("-")) &&
    browserVmRemoteVzLauncher.includes("-R") &&
    !browserVmRemoteVzLauncher.includes("control-forward") &&
    !browserVmRemoteVzLauncher.includes("waitForLocalTcpPort") &&
    browserVmRemoteVzLauncher.includes("control-stdio") &&
    browserVmRemoteVzLauncher.includes("startLocalTcpToUnixBridge") &&
    browserVmRemoteVzLauncher.includes("startLocalUnixToRemoteUnixBridge") &&
    browserVmRemoteVzLauncher.includes("waitForLocalControlHttp") &&
    browserVmRemoteVzLauncher.includes("def write_all(fd, data):") &&
    browserVmRemoteVzLauncher.includes("while sent < len(view):") &&
    browserVmRemoteVzLauncher.includes("write_all(1, data)") &&
    !browserVmRemoteVzLauncher.includes("os.write(1, data)") &&
    browserVmRemoteVzLauncher.includes("startRemoteUnixToTcpBridge") &&
    !browserVmRemoteVzLauncher.includes("startRemoteTcpToUnixBridge") &&
    browserVmRemoteVzLauncher.includes("await startRemoteRelayTunnel") &&
    browserVmRemoteVzLauncher.includes("await startControlTunnel") &&
    browserVmRemoteVzLauncher.includes("ELASTOS_BROWSER_VM_ICE_SERVER") &&
    browserVmRemoteVzLauncher.includes("ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4") &&
    browserVmRemoteVzLauncher.includes("ELASTOS_BROWSER_REMOTE_VZ_PROFILE_ROOT") &&
    browserVmRemoteVzLauncher.includes("ELASTOS_BROWSER_REMOTE_VZ_TURN_ENV") &&
    browserVmRemoteVzLauncher.includes("optionalRemoteEnvExports") &&
    browserVmRemoteVzLauncher.includes("rejectLegacyVzConfiguration") &&
    browserVmRemoteVzLauncher.includes("elastos.browser.vz-transport-authority/v1") &&
    browserVmRemoteVzLauncher.includes("elastos.browser.vz-launch-settlement/v1") &&
    browserVmRemoteVzLauncher.includes("boundSocketPaths") &&
    browserVmRemoteVzLauncher.includes("validateRemoteTransportResult") &&
    browserVmRemoteVzLauncher.includes("routeAbsenceProved") &&
    browserVmRemoteVzLauncher.includes("codesign --verify --strict") &&
    browserVmRemoteVzLauncher.includes("remoteProfileDiskPath") &&
    browserVmRemoteVzLauncher.includes("BrowserProfiles/default/profile.ext4") &&
    browserVmRemoteVzLauncher.includes('ELASTOS_BROWSER_VM_CONTROL_READY_TIMEOUT_MS') &&
    browserVmRemoteVzLauncher.includes("remoteControlReadyTimeoutMs") &&
    browserVmRemoteVzLauncher.includes("launchTimeoutMs - 30_000") &&
    browserVmRemoteVzLauncher.includes("120_000") &&
    browserVmRemoteVzLauncher.includes("remoteDebugHoldOnOpenErrorMs") &&
    browserVmRemoteVzLauncher.includes("remainingLaunchMarginMs") &&
    browserVmRemoteVzLauncher.includes("launchTimeoutMs - readyTimeoutMs - 5_000") &&
    browserVmRemoteVzLauncher.includes("ELASTOS_BROWSER_VM_CONTROL_PROXY_REQUEST_TIMEOUT_MS") &&
    browserVmRemoteVzLauncher.includes('defaultControlProxyRequestTimeoutMs = "120000"') &&
    browserVmRemoteVzLauncher.includes("process.env.ELASTOS_BROWSER_VM_CONTROL_PROXY_REQUEST_TIMEOUT_MS || defaultControlProxyRequestTimeoutMs") &&
    browserVmRemoteVzLauncher.includes("ELASTOS_BROWSER_VM_CONTROL_STATUS_PROBE_TIMEOUT_MS") &&
    browserVmRemoteVzLauncher.includes("ELASTOS_BROWSER_VM_DEBUG_HOLD_ON_OPEN_ERROR_MS") &&
    browserVmRemoteVzLauncher.includes("ELASTOS_BROWSER_VM_EGRESS_MAX_SESSIONS") &&
    browserVmRemoteVzLauncher.includes('ELASTOS_BROWSER_REMOTE_VZ_RELAY_MAX_SESSIONS || "16"') &&
    browserVmRemoteVzLauncher.includes("remoteSupervisorCleanupCommand") &&
    browserVmRemoteVzLauncher.includes("--elastos-vz-binding=") &&
    browserVmRemoteVzLauncher.includes("ps -ww -axo command=") &&
    browserVmRemoteVzLauncher.includes("ps -ww -axo pid=,command=") &&
    browserVmRemoteVzLauncher.includes("terminate_owned_supervisor") &&
    browserVmRemoteVzLauncher.includes("ELASTOS_BROWSER_REMOTE_VZ_REAP_STALE_SUPERVISORS") &&
    browserVmRemoteVzLauncher.includes("ELASTOS_BROWSER_REMOTE_VZ_REAP_STALE_RELAYS") &&
    !browserVmRemoteVzLauncher.includes("remoteStaleSupervisorCleanupCommand") &&
    !browserVmRemoteVzLauncher.includes("remoteStaleRelayCleanupCommand") &&
    browserVmRemoteVzLauncher.includes("remoteTransportAbsenceChecks") &&
    browserVmRemoteVzLauncher.includes("${field} proof failed") &&
    !browserVmRemoteVzLauncher.includes("let cleanupOk") &&
    browserVmRemoteVzLauncher.includes("let cleanupPromise = null") &&
    browserVmRemoteVzLauncher.includes("performOwnedResourceCleanup") &&
    !browserVmRemoteVzLauncher.includes("if (cleanupStarted) return null") &&
    browserVmRemoteVzLauncher.includes('proc_command=$(/bin/ps -ww -p "$pid" -o command= 2>/dev/null || true)') &&
    browserVmRemoteVzLauncher.includes("supervisor-${suffix}.pid") &&
    browserVmRemoteVzLauncher.includes("cleanup_supervisor") &&
    browserVmRemoteVzLauncher.includes("remoteRelayCleanupCommand") &&
    browserVmRemoteVzLauncher.includes("relay-${suffix}.pid") &&
    browserVmRemoteVzLauncher.includes("ELASTOS_BRIDGE_PIDFILE") &&
    browserVmRemoteVzLauncher.includes("cleanup_bridge") &&
    browserVmRemoteVzLauncher.includes("ELASTOS_BROWSER_VM_TRACE_EGRESS") &&
    browserVmRemoteVzLauncher.includes("filterSupervisorStderr") &&
    browserVmRemoteVzLauncher.includes("Browser VM host egress bridge (accepted session|session)") &&
    browserVmRemoteVzLauncher.includes("errorWithSupervisorTail") &&
    browserVmRemoteVzLauncher.includes('child.kill("SIGTERM")') &&
    browserVmRemoteVzLauncher.includes("/tmp/evzl") &&
    browserVmRemoteVzLauncher.includes("/tmp/evzs") &&
    browserVmRemoteVzLauncher.includes("bindingDigest") &&
    browserVmRemoteVzLauncher.includes("bvm-") &&
    browserVmRemoteVzLauncher.includes("validateUnixSocketPathBudget") &&
    browserVmRemoteVzLauncher.includes("adapter_ipc") &&
    browserVmRemoteVzLauncher.includes("runtime_stream_path") &&
    browserVmRemoteVzLauncher.includes("control_socket_path") &&
    browserVmRemoteVzLauncher.includes("rm -f") &&
    browserVmRemoteVzLauncher.includes("per_launch_vm_target") &&
    browserVmRemoteVzLauncherIntegration.includes("private_stdin_eof") &&
    browserVmRemoteVzLauncherIntegration.includes("post_effect_cleanup") &&
    browserVmRemoteVzLauncherIntegration.includes("zero_owned_residue") &&
    browserVmRemoteVzLauncherIntegration.includes("long_roots") &&
    browserVzSupervisorProcessTest.includes(
      "missing_transport_exits_before_any_vz_or_path_effect",
    ) &&
    browserVzSupervisorProcessTest.includes('"load_vm_start"') &&
    browserVzSupervisorProcessTest.includes('"start_vm_start"') &&
    !browserVmRemoteVzLauncher.includes("remote_provider:"),
  "Remote Browser VZ launcher must bridge Runtime stream IPC and VM control sockets with short macOS-safe socket paths",
);

assert(
  browserVmEngineSupervisor.includes("ELASTOS_BROWSER_VM_CONTROL_SOCKET") &&
    browserVmEngineSupervisor.includes("Browser VM engine target is not launch-ready") &&
    browserVmEngineSupervisor.includes("remote/operator VM provider") &&
    browserVmEngineSupervisor.includes("browser-vm-local-crosvm-launcher") &&
    browserVmEngineSupervisor.includes("sanitizedVmControlServiceEnv") &&
    browserVmEngineSupervisor.includes("delete env[REQUEST_ENV]") &&
    browserVmEngineSupervisor.includes('path.join(dataDir, "bvm")') &&
    browserVmEngineSupervisor.includes("/tmp/evzs") &&
    !browserVmEngineSupervisor.includes("function sessionSuffix") &&
    !browserVmEngineSupervisor.includes("fs.mkdirSync(sessionDir") &&
    browserVmEngineSupervisor.includes("Browser VM control isolation session_dir") &&
    browserVmEngineSupervisor.includes("elastos.browser.vm-engine.open/v1") &&
    browserVmEngineSupervisor.includes("chromium_microvm") &&
    browserVmEngineSupervisor.includes("per_launch_vm_target") &&
    browserVmEngineSupervisor.includes("validateBrowserProfileDescriptor") &&
    browserVmEngineSupervisor.includes("principal_owned_profile_disk") &&
    browserVmEngineSupervisor.includes("principal_owned_reset_scoped_unprotected") &&
    browserVmEngineSupervisor.includes("protected_storage !== false") &&
    browserVmEngineSupervisor.includes("BrowserProfiles/default/profile.ext4") &&
    !browserVmEngineSupervisor.includes("PROFILE_ROOT_ENV") &&
    !browserVmEngineSupervisor.includes("profileDirForRequest") &&
    !browserVmEngineSupervisor.includes("`principal-${digest}`") &&
    browserVmEngineSupervisor.includes("Browser VM supervisor requires webrtc_remote_display") &&
    browserVmEngineSupervisor.includes("Browser VM display sessions must report media_transport=runtime_relay") &&
    browserVmEngineSupervisor.includes("isRemoteVzControlLauncher") &&
    browserVmEngineSupervisor.includes("applyRemoteVzControlDefaults") &&
    browserVmEngineSupervisor.includes("ELASTOS_BROWSER_REMOTE_VZ_LAUNCH_TIMEOUT_MS") &&
    browserVmEngineSupervisor.includes("ELASTOS_BROWSER_REMOTE_VZ_") &&
    browserVmEngineSupervisor.includes("remote_vz_launch_timeout_ms") &&
    browserVmEngineSupervisor.includes("launchTimeoutMs - 30000") &&
    browserVmEngineSupervisor.includes("runtime_net_only") &&
    browserVmEngineSupervisor.includes("direct_network !== false") &&
    browserVmEnginePreflight.includes("elastos.browser.vm-engine-preflight/v1") &&
    browserVmEnginePreflight.includes("remote_control_supported") &&
    browserVmEnginePreflight.includes("control_socket_status") &&
    browserVmEnginePreflight.includes("GET /status HTTP/1.1") &&
    browserVmEnginePreflight.includes("missing_for_local_substrate") &&
    browserVmEnginePreflight.includes("Local crosvm Browser VM is unavailable because /dev/kvm is missing") &&
    browserVmEnginePreflight.includes("apple_virtualization_framework") &&
    browserVmEnginePreflight.includes('"kernel": stat(kernel)') &&
    browserVmEnginePreflight.includes("crosvm") &&
    browserVmArtifactPreflight.includes("elastos.browser.vm-artifact-preflight/v1") &&
    browserVmArtifactPreflight.includes("inspect_ext4_sidecar_manifest") &&
    browserVmArtifactPreflight.includes("local_substrate_artifacts_ready") &&
    browserVmArtifactPreflight.includes("browser-vm-selkies-start") &&
    browserVmArtifactPreflight.includes("browser-vm-guest-control-bridge") &&
    browserVmArtifactPreflight.includes("AUDIO_ROOTFS_FILES") &&
    browserVmArtifactPreflight.includes("audio_default_ready") &&
    browserVmArtifactPreflight.includes("rootfs manifest target preflight reports audio_default_ready=false") &&
    browserVmArtifactPreflight.includes('"pipewire": "/usr/bin/pipewire"') &&
    browserVmArtifactPreflight.includes('"pipewire_pulse": "/usr/bin/pipewire-pulse"') &&
    browserVmArtifactPreflight.includes('"wireplumber": "/usr/bin/wireplumber"') &&
    browserVmArtifactPreflight.includes('"pw_cli": "/usr/bin/pw-cli"') &&
    browserVmArtifactPreflight.includes("FORBIDDEN_INIT_SNIPPETS") &&
    browserVmArtifactPreflight.includes("must not reference stale console discovery") &&
    browserVmArtifactPreflight.includes("apple_virtualization_framework") &&
    browserVmArtifactPreflightSmoke.includes("elastos.browser.vm-artifact-preflight-smoke/v1") &&
    browserVmArtifactPreflightSmoke.includes("ELASTOS_BROWSER_VM_STAGED_ROOTFS") &&
    browserVmControlService.includes("elastos.browser.vm-control-service.config/v1") &&
    browserVmControlService.includes("Browser VM control service accepts only chromium_microvm") &&
    browserVmControlService.includes("Browser VM control service requires webrtc_remote_display") &&
    browserVmControlService.includes('const expectedMediaTransport = "runtime_relay"') &&
    browserVmControlService.includes("Browser VM display sessions must report media_transport=${expectedMediaTransport}") &&
    browserVmControlService.includes("Browser VM product display sessions must advertise audio=true and video=true") &&
    browserVmControlService.includes("Browser VM product display sessions must include an audio WebRTC offer") &&
    browserVmEngineSupervisor.includes("launcher: controlServiceArtifactFingerprints(config.launcher_program)") &&
    browserVmControlService.includes("persistent_launcher") &&
    browserVmControlService.includes("runPersistentProgram") &&
    browserVmControlService.includes("terminatePersistentLauncher") &&
    !browserVmControlService.includes("sameLaunchIdentity") &&
    !browserVmControlService.includes("launch_replacing") &&
    browserVmControlService.includes("reuse_idle_vms") &&
    browserVmControlService.includes("idleVmReuseEnabled") &&
    browserVmControlService.includes("idle_vm_reuse_disabled_retired") &&
    browserVmControlService.includes("retireNonReusableIdleVmsForSinglePageRuntime") &&
    browserVmControlService.includes("single_active_page_non_reusable_profile") &&
    browserVmControlService.includes("max_active_pages") &&
    browserVmControlService.includes("Browser VM active page capacity reached") &&
    browserVmControlService.includes("page_close_forced_vm_retirement") &&
    browserVmControlService.includes("per_launch_vm_target") &&
    browserVmControlService.includes("elastos.browser.vz-launch-settlement/v1") &&
    browserVmControlService.includes("validateVzLaunchSettlementForLaunch") &&
    browserVmControlService.includes("throw settledError") &&
    browserVmControlService.includes("launch_settlement_result") &&
    browserVmControlServiceSmoke.includes("elastos.browser.vm-control-service-smoke/v1") &&
    browserVmControlServiceSmoke.includes("capacity conflict changed the healthy owner") &&
    browserVmControlServiceSmoke.includes("explicit close did not permit the next lifecycle") &&
    browserVmControlServicePersistentSmoke.includes("elastos.browser.vm-control-service-persistent-smoke/v1") &&
    browserVmControlServicePersistentSmoke.includes("completed replay changed the healthy owner") &&
    browserVmControlServicePersistentSmoke.includes("failed guest close did not force terminal VM retirement") &&
    browserVmControlServicePersistentSmoke.includes("wrong single-page persistent status") &&
    browserVmControlServicePersistentSmoke.includes("fake-invalid-persistent-vm-launcher") &&
    browserVmControlServicePersistentSmoke.includes("terminal cleanup proof was incomplete") &&
    browserVmControlServicePersistentSmoke.includes("same-profile route change reused a terminally closed VM control socket") &&
    browserVmControlServicePersistentSmoke.includes("different principal/profile launch did not terminate the previous idle VM") &&
    browserVmControlServicePersistentSmoke.includes('"reuse_idle_vms": True') &&
    browserVmControlServicePersistentSmoke.includes("Browser VM launcher output is not JSON") &&
    browserVmControlServiceSettlementSmoke.includes("elastos.browser.vm-control-service-settlement-smoke/v1") &&
    browserVmControlServiceSettlementSmoke.includes("verify-typed-restart") &&
    browserVmControlServiceSettlementSmoke.includes("did_not_act cleanup_pending terminal_post_effect_cleanup") &&
    browserVmControlServiceSettlementSmoke.includes("substituted transport settlement escaped cleanup ownership") &&
    browserVmEngineContractSmoke.includes("elastos.browser.vm-engine-contract-smoke/v1") &&
    browserVmRemoteControlPreflightSmoke.includes("elastos.browser.vm-remote-control-preflight-smoke/v1") &&
    browserVmRemoteControlPreflightSmoke.includes("remote_vm_control_socket") &&
    browserVmRemoteControlPreflightSmoke.includes("stale remote control socket must fail closed") &&
    browserVmEngineContractSmoke.includes("browser-vm-product") &&
    !browserVmEngineContractSmoke.includes("--engine-mode") &&
    browserVmEngineContractSmoke.includes("vm_selkies_gstreamer_webrtc") &&
    browserVmEngineContractSmoke.includes("media_transport: \"runtime_relay\"") &&
    browserVmTargetPreflight.includes("elastos.browser.vm-target-preflight/v1") &&
    browserVmTargetPreflight.includes("Additional --require-runtime-deps contract") &&
    browserVmTargetPreflight.includes("audio_default_ready") &&
    browserVmTargetPreflight.includes("target and missing audio support fails this preflight") &&
    browserVmTargetPreflight.includes("browser-vm-runtime-relay") &&
    browserVmTargetPreflight.includes("browser-vm-guest-control-bridge") &&
    browserVmTargetPreflight.includes("elastos.browser.vm-guest-control-bridge.config/v1") &&
    browserVmTargetPreflight.includes("control_socket_ready_timeout_ms") &&
    browserVmTargetPreflight.includes("control_request_timeout_ms") &&
    browserVmTargetPreflight.includes("browser-vm-selkies-start") &&
    browserVmTargetPreflight.includes("ELASTOS_BROWSER_SELKIES_CONTROL_CONFIG") &&
    browserVmTargetPreflight.includes("ELASTOS_BROWSER_VM_CONTROL_BRIDGE_CONFIG") &&
    browserVmTargetPreflight.includes("optional_audio = {") &&
    browserVmTargetPreflight.includes("audio_default_ready = None") &&
    browserVmTargetPreflight.includes("missing.append(name)") &&
    browserVmTargetPreflight.includes('"pipewire": first_present') &&
    browserVmTargetPreflight.includes('"pipewire_pulse": first_present') &&
    browserVmTargetPreflight.includes('"wireplumber": first_present') &&
    browserVmTargetPreflight.includes('"pw_cli": first_present') &&
    browserVmTargetPreflight.includes("rootfs_checkpoint()") &&
    browserVmTargetPreflight.includes("selkies_checkpoint()") &&
    browserVmTargetPreflight.includes("ELASTOS_BROWSER_VM_ICE_SERVERS_JSON") &&
    browserVmTargetPreflight.includes("/run/elastos/browser-rtc.json") &&
    browserVmTargetPreflight.includes("/run/elastos/browser-ice-servers.json") &&
    browserVmTargetPreflight.includes("/run/elastos/browser-ice-transport-policy") &&
    browserVmTargetPreflight.includes("/run/elastos/browser-media-relay-network.json") &&
    browserVmTargetPreflight.includes("ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4") &&
    browserVmTargetPreflight.includes("patch_selkies_relay_policy") &&
    browserVmTargetPreflight.includes("ice-transport-policy") &&
    browserVmTargetPreflight.includes("script_errors") &&
    browserVmTargetPreflight.includes("runtime_exit_transport must be carrier_stream or vsock_relay") &&
    browserVmTargetPreflight.includes("control_transport must be vsock_relay") &&
    browserVmTargetPreflight.includes("--host-resolver-rules=MAP * ~NOTFOUND") &&
    browserVmTargetPreflight.includes("must not reference stale console discovery") &&
    browserVmTargetPreflightSmoke.includes("elastos.browser.vm-target-preflight-smoke/v1") &&
    browserVmTargetRefresh.includes("--guest-control-bridge-bin") &&
    browserVmTargetRefresh.includes("ELASTOS_BROWSER_VM_GUEST_CONTROL_BRIDGE_BIN") &&
    browserVmTargetRefresh.includes("/opt/elastos/bin/browser-vm-guest-control-bridge") &&
    browserVmTargetRefresh.includes("verify_rootfs_guest_control_bridge_contract") &&
    browserVmTargetRefresh.includes("rootfs guest-control bridge is stale") &&
    browserVmTargetRefresh.includes("/opt/elastos/bin/browser-vm-init") &&
    browserVmTargetRefresh.includes("extract_browser_vm_init") &&
    browserVmRuntimeRelay.includes("elastos.browser.vm-runtime-relay.config/v1") &&
    browserVmRuntimeRelay.includes("browser VM runtime relay must not grant direct network") &&
    browserVmRuntimeRelay.includes("VsockListen") &&
    browserVmRuntimeRelaySmoke.includes("elastos.browser.vm-runtime-relay-smoke/v1") &&
    browserVmGuestControlBridge.includes("elastos.browser.vm-guest-control-bridge.config/v1") &&
    browserVmGuestControlBridge.includes("browser VM guest control bridge must not grant direct network") &&
    browserVmGuestControlBridge.includes("control_socket_ready_timeout_ms") &&
    browserVmGuestControlBridge.includes("control_request_timeout_ms") &&
    browserVmGuestControlBridge.includes("DEFAULT_CONTROL_SOCKET_READY_TIMEOUT_MS") &&
    browserVmGuestControlBridge.includes("DEFAULT_CONTROL_REQUEST_TIMEOUT_MS") &&
    browserVmGuestControlBridge.includes("connect_guest_control_socket") &&
    browserVmGuestControlBridge.includes("wait_for_readable") &&
    browserVmGuestControlBridge.includes("VsockListen") &&
    browserVmGuestControlBridgeSmoke.includes("elastos.browser.vm-guest-control-bridge-smoke/v1") &&
    browserVmGuestControlBridgeSmoke.includes("control_socket_ready_timeout_ms") &&
    browserVmGuestControlBridgeSmoke.includes("control_request_timeout_ms") &&
    browserVzEngineSupervisor.includes("browser-vz-engine-supervisor requires macOS arm64") &&
    browserVzEngineSupervisor.includes("init=/opt/elastos/bin/browser-vm-init") &&
    browserVzEngineSupervisor.includes("root=/dev/vda rootfstype=ext4 rw") &&
    browserVzEngineSupervisor.includes("DEFAULT_CONTROL_PORT: u32 = 19092") &&
    !browserVzEngineSupervisor.includes("DEFAULT_RELAY_PORT") &&
    browserVzEngineSupervisor.includes("DEFAULT_CONTROL_PROXY_REQUEST_TIMEOUT_MS") &&
    browserVzEngineSupervisor.includes("ELASTOS_BROWSER_VM_CONTROL_PROXY_REQUEST_TIMEOUT_MS") &&
    !browserVzEngineSupervisor.includes("DEFAULT_CONTROL_STATUS_PROBE_TIMEOUT_MS") &&
    !browserVzEngineSupervisor.includes("probe_guest_control_status") &&
    !browserVzEngineSupervisor.includes("probe_guest_control_events") &&
    !browserVzEngineSupervisor.includes("guest control status probe") &&
    !browserVzEngineSupervisor.includes("guest control events probe") &&
    browserVzEngineSupervisor.includes("UNIX_SOCKET_PATH_BUDGET") &&
    browserVzEngineSupervisor.includes("validate_unix_socket_path_budget") &&
    browserVzEngineSupervisor.includes("/tmp/evzs") &&
    browserVzEngineSupervisor.includes("Browser VZ launcher requires adapter_ipc.runtime_stream_path") &&
    browserVzEngineSupervisor.includes("validate_runtime_stream_socket_path") &&
    browserVzEngineSupervisor.includes("launch_requires_runtime_owned_stream_path_for_egress") &&
    browserVzEngineSupervisor.includes("Browser VZ launcher requires display_mode=webrtc_remote_display") &&
    browserVzEngineSupervisor.includes("VzTransportLaunch") &&
    browserVzEngineSupervisor.includes("LEGACY_VZ_CONFIGURATION_KEYS") &&
    browserVzEngineSupervisor.includes("VZ_AUTHORITY_BOOT_ARG_PREFIXES") &&
    browserVzEngineSupervisor.includes("validate_vz_boot_args") &&
    browserVzEngineSupervisor.includes("preflight_vz_launch") &&
    browserVzEngineSupervisor.includes("bound_vz_launch_paths") &&
    browserVzEngineSupervisor.includes("VzLaunchOwner") &&
    browserVzEngineSupervisor.includes("TurnCleanupEvidence") &&
    browserVzEngineSupervisor.includes("LaunchTurnStartError") &&
    browserVzEngineSupervisor.includes("child_absent: self.terminate_and_reap()") &&
    browserVzEngineSupervisor.includes("TurnCleanupEvidence::Indeterminate => false") &&
    browserVzEngineSupervisor.includes("elastos.browser.vz-launch-settlement/v1") &&
    browserVzEngineSupervisor.includes("network_disabled: true") &&
    browserVzEngineSupervisor.includes("elastos.browser_ice_config_hex=") &&
    browserVzEngineSupervisor.includes("elastos.browser_width") &&
    browserVzEngineSupervisor.includes("elastos.browser_height") &&
    browserVzEngineSupervisor.includes("PRODUCT_STREAM_WIDTH: u64 = 1920") &&
    browserVzEngineSupervisor.includes("PRODUCT_STREAM_HEIGHT: u64 = 1080") &&
    browserVzEngineSupervisor.includes("display_boot_args_use_fixed_1080p_product_stream") &&
    browserVzEngineSupervisor.includes("engine=chromium_microvm") &&
    browserVzEngineSupervisor.includes("selkies_gstreamer") &&
    browserVzEngineSupervisor.includes("media_transport") &&
    browserVzEngineSupervisor.includes("runtime_relay") &&
    browserVzEngineSupervisor.includes("normalize_display_media_from_offer") &&
    browserVzEngineSupervisor.includes("sdp_has_media_kind") &&
    browserVzEngineSupervisor.includes("vm_selkies_gstreamer_webrtc") &&
    browserVzEngineSupervisor.includes("per_launch_vm_target") &&
    browserVzEngineSupervisor.includes("const BROWSER_VM_TARGET_VERSION") &&
    browserVzEngineSupervisor.includes('option_env!("ELASTOS_RELEASE_VERSION")') &&
    browserVzEngineSupervisor.includes('concat!(env!("CARGO_PKG_VERSION"), "-dev")') &&
    !browserVzEngineSupervisor.includes('version: "0.4.1".to_string()') &&
    browserVzEngineSupervisor.includes("DEFAULT_PROFILE_DISK_MIB") &&
    !browserVzEngineSupervisor.includes("ELASTOS_BROWSER_VM_PROFILE_DISK_ROOT") &&
    browserVzEngineSupervisor.includes("ELASTOS_BROWSER_VM_PROFILE_DISK_MIB") &&
    browserVzEngineSupervisor.includes('"ELASTOS_BROWSER_VM_EGRESS_MAX_SESSIONS"') &&
    browserVzEngineSupervisor.includes("post_effect_try!(env_u32(") &&
    browserVzEngineSupervisor.includes("attach_browser_profile_disk") &&
    browserVzEngineSupervisor.includes("profile_disk_from_request") &&
    browserVzEngineSupervisor.includes("validate_profile_disk_path") &&
    browserVzEngineSupervisor.includes("data_disk_path = Some") &&
    browserVzEngineSupervisor.includes("elastos.browser_profile_disk=required") &&
    browserVzEngineSupervisor.includes("browser_profile_uses_principal_owned_data_disk_descriptor") &&
    !browserVzEngineSupervisor.includes("BrowserVmHibernation") &&
    !browserVzEngineSupervisor.includes("discard_bad_hibernation_state") &&
    !browserVzEngineSupervisor.includes("discard_hibernation_tmp_state") &&
    browserVzEngineSupervisor.includes('#[cfg(target_os = "macos")]') &&
    browserVmTargetStage.includes("elastos.browser.vm-target-stage/v1") &&
    browserVmTargetStage.includes("browser-vm-init") &&
    browserVmTargetStage.includes("browser-vm-selkies-start") &&
    browserVmTargetStage.includes("selkies_checkpoint()") &&
    browserVmTargetStage.includes("profile initialized") &&
    browserVmTargetStage.includes("dependencies checked") &&
    browserVmTargetStage.includes("PipeWire is required for Browser audio") &&
    browserVmTargetStage.includes("pipewire-pulse is required for Browser audio") &&
    browserVmTargetStage.includes("WirePlumber is required for Browser audio") &&
    browserVmTargetStage.includes("pw-cli is required for Browser audio") &&
    browserVmTargetStage.includes("configure_browser_wireplumber_headless") &&
    browserVmTargetStage.includes("browser-vm-wireplumber-config.log") &&
    browserVmTargetStage.includes('alsa_monitor.properties["alsa.reserve"] = false') &&
    browserVmTargetStage.includes('bluez_monitor.properties["with-logind"] = false') &&
    browserVmTargetStage.includes("support.logind = disabled") &&
    browserVmTargetStage.includes('pulsesrc.set_property("device", "auto_null.monitor")') &&
    !browserVmTargetStage.includes("audio unavailable; continuing with video-only display") &&
    browserVmRootfsBuild.includes("pipewire-pulse") &&
    browserVmRootfsBuild.includes('pulsesrc.set_property("device", "auto_null.monitor")') &&
    browserVmRootfsBuild.includes("gst-inspect-1.0 pulsesrc") &&
    browserVmRootfsBuild.includes("forcing audio SDP offer for split product audio peer") &&
    !browserVmTargetStage.includes("console_device") &&
    !browserVmTargetStage.includes('console=*)') &&
    !browserVmTargetStage.includes("/dev/hvc0 /dev/ttyS0 /dev/console") &&
    browserVmTargetStage.includes("virtio_console") &&
    browserVmRootfsBuild.includes("virtio_console") &&
    browserVmTargetStage.includes("cmdline_value elastos.browser_profile") &&
    browserVmTargetStage.includes("cmdline_value elastos.browser_profile_disk") &&
    browserVmTargetStage.includes("/dev/vdb") &&
    browserVmTargetStage.includes("mount_browser_profile_disk") &&
    browserVmTargetStage.includes("principal-owned Browser profile disk is required but $disk is missing") &&
    browserVmTargetStage.includes('mount_dir="/var/lib/elastos/browser-profile-disk"') &&
    browserVmTargetStage.includes('ELASTOS_BROWSER_VM_PROFILE_DIR="$mount_dir/profiles/$key"') &&
    browserVmTargetStage.includes("browser-vm-guest-control-bridge") &&
    browserVmTargetStage.includes("validate_linux_guest_binary") &&
    browserVmTargetStage.includes("--target-platform") &&
    browserVmTargetStage.includes("selkies-gstreamer") &&
    browserVmTargetStage.includes("python3 -m selkies_gstreamer") &&
    browserVmTargetStage.includes("--web_root=/opt/gst-web") &&
    browserVmTargetStage.includes('ELASTOS_BROWSER_VM_SELKIES_ENCODER:=openh264enc') &&
    browserVmTargetStage.includes('--encoder="$ELASTOS_BROWSER_VM_SELKIES_ENCODER"') &&
    !browserVmTargetStage.includes("--encoder=x264enc") &&
    browserVmTargetStage.includes("ELASTOS_BROWSER_VM_ICE_SERVER") &&
    browserVmTargetStage.includes("ELASTOS_BROWSER_VM_ICE_SERVERS_JSON") &&
    browserVmTargetStage.includes("ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY") &&
    browserVmTargetStage.includes("ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4") &&
    browserVmTargetStage.includes("browser-media-relay-network.json") &&
    browserVmTargetStage.includes("media relay IPv4") &&
    browserVmTargetStage.includes("elastos.browser_ice_config_hex") &&
    browserVmTargetStage.includes("cmdline_value elastos.browser_width") &&
    browserVmTargetStage.includes("validate_display_dimension") &&
    browserVmTargetStage.includes("setup_media_relay_network") &&
    browserVmTargetStage.includes("virtio_net") &&
    browserVmTargetStage.includes("found_media_iface") &&
    browserVmTargetStage.includes('[ -n "$found_media_iface" ] && break') &&
    browserVmTargetStage.includes("patch_selkies_relay_policy") &&
    browserVmTargetStage.includes("_elastos_raw_caps_with_framerate") &&
    browserVmTargetStage.includes("stale Selkies Gst.Fraction constructor remains") &&
    browserVmTargetStage.includes("/run/elastos/browser-ice-transport-policy") &&
    browserVmTargetStage.includes("ice-transport-policy") &&
    browserVmTargetStage.includes("elastos_ice_transport_policy") &&
    browserVmTargetStage.includes("confirmed ICE transport policy after TURN setup") &&
    browserVmTargetStage.includes("_elastos_turn_transport_query") &&
    browserVmTargetStage.includes("urllib.parse.parse_qs") &&
    browserVmTargetStage.includes('"turn://%s:%s@%s:%s%s"') &&
    browserVmTargetStage.includes('get_property("ice-agent")') &&
    browserVmTargetStage.includes('ice_agent.emit("add-local-ip-address", "127.0.0.1")') &&
    browserVmTargetStage.includes("emitting ICE candidate") &&
    !browserVmTargetStage.includes("browser-vm-selkies-start: ICE config follows") &&
    !browserVmTargetStage.includes("cat /run/elastos/browser-rtc.json") &&
    browserVmTargetStage.includes("ip addr show dev") &&
    browserVmTargetStage.includes("webrtc_remote_display requires at least one turn:/turns:") &&
    browserVmTargetStage.includes("browser-ice-servers.json") &&
    browserVmTargetStage.includes('"ice_servers": $(cat /run/elastos/browser-ice-servers.json)') &&
    browserVmTargetStage.includes("mount_if_needed proc proc /proc") &&
    browserVmTargetStage.includes("--no-sandbox") &&
    browserVmTargetStage.includes('"--window-size=${ELASTOS_BROWSER_VM_WIDTH},${ELASTOS_BROWSER_VM_HEIGHT}"') &&
    !browserVmTargetStage.includes("--force-device-scale-factor") &&
    browserVmTargetStage.includes('"css_width": ${ELASTOS_BROWSER_VM_WIDTH}') &&
    browserVmTargetStage.includes('"css_height": ${ELASTOS_BROWSER_VM_HEIGHT}') &&
    browserVmTargetStage.includes("(async () =>") &&
    browserVmTargetStage.includes("runtime_net_only") &&
    browserVmTargetStage.includes("ELASTOS_BROWSER_VM_RELAY_PORT") &&
    browserVmTargetStage.includes("ELASTOS_BROWSER_VM_CONTROL_BRIDGE_PORT") &&
    browserVmTargetStage.includes("rootfs_mark()") &&
    browserVmTargetStage.includes("rootfs_checkpoint()") &&
    browserVmTargetStage.includes("browser-vm-rootfs-entry.log") &&
    browserVmTargetStage.includes("entered rootfs init") &&
    browserVmTargetStage.includes("opening main init log") &&
    browserVmTargetStage.includes("rootfs diagnostics initialized") &&
    browserVmTargetStage.includes("browser control socket present") &&
    browserVmTargetStage.includes("guest control bridge started") &&
    browserVmTargetStage.includes("mount -t proc proc /proc") &&
    !browserVmTargetStage.includes('[ -w "/dev/$console_device" ] &&') &&
    !browserVmTargetStage.includes('>"$ELASTOS_BROWSER_VM_SERIAL_LOG_DEV"') &&
    browserVmTargetStage.includes('"control_socket_ready_timeout_ms": 60000') &&
    browserVmTargetStage.includes('"control_request_timeout_ms": 120000') &&
    !browserVmTargetStage.includes("--proxy-bypass-list=<-loopback>") &&
    browserVmRootfsBuild.includes("elastos.browser.vm-rootfs-build/v1") &&
    browserVmRootfsBuild.includes("debootstrap") &&
    browserVmRootfsBuild.includes("elastos-tiny-initrd") &&
    browserVmRootfsBuild.includes('exec /usr/bin/chromium "\\$@"') &&
    browserVmRootfsBuild.includes('exec /opt/elastos/bin/chromium.real "\\$@"') &&
    browserVmRootfsBuild.includes("browser-vm-initrd") &&
    browserVmRootfsBuild.includes("require_mounts_clean") &&
    browserVmRootfsBuild.includes("rootfs pseudo-filesystem still mounted") &&
    browserVmRootfsBuild.includes("initrd_dump_diagnostics") &&
    browserVmRootfsBuild.includes("_elastos_raw_caps_with_framerate") &&
    browserVmRootfsBuild.includes("Selkies stale Gst.Fraction constructor remains") &&
    browserVmRootfsBuild.includes("browser-vm-initrd.log") &&
    browserVmRootfsBuild.includes("tail dmesg sync chmod") &&
    browserVmRootfsBuild.includes("initrd_mark_newroot") &&
    browserVmRootfsBuild.includes("mounted /dev/vda on /newroot") &&
    browserVmRootfsBuild.includes("post-mount compatibility patch complete") &&
    browserVmRootfsBuild.includes("exec switch_root to /opt/elastos/bin/browser-vm-init") &&
    browserVmRootfsBuild.includes("exec switch_root /newroot /opt/elastos/bin/browser-vm-init >>/newroot/var/log/elastos/browser-vm-initrd.log 2>&1") &&
    browserVmRootfsBuild.includes("block device /dev/vda did not appear") &&
    browserVmRootfsBuild.includes("exec switch_root failed to start with status") &&
    browserVmRootfsBuild.includes("builder\": \"debootstrap\"") &&
    browserVmRootfsBuild.includes("package-time update-initramfs skipped") &&
    browserVmRootfsBuild.includes("dpkg-divert --quiet --local --add --rename") &&
    browserVmRootfsBuild.includes("chromium") &&
    browserVmRootfsBuild.includes("selkies-gstreamer-web_v") &&
    browserVmRootfsBuild.includes("elastos_ice_transport_policy") &&
    browserVmRootfsBuild.includes("ice-transport-policy") &&
    browserVmRootfsBuild.includes("confirmed ICE transport policy after TURN setup") &&
    browserVmRootfsBuild.includes("emitting ICE candidate") &&
    browserVmRootfsBuild.includes("virtio_net") &&
    browserVmRootfsBuild.includes("Selkies must apply ElastOS relay-only ICE policy") &&
    browserVmRootfsBuild.includes("/opt/gst-web/index.html") &&
    browserVmRootfsBuild.includes("python3 -m pip install") &&
    browserVmRootfsBuild.includes("linux-libc-dev") &&
    browserVmRootfsBuild.includes("mke2fs -q -t ext4") &&
    !browserVmRootfsBuild.includes("docker ") &&
    !browserVmRootfsBuild.includes("Docker is used here only as an") &&
    browserVmTargetStageSmoke.includes("elastos.browser.vm-target-stage-smoke/v1") &&
    browserVmTargetStageSmoke.includes("elastos.browser_profile_disk") &&
    browserVmTargetStageSmoke.includes("/dev/vdb") &&
    browserVmTargetStageSmoke.includes('--user-data-dir=${ELASTOS_BROWSER_VM_PROFILE_DIR}') &&
    browserVmTargetStageSmoke.includes("ELASTOS_BROWSER_VM_SELKIES_ENCODER:=openh264enc") &&
    browserVmTargetStageSmoke.includes('--encoder="$ELASTOS_BROWSER_VM_SELKIES_ENCODER"') &&
    browserVmTargetDoc.includes("Browser product target is a per-launch VM") &&
    browserVmTargetDoc.includes("Selkies is the in-guest display/input transport, not the isolation boundary") &&
    browserVmTargetDoc.includes("The Linux guest contract is the portable layer across Linux and macOS") &&
    browserVmTargetDoc.includes("The source-home Browser display path is WebRTC-only") &&
    browserVmTargetDoc.includes("`ELASTOS_BROWSER_VM_SELKIES_ENCODER` defaults to `openh264enc`") &&
    browserVmTargetDoc.includes("Runtime-frame display is not a source-home") &&
    browserVmTargetDoc.includes("does not automatically start a hidden Browser VM") &&
    browserVmTargetDoc.includes("warm sessions must be Runtime/provider-owned") &&
    browserVmTargetDoc.includes("Cold-booting a brand-new") &&
    browserVmTargetDoc.includes("is not the desired product") &&
    browserVmTargetDoc.includes("build-browser-vm-rootfs.sh") &&
    browserVmTargetDoc.includes("full bootable rootfs must also pass runtime dependency mode") &&
    browserVmTargetDoc.includes("--target-dir /path/to/full-rootfs --require-runtime-deps") &&
    browserVmTargetDoc.includes("PipeWire, PipeWire Pulse, WirePlumber") &&
    browserVmTargetDoc.includes("Refresh-only is not") &&
    browserVmTargetDoc.includes("sufficient for package/dependency changes") &&
    browserVmTargetDoc.includes("browser-vm-runtime-relay") &&
    browserVmTargetDoc.includes("browser-vm-guest-control-bridge") &&
    browserVmTargetDoc.includes("browser-vm-selkies-start") &&
    browserVmTargetDoc.includes("rejects host binaries") &&
    browserVmTargetDoc.includes("stage-browser-vm-target.sh") &&
    browserVmTargetDoc.includes("media_transport=runtime_relay") &&
    browserVmTargetDoc.includes("active-principal `localhost://Users/<root>/BrowserProfiles/default/profile.ext4`") &&
    browserVmTargetDoc.includes("principal-owned persistent ext4") &&
    browserVmTargetDoc.includes("Current H038 boundary") &&
    browserVmTargetDoc.includes("not yet protected principal-root object storage") &&
    browserVmTargetDoc.includes("not a claim that Chromium cookies") &&
    browserVmTargetDoc.includes("storage_posture=principal_owned_reset_scoped_unprotected") &&
    browserVmTargetDoc.includes("protected_storage=false") &&
    browserCapsuleDoc.includes("0.5.0 truth boundary") &&
    browserCapsuleDoc.includes("not yet a protected principal-root") &&
    browserCapsuleDoc.includes("object envelope") &&
    browserCapsuleDoc.includes("not yet exported/imported by Recovery Kit") &&
    browserCapsuleDoc.includes("not be described as encrypted/recoverable") &&
    browserCapsuleDoc.includes("storage_posture=principal_owned_reset_scoped_unprotected") &&
    !browserCapsuleDoc.includes("recoverable/migratable") &&
    state.includes("Browser VM Chromium profile disks are principal-owned and reset-scoped") &&
    state.includes("not protected principal-root envelopes or Recovery Kit-packaged state yet") &&
    state.includes("storage_posture=principal_owned_reset_scoped_unprotected") &&
    state.includes("this does not include Browser VM Chromium profile disks yet") &&
    architectureDoc.includes("Home shell browser state") &&
    architectureDoc.includes("not Browser VM Chromium profile disks") &&
    browserVmTargetDoc.includes("POST /api/apps/browser/profile/reset") &&
    installDoc.includes("Browser VM target maintenance is an operator path") &&
    installDoc.includes("Refresh-only is not sufficient for package/dependency changes") &&
    installDoc.includes("scripts/browser-vm-artifact-preflight.sh") &&
    installDoc.includes("PipeWire/WirePlumber/GStreamer dependency set") &&
    elastosCommon.includes("browser_profile_key_from_value") &&
    elastosCommon.includes("Sha256::digest") &&
    elastosCommon.includes('"profile-{}"') &&
    elastosCommon.includes("is_safe_browser_profile_key") &&
    !elastosCommon.includes(".file_name()") &&
    gatewayApi.includes("/api/apps/browser/profile/reset") &&
    gatewayBrowserApi.includes("browser_app_profile_reset") &&
    gatewayBrowserApi.includes("browser_principal_has_live_sessions") &&
    gatewayBrowserApi.includes("principal_owned_profile_disk") &&
    gatewayBrowserApi.includes("BROWSER_PROFILE_STORAGE_POSTURE") &&
    gatewayBrowserApi.includes('"protected_storage": false') &&
    browserProfileResetRoute.includes('"scope": "active_principal"') &&
    !browserProfileResetRoute.includes('"profile_key": profile_key') &&
    !browserProfileResetRoute.includes('"principal_id": context.principal_id') &&
    gatewayBrowserProfileTests.includes("browser_profile_reset_removes_only_principal_profile_disk") &&
    gatewayBrowserProfileTests.includes("principal_owned_reset_scoped_unprotected") &&
    gatewayBrowserProfileTests.includes('payload["profile"]["encrypted"], false') &&
    gatewayBrowserProfileTests.includes("browser_profile_reset_refuses_live_principal_session") &&
    browserEngineAdapter.includes("launch_with_supervisor") &&
    browserEngineAdapter.includes("display_session_receipt") &&
    browserEngineAdapter.includes("elastos.browser.engine.identity/v1") &&
    browserEngineAdapter.includes("cleanup_isolated_session"),
  "Browser VM engine path must be explicit, fail-closed, cross-platform-preflighted, artifact-buildable, and separate from Docker/container cleanup",
);

assert(
  browserMacVmProof.includes("elastos.browser.mac-vm-proof/v1") &&
    browserMacVmProof.includes("HOME_VIRTUAL_AUTH_BROWSER_EMBEDDED_UI_INPUT=1") &&
    browserMacVmProof.includes("HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTICS=1") &&
    browserMacVmProof.includes("curl -fsS -m 2 --unix-socket") &&
    browserMacVmProof.includes("decoded_frames_after_click") &&
    browserMacVmProof.includes("dropped_frames_after_click") &&
    browserMacVmProof.includes("quality_gates") &&
    browserMacVmProof.includes("max_remote_video_ready_ms") &&
    browserMacVmProof.includes("decoded_frame_delta_ok") &&
    browserMacVmProof.includes("device_pixel_ratio_ok") &&
    browserMacVmProof.includes("source_video_matches_panel") &&
    browserMacVmProof.includes("broken_image_count") &&
    browserMacVmProof.includes("pending_image_count") &&
    browserMacVmProof.includes("pending_image_samples") &&
    homePasskeyVirtualAuthSmoke.includes("diagnostics.body.images") &&
    browserMacVmProof.includes('status: "not_recorded"') &&
    macDoc.includes("scripts/browser-mac-vm-proof.sh") &&
    macDoc.includes("`quality_gates`") &&
    macDoc.includes("manual_acceptance.status=not_recorded"),
  "Mac Browser VM proof collector must bundle health, hash parity, remote video/input, diagnostics, cleanup, zoom/performance quality gates, and an explicit no-manual-acceptance marker",
);

assert(
  browserJs.includes("clipboard_write") &&
    browserJs.includes("clipboard_read") &&
    browserJs.includes("paste_text") &&
    browserJs.includes('{ type: "paste_text", text: event.key }') &&
    browserSelkiesControlService.includes("Page.setInterceptFileChooserDialog") &&
    browserSelkiesControlService.includes("DOM.setFileInputFiles") &&
    browserSelkiesControlService.includes("uploadFileIntoBrowserPage") &&
    browserSelkiesControlService.includes("cleanupBrowserUploadTempFiles") &&
    browserSelkiesControlService.includes("Runtime wallet bridge proxy is required") &&
    !browserSelkiesControlService.includes("walletRuntimeFetchDirect") &&
    browserJs.includes("browser:file-picker-selection") &&
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
  "Browser UI must bridge copy through Selkies clipboard messages and paste/printable keys through Runtime/provider CDP insertText while file uploads are Library-mediated",
);
assert(
  homeGuiWindowsSource.includes("function iframeAllowForLaunch") &&
    homeGuiWindowsSource.includes('allow="${iframeAllowForLaunch(launched)}"') &&
    !homeGuiWindowsSource.includes('"clipboard-read"') &&
    !homeGuiWindowsSource.includes('"clipboard-write"') &&
    homeShellHostSource.includes("createHomeClipboardHost") &&
    homeShellHostSource.includes("homeClipboardHost.handle(event, context, data)") &&
    homeClipboardHost.includes("await clipboard.readText()") &&
    homeClipboardHost.includes("await clipboard.writeText(clipboardText)") &&
    homeClipboardHost.includes("await prompt.request") &&
    homeClipboardHost.includes("context.targetId") &&
    !homeClipboardHost.includes("data.targetId") &&
    homeShellHostSource.includes(
      "homeClipboardHost.resetFrame(context.clipboardState, context)",
    ) &&
    homeClipboardHost.includes(
      'from "./home-clipboard-protocol.js?v=home-20260726a"',
    ) &&
    homeClipboardClient.includes(
      'from "./home-clipboard-protocol.js?v=home-20260726a"',
    ) &&
    homeClipboardProtocol.includes(
      "MAX_HOME_CLIPBOARD_TEXT_UTF8_BYTES = 65_536",
    ),
  "Only the visible trusted Home host may perform bounded Browser Clipboard reads or writes; the opaque Browser iframe must receive no Clipboard permission",
);

assert(
  browserJs.includes("browser-status-copy") &&
    browserJs.includes("Copy Browser status message") &&
    browserJs.includes("await homeClipboard.writeText(message)") &&
    browserJs.includes("homeClipboard.canRequest()") &&
    browserJs.includes("createHomeClipboardClient") &&
    homeClipboardClient.includes("createHomeClipboardClient") &&
    browserJs.includes("MAX_CLIPBOARD_TEXT_UTF8_BYTES = 65_536") &&
    browserJs.includes("MAX_CLIPBOARD_ENCODED_BYTES") &&
    browserJs.includes("MAX_CLIPBOARD_ENCODED_CHUNK_BYTES") &&
    browserJs.includes("MAX_CLIPBOARD_CHUNK_COUNT") &&
    browserJs.includes("CLIPBOARD_ASSEMBLY_TIMEOUT_MS") &&
    browserJs.includes("CLIPBOARD_COPY_INTENT_TIMEOUT_MS") &&
    browserJs.includes("pendingRemoteCopy") &&
    browserJs.includes("readHostClipboardText()") &&
    browserJs.includes('getData("text/plain")') &&
    browserJs.includes("teardownRemoteClipboard") &&
    browserJs.includes("homeClipboard.teardown()") &&
    !browserJs.includes("navigator.clipboard") &&
    !browserJs.includes("execCommand") &&
    !browserJs.includes("isOpaqueClipboardFrame") &&
    browserRemoteDisplay.includes("handleRemoteInputChannelTeardown();") &&
    browserRemoteDisplay.includes(
      'inputChannel.addEventListener("close", teardownBoundInputChannel)',
    ) &&
    browserRemoteDisplay.includes(
      'inputChannel.addEventListener("error", teardownBoundInputChannel)',
    ) &&
    browserStyle.includes('.browser-status[data-visible="true"][data-copyable="true"]') &&
    browserStyle.includes(".browser-status-copy") &&
    browser.includes("browser.js?v=browser-20260731a") &&
    !browser.includes("browser.js?v=browser-20260730a") &&
    !browser.includes("browser.js?v=browser-20260728a") &&
    !browser.includes("browser.js?v=browser-20260727a") &&
    !browser.includes("browser.js?v=browser-20260726b") &&
    !browser.includes("browser.js?v=browser-20260726a") &&
    !browser.includes("browser.js?v=browser-20260725a") &&
    !browser.includes("browser.js?v=browser-20260724a") &&
    !browser.includes("browser.js?v=browser-20260711c") &&
    homeShellHostContract.includes("### First-party Clipboard edge") &&
    homeShellHostContract.includes("not an ESP") &&
    homeShellHostContract.includes("Unsolicited remote Clipboard messages cannot change the") &&
    homeClipboardHeadlessSmoke.includes(
      "elastos.home.clipboard-headless-smoke/v1",
    ),
  "Browser Clipboard must use the closed trusted-Home edge, bind guest content to explicit local intent, preserve strict bounds and teardown, and contain no opaque-frame Clipboard fallback",
);

assert(
  browserJs.includes("function resetBrowserProfile") &&
    browserJs.includes("/api/apps/browser/profile/reset") &&
    browserJs.includes("Reset Browser cookies, local storage, history, and cache for this account?") &&
    browserJs.includes("await closeRuntimePage(activePage, {") &&
    browserJs.includes("await closeRuntimePage(stalePage, {") &&
    browserJs.includes("publishRuntimePageForHost(null)") &&
    browserJs.includes("Browser profile reset. Open the address again.") &&
    !browserJs.includes("ELASTOS_BROWSER_VM_PROFILE_DISK_ROOT") &&
    browserStyle.includes(".browser-settings-danger"),
  "Browser profile reset must be a user-confirmed Runtime route after closing active pages, without exposing host profile disk paths",
);

assert(
  browserJs.includes('const PRODUCT_DISPLAY_MODE = "webrtc_remote_display"') &&
    !browserJs.includes("DISPLAY_MODE_PREFERENCE") &&
    !browserJs.includes("function displayModeForReconnect") &&
    !browserJs.includes("function shouldRecoverStalledWebrtc") &&
    !browserJs.includes("displayModeOverride") &&
    browserJs.includes('|| "webrtc_remote_display"') &&
    !browserJs.includes(["screen", "shot"].join("")) &&
    !browserJs.includes("fetchBrowserFrame") &&
    !browserJs.includes(`|| "${["runtime", "frame"].join("_")}";`),
  "Browser UI must default to VM WebRTC/datachannel without image polling fallbacks",
);

assert(
  browserJs.includes("browser-status.js?v=browser-20260730b") &&
    browserRemoteDisplay.includes("browser-status.js?v=browser-20260730b") &&
    !browserJs.includes("browser-status.js?v=browser-20260711c") &&
    !browserRemoteDisplay.includes("browser-status.js?v=browser-20260711c") &&
    !browserJs.includes("browser-status.js?v=browser-20260626e") &&
    !browserRemoteDisplay.includes("browser-status.js?v=browser-20260626e") &&
    !browserJs.includes("browser-status.js?v=browser-20260616c") &&
    !browserRemoteDisplay.includes("browser-status.js?v=browser-20260616c") &&
    !browserRemoteDisplay.includes("browser-status.js?v=browser-20260616a") &&
    !browserJs.includes("browser-status.js?v=browser-20260615e") &&
    !browserJs.includes("browser-status.js?v=browser-20260615f") &&
    !browserJs.includes("browser-status.js?v=browser-20260615g"),
  "Browser status module cache key must advance with display default changes",
);

assert(
  browserJs.includes("browser-remote-display.js?v=browser-20260731a") &&
    !browserJs.includes("browser-remote-display.js?v=browser-20260730b") &&
    !browserJs.includes("browser-remote-display.js?v=browser-20260728a") &&
    !browserJs.includes("browser-remote-display.js?v=browser-20260727a") &&
    !browserJs.includes("browser-remote-display.js?v=browser-20260724a") &&
    !browserJs.includes("browser-remote-display.js?v=browser-20260711h") &&
    !browserJs.includes("browser-remote-display.js?v=browser-20260629a") &&
    !browserJs.includes("browser-remote-display.js?v=browser-20260627a") &&
    !browserJs.includes("browser-remote-display.js?v=browser-20260618b") &&
    !browserJs.includes("browser-remote-display.js?v=browser-20260616e") &&
    !browserJs.includes("browser-remote-display.js?v=browser-20260616d") &&
    !browserJs.includes("browser-remote-display.js?v=browser-20260616c") &&
    !browserJs.includes("browser-remote-display.js?v=browser-20260616b") &&
    !browserJs.includes("browser-remote-display.js?v=browser-20260616a") &&
    !browserJs.includes("browser-remote-display.js?v=browser-20260615g"),
  "Browser remote-display module cache key must advance with WebRTC default changes",
);

assert(
  browserJs.includes("Remote display negotiated but no video frame arrived") &&
    browserJs.includes("pollEngineCandidates") &&
    browserJs.includes("WEBRTC_ENGINE_CANDIDATE_POLL_ATTEMPTS") &&
    browserJs.includes("signalCandidate(null)") &&
    browserJs.includes('const iceTransportPolicy =') &&
    browserJs.includes(
      'displaySession.media_transport === "runtime_relay" && !engineRelayOnly',
    ) &&
    browserJs.includes("iceTransportPolicy,") &&
    browserRemoteDisplay.includes(
      'displaySession.ice_connection_policy === "runtime_launch_relay_only"',
    ) &&
    browserRemoteDisplay.includes(
      "validateRuntimeLaunchTurn(displaySession, enginePage)",
    ) &&
    browserRemoteDisplay.includes("runtimeLaunchRelayOnly ||") &&
    browserRemoteDisplay.includes("sdpHasOnlyRelayCandidates") &&
    browserRemoteDisplay.includes(
      'iceCandidateType(normalized) !== "relay"',
    ) &&
    browserJs.includes("The Browser Engine is running, but the secure display connection is not ready.") &&
    browserJs.includes(
      "Runtime must close this session before another Browser Engine or Exit Node can open.",
    ) &&
    !browserRemoteDisplay.includes("Refresh Browser") &&
    browserJs.includes("failRemoteDisplay(nextPeerConnection, \"no_first_frame\")") &&
    browserJs.includes("createRuntimePageCleanupController") &&
    browserJs.includes("sameRuntimePageOwner") &&
    browserJs.includes("Runtime cleanup is pending") &&
    browserJs.includes("Runtime confirmed the failed Browser session closed") &&
    !browserJs.includes("The stuck Browser session was closed"),
  "Browser WebRTC must use relay-only ICE for runtime_relay sessions, poll late engine candidates, and retain exact Runtime page ownership until terminal cleanup without downgrading display modes or exposing relay internals to users",
);

assert(
  browserRuntimeTurnCapabilitySmoke.includes(
    "validateRuntimeLaunchTurn",
  ) &&
    browserRuntimeTurnCapabilitySmoke.includes("substitution_rejected") &&
    browserRuntimeTurnCapabilitySmoke.includes("expiry_rejected") &&
    browserRuntimeTurnCapabilitySmoke.includes("credential_hash_verified") &&
    browserRuntimeTurnCapabilitySmoke.includes("home_persistence_absent") &&
    homePasskeyVirtualAuthSmoke.includes(
      '["credential", "auth_secret", "transport_secret"].includes(key)',
    ) &&
    homePasskeyVirtualAuthSmoke.includes(
      "JSON.stringify(redactSensitive(error.details), null, 2)",
    ),
  "Browser launch TURN must be hash-bound, expiry-bound, substitution-safe, relay-only, and redacted from persisted proof output",
);

const releaseRuntimePageForUnloadBlock = sourceBlock(
  browserMain,
  "function releaseRuntimePageForUnload()",
  "Browser unload release",
);
const finalizeRuntimePageCloseBlock = sourceBlock(
  browserMain,
  "function finalizeRuntimePageClose(owner)",
  "Browser terminal close finalizer",
);
const failRuntimeOwnedPageBlock = sourceBlock(
  browserMain,
  "async function failRuntimeOwnedPage(",
  "Browser Runtime-owned failure cleanup",
);
assert(
  releaseRuntimePageForUnloadBlock.includes("stopPageStatusPolling();") &&
    releaseRuntimePageForUnloadBlock.includes("stopPageHeartbeat();") &&
    !releaseRuntimePageForUnloadBlock.includes("resizeObserver") &&
    !releaseRuntimePageForUnloadBlock.includes("closeRuntimePage(") &&
    !releaseRuntimePageForUnloadBlock.includes("currentPage = null") &&
    !releaseRuntimePageForUnloadBlock.includes("publishRuntimePageForHost(null)") &&
    !releaseRuntimePageForUnloadBlock.includes("closeRemoteDisplay()") &&
    failRuntimeOwnedPageBlock.includes("closeRemoteDisplay();") &&
    !failRuntimeOwnedPageBlock.includes("currentPage = null") &&
    !failRuntimeOwnedPageBlock.includes("publishRuntimePageForHost(null)") &&
    finalizeRuntimePageCloseBlock.includes("currentPage = null;") &&
    finalizeRuntimePageCloseBlock.includes("currentPageGeneration = 0;") &&
    finalizeRuntimePageCloseBlock.includes("currentBrowserEngineId = \"\";") &&
    finalizeRuntimePageCloseBlock.includes("currentRemoteExitId = \"\";") &&
    finalizeRuntimePageCloseBlock.includes("publishRuntimePageForHost(null);") &&
    finalizeRuntimePageCloseBlock.includes("closeRemoteDisplay();") &&
    (browserMain.match(/currentPage = null;/g) || []).length === 2 &&
    (browserMain.match(/publishRuntimePageForHost\(null\);/g) || []).length === 1 &&
    (browserMain.match(/closeRemoteDisplay\(\);/g) || []).length === 2,
  "Browser unload and post-ownership failure cleanup must retain Runtime ownership; only a Runtime-proven terminal close may clear the exact page generation, identities, or persistence",
);

assert(
  browserJs.includes("function isBrowserErrorUrl") &&
    browserJs.includes("chrome-error://chromewebdata/") &&
    browserJs.includes("if (isBrowserErrorUrl(currentUrl))") &&
    browserJs.includes("if (crossStreamTarget)") &&
    !browserJs.includes("through a fresh Runtime route") &&
    browserJs.includes("Reopening ${visibleAddressForUrl(nextUrl)} in a fresh Browser session") &&
    browserJs.includes("return requestRuntimeOpen(nextUrl);") &&
    browserSelkiesControlService.includes("assertBrowserNavigationSucceeded(navigation, \"navigation\")") &&
    browserSelkiesControlService.includes("assertBrowserStateDidNotLandOnErrorPage(state, \"navigation\")") &&
    browserSelkiesControlService.includes("chrome-error://chromewebdata/") &&
    browserSelkiesControlService.includes("browser CDP ${label} failed") &&
    browserSelkiesControlService.includes("navigation.errorText") &&
    browserSelkiesControlService.includes("replaceBrowserPageTarget") &&
    browserSelkiesControlService.includes("browser_page_command_navigation_retarget") &&
    browserSelkiesControlServiceSmoke.includes("https://docs-late.ela.city/") &&
    browserSelkiesControlServiceSmoke.includes("late Chrome error navigation must retry on a fresh target"),
  "Browser navigation must surface CDP navigation failures and recover chrome-error pages through a fresh engine target or Runtime open",
);

assert(
  browserJs.includes(`if (remoteVideo.srcObject !== stream) {
        remoteVideo.srcObject = stream;
      }
      remoteVideo.hidden = false;
      renderEmpty.hidden = true;`),
  "Browser WebRTC display must expose the video sink when a stream attaches instead of blocking first-frame events behind hidden layout",
);

assert(
    browserJs.includes("browser-input-surface.js?v=browser-20260725b") &&
    !browserJs.includes("browser-input-surface.js?v=browser-20260711c") &&
    browserInputSurface.includes('renderPanel.addEventListener("click"') &&
    !browserJs.includes('renderImage.addEventListener("click"') &&
    browserInputSurface.includes('remoteVideo.addEventListener("click"') &&
    browserInputSurface.includes("isMediaClickTarget(event.target)") &&
    browserInputSurface.includes("event.stopPropagation();") &&
    browserInputSurface.includes("const target = remoteVideo;") &&
    browserInputSurface.includes("if (!target || target.hidden)") &&
    browserInputSurface.includes("target.videoWidth || view.width || rect.width") &&
    browserInputSurface.includes("target.videoHeight || view.height || rect.height") &&
    !browserJs.includes("browser-input-surface.js?v=browser-20260617b"),
  "Browser clicks must map against the WebRTC video surface without double dispatch or image fallback",
);

assert(
  browserJs.includes("function recoverMissingRuntimePage") &&
    browserJs.includes('recoverMissingRuntimePage(error, "Browser session was released.")') &&
    !browserJs.includes("Browser Runtime frame was released.") &&
    browserJs.includes("settleRemoteDisplayFailure") &&
    !browserJs.includes('showStatus("Browser session reconnected.")'),
  "Browser visible WebRTC pages must settle exact Runtime ownership and require an explicit user open after the VM page is released",
);

assert(
  !browserJs.includes("scheduleViewportResize") &&
    !browserJs.includes("ResizeObserver") &&
    !browserJs.includes('{ type: "resize"') &&
    browserSelkiesControlService.includes(
      "Emulation.setDeviceMetricsOverride",
    ) &&
    browserSelkiesControlService.includes(
      "deviceScaleFactor: 1",
    ) &&
    browserSelkiesControlService.includes("function browserDisplayMetrics") &&
    browserSelkiesControlService.includes("const PRODUCT_RASTER_WIDTH = 1920") &&
    browserSelkiesControlService.includes("const PRODUCT_RASTER_HEIGHT = 1080") &&
    browserSelkiesControlService.includes(
      "Browser guest raster is fixed at 1920x1080",
    ) &&
    !browserSelkiesControlService.includes("function resizeBrowserPage") &&
    browserSelkiesControlService.includes("function mediaKindsForSdp") &&
    !browserSelkiesControlService.includes("isSelkiesAudioUnavailable") &&
    !browserSelkiesControlService.includes("audio_offer_unavailable") &&
    browserSelkiesControlService.includes("const audioSdp = normalizeAudioOfferSdp(audioOfferSdp)") &&
    browserSelkiesControlService.includes("const audioMedia = mediaKindsForSdp(audioSdp)") &&
    browserSelkiesControlService.includes("this.webrtcMedia = { audio: audioMedia.audio, video: media.video }") &&
    browserSelkiesControlService.includes("audio: audioMedia.audio") &&
    browserSelkiesControlService.includes("video: media.video") &&
    browserSelkiesControlServiceSmoke.includes("audio-unavailable product display launch unexpectedly succeeded") &&
    browserSelkiesControlServiceSmoke.includes("audio-unavailable launch did not fail with a Selkies audio error") &&
    browserHostedProductSupervisor.includes("hosted product display session must advertise video=true") &&
    browserHostedProductSupervisor.includes("hosted product display session must report audio availability") &&
    browserHostedProductSupervisor.includes("hosted product audio sessions must include an audio media section") &&
    !browserEngineAdapter.includes("supervisor_accepts_video_only_vm_product_display") &&
    browserEngineAdapter.includes("Browser VM product display sessions must advertise audio=true and video=true") &&
    browserDisplayModeSmoke.includes("contained video corners must map to all encoded corners after viewer resize") &&
    browserFixedProductRasterChromiumTest.includes(
      "installed-shaped Chromium fills the DPR-1 raster through a loopback proxy",
    ) &&
    browserFixedProductRasterChromiumTest.includes(
      'corners: ["tl", "tr", "bl", "br"]',
    ) &&
    browserFixedProductRasterChromiumTest.includes(
      "fixture navigation did not traverse the loopback Runtime-shaped proxy",
    ),
  "Browser must keep one fixed 1920x1080 DPR-1 guest raster, scale only the viewer, and keep media flags matched to negotiated SDP",
);
assert(
  browserJs.includes("const LIBRARY_FILE_PICKER_MAX_BYTES = 16 * 1024 * 1024") &&
    browserSelkiesControlService.includes("const MAX_BROWSER_FILE_UPLOAD_BYTES = 16 * 1024 * 1024") &&
    browserVmControlService.includes("const MAX_BROWSER_FILE_UPLOAD_BYTES = 16 * 1024 * 1024") &&
    browserVmControlService.includes("MAX_BROWSER_INPUT_BODY_BYTES") &&
    browserVmControlService.includes("await readJsonBody(req, MAX_BROWSER_INPUT_BODY_BYTES)") &&
    gatewayApi.includes("const BROWSER_FILE_UPLOAD_BYTES: usize = 16 * 1024 * 1024") &&
    gatewayApi.includes("const BROWSER_INPUT_BODY_MAX_BYTES") &&
    gatewayApi.includes("DefaultBodyLimit::max(BROWSER_INPUT_BODY_MAX_BYTES)"),
  "Browser Library file-picker upload size must be enforced consistently in Browser UI, Selkies, VM control, and gateway body limits",
);

assert(
  browserJs.includes("function syncDisplayInputFromSession(displaySession)") &&
    browserJs.includes("syncDisplayInputFromSession(page.display_session)") &&
    browserJs.includes("currentPage?.display_session?.input === \"datachannel\"") &&
    browserJs.includes("currentPage?.display_session?.input_protocol === \"selkies_v1\"") &&
    browserJs.includes('currentInputTransport() === "datachannel"') &&
    browserJs.includes("PAGE_STATUS_AFTER_INPUT_DELAY_MS") &&
    browserJs.includes("PAGE_STATUS_AFTER_INPUT_FOLLOWUP_DELAYS_MS") &&
    browserJs.includes("pageStatusRefreshTimers = delays.map") &&
    browserJs.includes("schedulePageStatusRefresh") &&
    browserJs.includes("forceAddress: true") &&
    browserJs.includes("(!forceAddress && isAddressEditing())") &&
    browserJs.includes("fast = false") &&
    browserJs.includes("?fast=1") &&
    browserJs.includes("fetchPageStatus({ fast: true })") &&
    browserJs.includes("fetchPageStatus({ history, forceAddress })") &&
    browserSelkiesControlService.includes("function cachedBrowserPageState(browserPage)") &&
    browserSelkiesControlService.includes('state_source: fastStatus ? "cache" : "cdp"') &&
    browserSelkiesControlService.includes("refreshBrowserPageState(") &&
    browserSelkiesControlService.includes("broken_image_count") &&
    browserSelkiesControlService.includes("clickable_elements") &&
    browserSelkiesControlService.includes("top_element") &&
    browserSelkiesControlService.includes("viewport_width") &&
    browserSelkiesControlServiceSmoke.includes(
      "status did not refresh CDP URL after datachannel navigation",
    ) &&
    browserSelkiesControlServiceSmoke.includes("fast page status must be cache-backed"),
  "Browser UI must use the launch display_session as the source of truth for WebRTC datachannel/Selkies input, keep passive polling cache-backed, and force a bounded Runtime page-status refresh after datachannel navigation instead of leaving stale URL/image diagnostics",
);

assert(
  browserHostedProviderBakeoff.includes("--artifact-out") &&
    browserNativeTargetPreflight.includes("--artifact-out") &&
    browserObjectiveAudit.includes("manual UX evidence") &&
    browserObjectiveAudit.includes("audio_product_proven") &&
    browserObjectiveAudit.includes("manual_user_acceptance") &&
    browserObjectiveAudit.includes("TASKS.md") &&
    browserObjectiveAudit.includes("ROADMAP.md") &&
    browserObjectiveAudit.includes("docs/BROWSER_PROVIDER_BAKEOFF.md"),
  "Browser completion gates must produce durable machine artifacts and planning evidence",
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
    browserSessionCapacitySmoke.includes("HOME_VIRTUAL_AUTH_BROWSER_OPEN=1") &&
    browserSessionCapacitySmoke.includes("HOME_VIRTUAL_AUTH_BROWSER_EXPECT_CAPACITY_REJECTION") &&
    browserSessionCapacitySmoke.includes("HOME_VIRTUAL_AUTH_BROWSER_OPEN_CONCURRENT") &&
    browserSessionCapacitySmoke.includes('HOME_VIRTUAL_AUTH_BROWSER_OPEN_HOLD_MS="${HOME_VIRTUAL_AUTH_BROWSER_OPEN_HOLD_MS:-30000}"') &&
    browserSessionCapacitySmoke.includes("scripts/home-passkey-virtual-auth-smoke.mjs") &&
    read("scripts/home-passkey-virtual-auth-smoke.mjs").includes(
      "HOME_VIRTUAL_AUTH_BROWSER_EXPECT_CAPACITY_REJECTION",
    ) &&
    read("scripts/home-passkey-virtual-auth-smoke.mjs").includes("await heartbeat();") &&
    read("scripts/home-passkey-virtual-auth-smoke.mjs").includes("browser_capacity_unavailable") &&
    read("scripts/README.md").includes("HOME_VIRTUAL_AUTH_BROWSER_OPEN=0 HOME_VIRTUAL_AUTH_BROWSER_SUMMARY=1") &&
    read("scripts/README.md").includes("holds heartbeats for") &&
    read("scripts/README.md").includes("browser_capacity_unavailable"),
  "Browser session-capacity proof must document summary-only checks separately and exercise long-hold heartbeat continuity plus active-page capacity rejection",
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
