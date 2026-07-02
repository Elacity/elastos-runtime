#!/usr/bin/env node
import fs from "node:fs";
import vm from "node:vm";
import {
  isBrowserErrorUrl,
  sameBrowserStreamTarget,
} from "../capsules/browser/browser/browser-runtime-api.js";

const repoRoot = new URL("../", import.meta.url);
const browserSource = [
  "capsules/browser/browser/browser.js",
  "capsules/browser/browser/browser-clipboard.js",
  "capsules/browser/browser/browser-history.js",
  "capsules/browser/browser/browser-input-surface.js",
  "capsules/browser/browser/browser-input.js",
  "capsules/browser/browser/browser-location.js",
  "capsules/browser/browser/browser-remote-display.js",
  "capsules/browser/browser/browser-runtime-api.js",
  "capsules/browser/browser/browser-status.js",
  "capsules/browser/browser/browser-webrtc.js",
  "scripts/browser-selkies-control-service.mjs",
]
  .map((path) => fs.readFileSync(new URL(path, repoRoot), "utf8"))
  .join("\n");
const browserStyle = fs.readFileSync(
  new URL("capsules/browser/browser/style.css", repoRoot),
  "utf8",
);
const homeShellWindowsSource = fs.readFileSync(
  new URL("capsules/home/browser/shell-windows.js", repoRoot),
  "utf8",
);
const homeShellWindowGeometrySource = fs.readFileSync(
  new URL("capsules/home/browser/shell-window-geometry.js", repoRoot),
  "utf8",
);

function extractFunction(source, name) {
  const marker = `function ${name}(`;
  const start = source.indexOf(marker);
  if (start < 0) {
    throw new Error(`${name} function not found`);
  }
  const open = source.indexOf("{", start);
  if (open < 0) {
    throw new Error(`${name} function body not found`);
  }
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
  throw new Error(`${name} function body is not balanced`);
}

const requestedDisplayModeSource = extractFunction(
  browserSource,
  "requestedDisplayMode",
);
const sanitizedErrorTextSource = extractFunction(
  browserSource,
  "sanitizedErrorText",
);
const browserLaunchFailureSummarySource = extractFunction(
  browserSource,
  "browserLaunchFailureSummary",
);
const isAuthoritySessionErrorSource = extractFunction(
  browserSource,
  "isAuthoritySessionError",
);
const friendlyOpenErrorSource = extractFunction(
  browserSource,
  "friendlyOpenError",
);
const keysymForBrowserKeySource = extractFunction(
  browserSource,
  "keysymForBrowserKey",
);
const selkiesKeypressMessagesForBrowserKeySource = extractFunction(
  browserSource,
  "selkiesKeypressMessagesForBrowserKey",
);
const selkiesMessagesForInputSource = extractFunction(
  browserSource,
  "selkiesMessagesForInput",
);
const browserPointFromEventSource = extractFunction(
  browserSource,
  "browserPointFromEvent",
);
const browserMediaContentRectSource = extractFunction(
  browserSource,
  "browserMediaContentRect",
);

function runCase(query) {
  const script = new vm.Script(`
    const params = new URLSearchParams(${JSON.stringify(query)});
    const debugMetrics = params.get("debug") === "1" || params.get("metrics") === "1";
    ${requestedDisplayModeSource}
    requestedDisplayMode();
  `);
  return script.runInNewContext({ URLSearchParams });
}

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function expectValue(query, expected) {
  const actual = runCase(query);
  assert(
    actual === expected,
    `${query || "<empty>"} expected ${expected}, got ${actual}`,
  );
}

function expectError(query, expectedMessage) {
  try {
    runCase(query);
  } catch (error) {
    assert(
      String(error?.message || error).includes(expectedMessage),
      `${query} expected error containing ${expectedMessage}, got ${error?.message || error}`,
    );
    return;
  }
  throw new Error(`${query} expected an error`);
}

expectValue("", "webrtc_remote_display");
expectValue("display=webrtc_remote_display", "webrtc_remote_display");
expectValue("display=native_surface", "native_surface");
expectError("display=diagnostic", "Unsupported Browser display mode");
expectError("display_mode=frame", "Unsupported Browser display mode");
expectError("display=unsupported", "Unsupported Browser display mode");

{
  const script = new vm.Script(`
    ${isAuthoritySessionErrorSource}
    ${sanitizedErrorTextSource}
    ${browserLaunchFailureSummarySource}
    ${friendlyOpenErrorSource}
    friendlyOpenError({
      status: 400,
      message: "browser engine supervisor exited with status exit status: 1; Browser VM persistent launcher exited before readiness with 0:\\n\\u001b[32m INFO\\u001b[0m browser-vz-engine-supervisor stage=open_guest_page_start"
    });
  `);
  const message = script.runInNewContext({});
  assert(
    message === "Browser Engine failed to start cleanly. The failed session was closed; refresh Browser, or choose another Browser Engine.",
    `Browser launch failures must be sanitized, got ${message}`,
  );
}

