import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import test from "node:test";
import { selkiesMessagesForInput } from "../capsules/browser/browser/browser-input.js";

const surface = readFileSync(new URL("../capsules/browser/browser/browser-input-surface.js", import.meta.url), "utf8");
function surfaceFunction(name) {
  const start = surface.indexOf(`  function ${name}(`);
  const end = surface.indexOf("\n  }", start);
  return surface.slice(start, end + 4);
}
for (const [width, height] of [[390, 1200], [844, 338], [320, 240], [1000, 700]]) {
  test(`${width}x${height}: contained decoded corners map to valid pixels and wheel preserves zero`, () => {
    const rect = { left: 20, top: 30, width: 1000, height: 900 };
    const scale = Math.min(rect.width / width, rect.height / height);
    const x = rect.left + (rect.width - width * scale) / 2;
    const y = rect.top + (rect.height - height * scale) / 2;
    const map = vm.runInNewContext(`${surfaceFunction("browserMediaContentRect")}\n${surfaceFunction("browserPointFromEvent")}\nbrowserPointFromEvent`, {
      remoteVideo: { hidden: false, videoWidth: width, videoHeight: height,
        getBoundingClientRect: () => rect,
        ownerDocument: { defaultView: { getComputedStyle: () => ({ objectFit: "contain" }) } } },
      getCurrentView: () => ({ width: 1920, height: 1080 }), // stale control ACK must lose to decoded geometry
    });
    const points = [[x, y], [x + width * scale, y], [x, y + height * scale], [x + width * scale, y + height * scale]];
    for (const [index, [clientX, clientY]] of points.entries()) {
      const point = JSON.parse(JSON.stringify(map({ clientX, clientY })));
      assert.deepEqual(point, { x: index % 2 ? width - 1 : 0, y: index >= 2 ? height - 1 : 0 });
      assert.equal(selkiesMessagesForInput({ type: "wheel", ...point, delta_y: 120 }, { width, height })[0],
        `m,${point.x},${point.y},8,2`);
    }
    assert.equal(map({ clientX: x - 10, clientY: y - 10 }), null);
    const release = JSON.parse(JSON.stringify(map({ clientX: x - 10, clientY: y - 10 }, true)));
    assert.deepEqual(release, { x: 0, y: 0 });
  });
}

function shellFixture() {
  const source = readFileSync(new URL("../capsules/home-gui/browser/shell-window-geometry.js", import.meta.url), "utf8")
    .replace(/^import[\s\S]*?;\n/gm, "").replace(/export /g, "");
  const events = {}, node = { dataset: { target: "browser", maximized: "false", windowId: "b" },
    style: { left: "100px", top: "100px", width: "1000px", height: "700px" },
    querySelectorAll: () => [handle] };
  const handle = { dataset: { resize: "e" }, addEventListener: (name, cb) => { events[name] = cb; },
    setPointerCapture() {}, releasePointerCapture() {} };
  const context = { desktop: { getBoundingClientRect: () => ({ left: 0, top: 0 }) },
    window: { innerWidth: 2000, innerHeight: 1500 },
    windowSnapPreview: { hidden: true }, WINDOW_MIN_WIDTH: 320, WINDOW_MIN_HEIGHT: 240,
    WINDOW_SNAP_THRESHOLD: 20, WINDOW_SIDE_INSET: 8, WINDOW_TOP_INSET: 8, WINDOW_BOTTOM_INSET: 8,
    beginShellInteraction() {}, endShellInteraction() {}, registerStageGesture() {}, isPhone: () => false,
    clamp: (value, min, max) => Math.max(min, Math.min(max, value)) };
  const api = vm.runInNewContext(`${source}\n({fitWindowToBrowserAspect, attachWindowResize, applyWindowPlacement, applyWindowSnapState, rememberWindowRestoreBounds, restoreWindowFromSpecialState})`, context);
  return { api, node, handle, events };
}

test("Browser fit and restore preserve independent width and height", () => {
  const { api, node } = shellFixture();
  api.fitWindowToBrowserAspect(node);
  assert.equal(node.style.width, "1000px"); assert.equal(node.style.height, "700px");
  api.applyWindowPlacement(node, { x: 10, y: 10, width: 390, height: 1200 });
  assert.equal(node.style.width, "390px"); assert.equal(node.style.height, "1200px");
  api.applyWindowSnapState(node, "left");
  assert.ok(parseFloat(node.style.height) > parseFloat(node.style.width), "left snap fills the tall workspace");
});

test("Browser east drag changes width alone, south drag changes height alone", () => {
  for (const direction of ["e", "s"]) {
    const { api, node, handle, events } = shellFixture(); handle.dataset.resize = direction;
    api.attachWindowResize(node, () => {});
    const event = { pointerId: 1, button: 0, clientX: 1100, clientY: 800,
      preventDefault() {}, stopPropagation() {} };
    events.pointerdown(event); events.pointermove({ ...event, clientX: 1200, clientY: 900 });
    events.pointerup({ ...event, type: "pointerup" });
    assert.equal(node.style.width, direction === "e" ? "1100px" : "1000px");
    assert.equal(node.style.height, direction === "s" ? "800px" : "700px");
  }
});


test("Browser maximize fills the workspace and restore keeps free dimensions", () => {
  const { api, node } = shellFixture();
  api.rememberWindowRestoreBounds(node);
  node.dataset.browserMaximized = "true";
  api.fitWindowToBrowserAspect(node);
  assert.equal(node.style.width, "1984px"); assert.equal(node.style.height, "1484px");
  api.restoreWindowFromSpecialState(node); api.fitWindowToBrowserAspect(node);
  assert.equal(node.style.width, "1000px"); assert.equal(node.style.height, "700px");
});
