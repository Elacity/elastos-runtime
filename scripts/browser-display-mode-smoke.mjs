#!/usr/bin/env node
import fs from "node:fs";
import vm from "node:vm";

const repoRoot = new URL("../", import.meta.url);
const browserSource = [
  "capsules/browser/browser.js",
  "capsules/browser/browser-clipboard.js",
  "capsules/browser/browser-history.js",
  "capsules/browser/browser-input-surface.js",
  "capsules/browser/browser-input.js",
  "capsules/browser/browser-location.js",
  "capsules/browser/browser-remote-display.js",
  "capsules/browser/browser-runtime-api.js",
  "capsules/browser/browser-status.js",
  "capsules/browser/browser-webrtc.js",
  "scripts/browser-selkies-control-service.mjs",
]
  .map((path) => fs.readFileSync(new URL(path, repoRoot), "utf8"))
  .join("\n");
const browserStyle = fs.readFileSync(
  new URL("capsules/browser/style.css", repoRoot),
  "utf8",
);
const homeShellWindowsSource = fs.readFileSync(
  new URL("capsules/home/browser/shell-windows.js", repoRoot),
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
expectValue("display=diagnostic&debug=1", "diagnostic_frame");
expectValue("display_mode=diagnostic_frame&metrics=1", "diagnostic_frame");
expectError("display=diagnostic", "requires debug=1 or metrics=1");
expectError("display_mode=diagnostic_frame", "requires debug=1 or metrics=1");
expectError("display=unsupported", "Unsupported Browser display mode");

assert(
  !browserSource.includes(
    '["webrtc_remote_display", "native_surface", "diagnostic_frame"].includes(value)',
  ),
  "diagnostic_frame must not be in the normal Browser display-mode allow-list",
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
    needle: "Click the page to enable remote audio.",
    label:
      "remote display must fail visibly if browser autoplay blocks audio unlock",
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
    browserSource.includes("!requiresRuntimeRoute"),
  "Browser viewport resize must route through Runtime/provider CDP control instead of Selkies datachannel fallback",
);
assert(
  browserSource.includes(
    'if (currentDisplayMode === "webrtc_remote_display")',
  ) &&
    browserSource.includes("lastViewport = viewport;") &&
    browserSource.includes("return;"),
  "Stable WebRTC Browser display must not send provider resize commands that can freeze the fixed compositor stream",
);
assert(
  !selkiesMessagesForInputSource.includes('event.type === "resize"'),
  "Selkies datachannel input must not own viewport resize; resize is provider state, not pointer input",
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
    const renderImage = { hidden: true };
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
      ownerDocument: { defaultView: { getComputedStyle: () => ({ objectFit: "fill" }) } },
      getBoundingClientRect: () => ({ left: 0, top: 0, width: 1000, height: 1000 })
    };
    const renderImage = { hidden: true };
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
    `filled center must map to video center, got ${JSON.stringify(points.center)}`,
  );
  assert(
    points.outside.x === 960 && points.outside.y === 108,
    `filled surface must map full visible area to remote coordinates, got ${JSON.stringify(points.outside)}`,
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
  /\.browser-remote-display\s*\{[\s\S]*?object-fit:\s*fill\s*;/.test(
    browserStyle,
  ) &&
    /\.browser-render\s*\{[\s\S]*?object-fit:\s*fill\s*;/.test(browserStyle),
  "Browser render surfaces must fill the capsule viewport; the engine owns browser viewport fidelity",
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
    browserSource.includes("pasteHostClipboardIntoRemote") &&
    browserSource.includes("focusRemoteInput") &&
    browserSource.includes("focusKeyboardCapture") &&
    browserSource.includes("handlePasteChord") &&
    browserSource.includes('event.getModifierState?.("Control")') &&
    browserSource.includes("hostModifierState.control"),
  "Browser UI must capture host paste and route it through Runtime/provider insertText instead of simulated remote Ctrl+V",
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