assert(
  browserSource.includes(
    '["webrtc_remote_display", "native_surface"].includes(value)',
  ),
  "Browser display-mode allow-list must only include product WebRTC and native surfaces",
);

const requiredAudioInvariants = [
  {
    needle: "const expectsAudio = displaySession.audio === true",
    label:
      "remote display must require explicit audio=true in the display-session receipt",
  },
  {
    needle: "prepareAudio(expectsAudio)",
    label:
      "remote display must initialize audio autoplay/gesture state before playback",
  },
  {
    needle:
      'nextPeerConnection.addTransceiver("audio", { direction: "recvonly" })',
    label:
      "remote display must request a receive-only audio track when the provider advertises audio",
  },
  {
    needle: "remoteVideo.muted = true",
    label:
      "remote display must start muted so browser autoplay can connect first",
  },
  {
    needle: "Remote display ready. Click the page to enable audio.",
    label:
      "remote display must prompt for user-gesture audio unlock instead of silently claiming audio",
  },
  {
    needle: "Remote audio enabled.",
    label: "remote display must report successful audio unlock",
  },
  {
    needle:
      "A later user gesture will retry audio unlock; do not pin UI over the page.",
    label:
      "remote display must retry user-gesture audio unlock without pinning a false alarm over the page",
  },
  {
    needle:
      'renderPanel.addEventListener("pointerdown", unlockRemoteAudioFromGesture',
    label:
      "remote display must unlock audio from a direct pointer gesture, not only delayed click events",
  },
  {
    needle: "remoteVideo.volume = 1",
    label:
      "remote display must reset audible volume before unlocking WebRTC audio",
  },
  {
    needle: "audio ${audioState}",
    label: "debug metrics must expose audio unlock state",
  },
  {
    needle: "arx ${audioBytes}",
    label: "debug metrics must expose received audio bytes",
  },
];

for (const invariant of requiredAudioInvariants) {
  assert(browserSource.includes(invariant.needle), invariant.label);
}

assert(
  browserSource.includes(`if (remoteVideo.srcObject !== stream) {
        remoteVideo.srcObject = stream;
      }
      remoteVideo.hidden = false;
      renderEmpty.hidden = true;`),
  "remote display must show the WebRTC video sink as soon as a stream attaches instead of waiting on hidden-element frame events",
);

{
  const script = new vm.Script(`
    let currentView = { width: 1920, height: 1080 };
    ${keysymForBrowserKeySource}
    ${selkiesKeypressMessagesForBrowserKeySource}
    ${selkiesMessagesForInputSource}
    JSON.stringify({
      down: selkiesMessagesForInput({ type: "wheel", x: 960, y: 540, delta_y: 120 }),
      up: selkiesMessagesForInput({ type: "wheel", x: 960, y: 540, delta_y: -120 }),
      lower: selkiesMessagesForInput({ type: "key", key: "a" }),
      upper: selkiesMessagesForInput({ type: "key", key: "A" }),
      text: selkiesMessagesForInput({ type: "text", text: "Aa" })
    });
  `);
  const wheel = JSON.parse(script.runInNewContext({}));
  assert(
    wheel.down[0] === "m,960,540,8,2",
    `wheel down must map to Selkies scroll-down mask, got ${wheel.down[0]}`,
  );
  assert(
    wheel.up[0] === "m,960,540,16,2",
    `wheel up must map to Selkies scroll-up mask, got ${wheel.up[0]}`,
  );
  assert(
    JSON.stringify(wheel.lower) === JSON.stringify(["co,end,a"]),
    `lowercase input must use Selkies deterministic text input, got ${JSON.stringify(wheel.lower)}`,
  );
  assert(
    JSON.stringify(wheel.upper) === JSON.stringify(["co,end,A"]),
    `uppercase input must use Selkies deterministic text input, got ${JSON.stringify(wheel.upper)}`,
  );
  assert(
    JSON.stringify(wheel.text) === JSON.stringify(["co,end,Aa"]),
    `text input must preserve uppercase through Selkies, got ${JSON.stringify(wheel.text)}`,
  );
}

