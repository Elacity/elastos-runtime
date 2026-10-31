import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { inputSurfaceFixture } from "./lib/browser-input-surface-fixture.mjs";
import test from "node:test";
import { selkiesMessagesForInput } from "../capsules/browser/browser/browser-input.js";

for (const [width, height] of [[390, 1200], [844, 338], [320, 240], [1000, 700]]) {
  test(`${width}x${height}: contained decoded corners map to valid pixels and wheel preserves zero`, () => {
    const rect = { left: 20, top: 30, width: 1000, height: 900 };
    const f = inputSurfaceFixture(width, height, rect);
    const scale = Math.min(rect.width / width, rect.height / height);
    const x = rect.left + (rect.width - width * scale) / 2;
    const y = rect.top + (rect.height - height * scale) / 2;
    const points = [[x, y], [x + width * scale, y], [x, y + height * scale], [x + width * scale, y + height * scale]];
    for (const [index, [clientX, clientY]] of points.entries()) {
      f.video.emit("click", { clientX, clientY });
      const point = f.sent.at(-1);
      assert.deepEqual(point, { type: "click", x: index % 2 ? width - 1 : 0, y: index >= 2 ? height - 1 : 0 });
      assert.equal(selkiesMessagesForInput({ ...point, type: "wheel", delta_y: 120 }, { width, height })[0], `m,${point.x},${point.y},8,2`);
    }
    f.video.emit("click", { clientX: x - 10, clientY: y - 10 });
    assert.equal(f.sent.length, 4);
    f.block(true); f.video.emit("click", { clientX: x, clientY: y });
    assert.equal(f.sent.length, 4, "pending decoded geometry blocks pointer mapping");
    f.block(false); f.video.emit("click", { clientX: x, clientY: y });
    assert.equal(f.sent.length, 5);
  });
}

let fixtureNumber = 0;
async function shellFixture() {
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
  globalThis.window = context.window;
  globalThis.__geometryFixture = context;
  const hooks = registerHooks({ resolve(specifier, options, next) {
    if (options.parentURL?.includes("shell-window-geometry.js") && /^\.\/shell-(core|form-factor|popovers)\.js/.test(specifier)) {
      const names = Object.keys(context).filter(name => name !== "window");
      return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(names.map(name => `export const ${name} = globalThis.__geometryFixture.${name};`).join("\n"))}#${fixtureNumber}` };
    }
    return next(specifier, options);
  } });
  let api;
  try { api = await import(`../capsules/home-gui/browser/shell-window-geometry.js?fixture=${++fixtureNumber}`); }
  finally { hooks.deregister(); delete globalThis.__geometryFixture; }

  return { api, node, handle, events };
}

test("Browser fit and restore preserve independent width and height", async () => {
  const { api, node } = await shellFixture();
  api.fitBrowserWindow(node);
  assert.equal(node.style.width, "1000px"); assert.equal(node.style.height, "700px");
  api.applyWindowPlacement(node, { x: 10, y: 10, width: 390, height: 1200 });
  assert.equal(node.style.width, "390px"); assert.equal(node.style.height, "1200px");
  api.applyWindowSnapState(node, "left");
  assert.ok(parseFloat(node.style.height) > parseFloat(node.style.width), "left snap fills the tall workspace");
});

test("Browser east drag changes width alone, south drag changes height alone", async () => {
  for (const direction of ["e", "s"]) {
    const { api, node, handle, events } = await shellFixture(); handle.dataset.resize = direction;
    api.attachWindowResize(node, () => {});
    const event = { pointerId: 1, button: 0, clientX: 1100, clientY: 800,
      preventDefault() {}, stopPropagation() {} };
    events.pointerdown(event); events.pointermove({ ...event, clientX: 1200, clientY: 900 });
    events.pointerup({ ...event, type: "pointerup" });
    assert.equal(node.style.width, direction === "e" ? "1100px" : "1000px");
    assert.equal(node.style.height, direction === "s" ? "800px" : "700px");
  }
});


test("Browser maximize fills the workspace and restore keeps free dimensions", async () => {
  const { api, node } = await shellFixture();
  api.rememberWindowRestoreBounds(node);
  node.dataset.browserMaximized = "true";
  api.fitBrowserWindow(node);
  assert.equal(node.style.width, "1984px"); assert.equal(node.style.height, "1484px");
  api.restoreWindowFromSpecialState(node); api.fitBrowserWindow(node);
  assert.equal(node.style.width, "1000px"); assert.equal(node.style.height, "700px");
});
