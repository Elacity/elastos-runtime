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
  "capsules/home/browser/home-clipboard-client.js",
  "capsules/home/browser/home-clipboard-protocol.js",
  "capsules/browser/browser/browser-history.js",
  "capsules/browser/browser/browser-input-surface.js",
  "capsules/browser/browser/browser-input.js",
  "capsules/browser/browser/browser-location.js",
  "capsules/browser/browser/browser-page-cleanup.js",
  "capsules/browser/browser/browser-remote-display.js",
  "capsules/browser/browser/browser-runtime-api.js",
  "capsules/browser/browser/browser-status.js",
  "capsules/browser/browser/browser-webrtc.js",
  "scripts/browser-selkies-control-service.mjs",
]
  .map((path) => fs.readFileSync(new URL(path, repoRoot), "utf8"))
  .join("\n");
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
const runtimeOpenOutcomeSource = extractFunction(
  browserSource,
  "runtimeOpenOutcome",
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
    ${runtimeOpenOutcomeSource}
    ${friendlyOpenErrorSource}
    friendlyOpenError({
      status: 503,
      message: "browser engine supervisor exited with status exit status: 1",
      payload: {
        outcome: {
          schema: "elastos.browser.open-outcome/v1",
          state: "terminal_pre_effect_failure",
          effects: {
            page_acquired: false,
            vm_acquired: false,
            stream_acquired: false
          }
        }
      }
    });
  `);
  const message = script.runInNewContext({});
  assert(
    message === "Browser Engine failed to start cleanly. No Browser page or VM was acquired.",
    `Browser launch failures must use their structured Runtime outcome, got ${message}`,
  );
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
      topLeft: browserPointFromEvent({ clientX: 0, clientY: 218.75 }),
      bottomRight: browserPointFromEvent({ clientX: 1000, clientY: 781.25 }),
      outside: browserPointFromEvent({ clientX: 500, clientY: 100 })
    });
  `);
  const points = JSON.parse(script.runInNewContext({}));
  assert(
    points.center.x === 960 && points.center.y === 540,
    `contained center must map to video center, got ${JSON.stringify(points.center)}`,
  );
  assert(
    points.topLeft.x === 0 &&
      points.topLeft.y === 0 &&
      points.bottomRight.x === 1919 &&
      points.bottomRight.y === 1079,
    `contained video corners must map to all encoded corners after viewer resize, got ${JSON.stringify(points)}`,
  );
  assert(
    points.outside === null,
    `contained letterbox clicks must not map into remote coordinates, got ${JSON.stringify(points.outside)}`,
  );
}
console.log(
  JSON.stringify({
    schema: "elastos.browser.display-mode-smoke/v1",
    ok: true,
    default_mode: "webrtc_remote_display",
    diagnostic_requires_debug: true,
    input_invariants_checked: 12,
  }),
);