assert(
  browserSource.includes("const requiresRuntimeRoute =") &&
    browserSource.includes('event?.type === "browser_command"') &&
    browserSource.includes('event?.type === "resize"') &&
    browserSource.includes('event?.type === "file_upload"') &&
    browserSource.includes("!requiresRuntimeRoute"),
  "Browser viewport resize and file upload must route through Runtime/provider CDP control instead of Selkies datachannel fallback",
);
assert(
  sameBrowserStreamTarget("https://ela.city/", "https://ela.city/home") &&
    !sameBrowserStreamTarget("https://ela.city/", "https://example.com/") &&
    isBrowserErrorUrl("chrome-error://chromewebdata/") &&
    isBrowserErrorUrl("chrome-error://crash/") &&
    !isBrowserErrorUrl("https://docs.ela.city/") &&
    browserSource.includes("const crossStreamTarget = !sameBrowserStreamTarget(currentUrl, nextUrl);") &&
    browserSource.includes("if (isBrowserErrorUrl(currentUrl))") &&
    browserSource.includes("return requestRuntimeOpen(nextUrl);") &&
    browserSource.includes("Reopening ${visibleAddressForUrl(nextUrl)} in a fresh Browser session"),
  "Browser address-bar navigation must try Runtime/provider navigation from Chrome error pages before falling back to a fresh Runtime session",
);
assert(
  browserSource.includes("function scheduleViewportResize()") &&
    browserSource.includes("lastViewport = viewport;") &&
    !browserSource.includes('type: "resize"'),
  "Stable WebRTC Browser display must not send provider resize commands that can freeze the fixed compositor stream",
);
assert(
  homeShellWindowsSource.includes(`syncBrowserWindow(entry, launched);
  if (entry.targetId === "browser") {
    fitLaunchedWindow(entry);
  }`) &&
    homeShellWindowsSource.includes("fitWindowToLargestBrowserAspect") &&
    homeShellWindowsSource.includes("dataset.browserMaximized") &&
    homeShellWindowGeometrySource.includes(
      "export function fitWindowToLargestBrowserAspect",
    ) &&
    homeShellWindowGeometrySource.includes(
      'node.dataset.target === BROWSER_TARGET_ID',
    ) &&
    !homeShellWindowsSource.includes("prebootBrowserTarget") &&
    !homeShellWindowsSource.includes("dataset.preboot") &&
    homeShellWindowsSource.includes(`if (entry.targetId === "browser") {
    fitWindowToBrowserAspect(entry.node);
    rememberWindowRestoreBounds(entry.node);
    return;
  }`),
  "Home Browser windows must immediately fit/persist 16:9 restore geometry without automatic hidden preboot windows",
);
assert(
  !selkiesMessagesForInputSource.includes('event.type === "resize"'),
  "Selkies datachannel input must not own viewport resize; resize is provider state, not pointer input",
);
assert(
  browserSource.includes(
    "const PAGE_STATUS_AFTER_INPUT_FOLLOWUP_DELAYS_MS = [650, 1800, 3500, 6500]",
  ) &&
    browserSource.includes("const PAGE_STATUS_INTERVAL_MS = 2_500") &&
    browserSource.includes("pageStatusRefreshTimers = delays.map") &&
    browserSource.includes("forceAddress = false") &&
    browserSource.includes("forceAddress: true") &&
    browserSource.includes("(!forceAddress && isAddressEditing())") &&
    browserSource.includes("stopPageStatusRefresh();"),
  "WebRTC datachannel input must schedule a bounded forced post-input status burst so SPA navigation updates the Browser address",
);
assert(
  browserSource.includes('const PRODUCT_DISPLAY_MODE = "webrtc_remote_display"') &&
    !browserSource.includes("DISPLAY_MODE_PREFERENCE") &&
    !browserSource.includes("function displayModeForReconnect") &&
    !browserSource.includes("function shouldRecoverStalledWebrtc") &&
    !browserSource.includes("displayModeOverride") &&
    browserSource.includes("pollEngineCandidates") &&
    browserSource.includes("WEBRTC_ENGINE_CANDIDATE_POLL_ATTEMPTS") &&
    browserSource.includes("signalCandidate(null)") &&
    browserSource.includes("Remote display negotiated but no video frame arrived") &&
    browserSource.includes("The Browser Engine is running, but the secure display connection is not ready.") &&
    browserSource.includes("this device has no secure display relay candidate") &&
    browserSource.includes("shared secure display route") &&
    browserSource.includes("Refresh Browser, or choose another Browser Engine or Exit Node.") &&
    browserSource.includes('const iceTransportPolicy =') &&
    browserSource.includes('displaySession.media_transport === "runtime_relay" ? "relay" : "all"') &&
    browserSource.includes("iceTransportPolicy,") &&
    browserSource.includes("failRemoteDisplay(nextPeerConnection, \"no_first_frame\")") &&
    browserSource.includes('onRecoveryRequired(message, { retry: false })') &&
    browserSource.includes("The stuck Browser session was closed"),
  "Browser product launch must not silently fall back from WebRTC to image polling, leave stuck Browser sessions, or expose relay internals to users",
);

{
  const script = new vm.Script(`
    let currentView = { width: 1100, height: 714 };
    let currentDisplayMode = "webrtc_remote_display";
    const getCurrentDisplayMode = () => currentDisplayMode;
    const getCurrentView = () => currentView;
    const remoteVideo = {
      hidden: false,
      videoWidth: 1920,
      videoHeight: 1080,
      ownerDocument: { defaultView: { getComputedStyle: () => ({ objectFit: "fill" }) } },
      getBoundingClientRect: () => ({ left: 10, top: 20, width: 960, height: 540 })
    };
    ${browserMediaContentRectSource}
    ${browserPointFromEventSource}
    JSON.stringify(browserPointFromEvent({ clientX: 490, clientY: 290 }));
  `);
  const point = JSON.parse(script.runInNewContext({}));
  assert(
    point.x === 960 && point.y === 540,
    `remote input must map against video coordinates, got ${JSON.stringify(point)}`,
  );
}

{
  const script = new vm.Script(`
    let currentView = { width: 1920, height: 1080 };
    let currentDisplayMode = "webrtc_remote_display";
    const getCurrentDisplayMode = () => currentDisplayMode;
    const getCurrentView = () => currentView;
    const remoteVideo = {
      hidden: false,
      videoWidth: 1920,
      videoHeight: 1080,
      ownerDocument: { defaultView: { getComputedStyle: () => ({ objectFit: "contain" }) } },
      getBoundingClientRect: () => ({ left: 0, top: 0, width: 1000, height: 1000 })
    };
    ${browserMediaContentRectSource}
    ${browserPointFromEventSource}
    JSON.stringify({
      center: browserPointFromEvent({ clientX: 500, clientY: 500 }),
      outside: browserPointFromEvent({ clientX: 500, clientY: 100 })
    });
  `);
  const points = JSON.parse(script.runInNewContext({}));
  assert(
    points.center.x === 960 && points.center.y === 540,
    `contained center must map to video center, got ${JSON.stringify(points.center)}`,
  );
  assert(
    points.outside === null,
    `contained letterbox clicks must not map into remote coordinates, got ${JSON.stringify(points.outside)}`,
  );
}

assert(
  browserSource.includes("touchPanState"),
  "Browser must track touch/pen pan gestures for remote scrolling",
);
assert(
  browserSource.includes("suppressSyntheticClickUntil"),
  "Browser must suppress duplicate synthetic clicks after touch taps",
);
assert(
  /\.browser-remote-display\s*\{[\s\S]*?object-fit:\s*contain\s*;/.test(
    browserStyle,
  ),
  "Browser remote display surface must preserve aspect ratio so Home resizing cannot visually zoom or stretch the page",
);
assert(
  /html,\s*\nbody\s*\{[\s\S]*?overflow:\s*hidden\s*;/.test(browserStyle) &&
    /\.browser-shell\s*\{[\s\S]*?height:\s*100%\s*;[\s\S]*?min-height:\s*0\s*;[\s\S]*?overflow:\s*hidden\s*;/.test(
      browserStyle,
    ),
  "Browser app must fill the Home iframe without document scrollbars or viewport min-height overflow",
);
assert(
  browserSource.includes("clipboard_write") &&
    browserSource.includes("clipboard_read") &&
    browserSource.includes("paste_text") &&
    browserSource.includes("Input.insertText") &&
    browserSource.includes('{ type: "paste_text", text: event.key }') &&
    browserSource.includes("pasteHostClipboardIntoRemote") &&
    browserSource.includes("focusRemoteInput") &&
    browserSource.includes("focusKeyboardCapture") &&
    browserSource.includes("handlePasteChord") &&
    browserSource.includes('event.getModifierState?.("Control")') &&
    browserSource.includes("hostModifierState.control"),
  "Browser UI must capture host paste and printable keys through Runtime/provider insertText instead of simulated remote Ctrl+V",
);
assert(
  homeShellWindowsSource.includes("function iframeAllowForLaunch") &&
    homeShellWindowsSource.includes('launched?.target === "browser"') &&
    homeShellWindowsSource.includes('"clipboard-read"') &&
    homeShellWindowsSource.includes('"clipboard-write"') &&
    homeShellWindowsSource.includes('allow="${iframeAllowForLaunch(launched)}"'),
  "Home must grant clipboard-read/write only through the Browser iframe allow policy so remote paste can read the host clipboard",
);

console.log(
  JSON.stringify({
    schema: "elastos.browser.display-mode-smoke/v1",
    ok: true,
    default_mode: "webrtc_remote_display",
    diagnostic_requires_debug: true,
    audio_invariants_checked: requiredAudioInvariants.length,
    input_invariants_checked: 10,
  }),
);
