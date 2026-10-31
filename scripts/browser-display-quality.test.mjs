import assert from "node:assert/strict";
import test from "node:test";
import { browserDisplayMetrics, resizeBrowserPage, applyBrowserDisplayMode, pruneBrowserDisplayModes,
  validateBrowserGeometry } from "./browser-selkies-control-service.mjs";

function fixture({ failMode = false, failMetrics = false, failGate = "", failVerify = false } = {}) {
  const log = [];
  let size = { width: 1000, height: 700 }, viewport = { ...size };
  let native = { schema: "elastos.browser.x11-geometry/v1", ok: true,
    root: { xid: "0x1", ...size },
    window: { xid: "0x2", parent_xid: "0x1", pid: 123, wm_class: ["app", "Chromium"],
      x: 0, y: 0, ...size, border_width: 0, override_redirect: false } };
  const page = { config: { displaySurface: { stream: { ...size } } },
    displaySession: { width: size.width, height: size.height },
    browserPage: { target_id: "same-target", ...size, _cdp: { async request(method, params) {
      if (method === "Browser.getWindowForTarget") return { windowId: 123,
        bounds: { left: native.window.x, top: native.window.y, width: native.window.width, height: failVerify && native.window.width == 390 ? 1 : native.window.height, windowState: "fullscreen" } };
      if (method === "Page.getLayoutMetrics") return { cssVisualViewport: { clientWidth: viewport.width, clientHeight: viewport.height } };
      if (method === "Runtime.evaluate") return { result: { value: { ...viewport, dpr: 1,
        screenWidth: viewport.width, screenHeight: viewport.height, outerWidth: Math.max(500, viewport.width), outerHeight: viewport.height } } };
      if (method === "Emulation.setDeviceMetricsOverride") {
        log.push(`metrics:${params.width}x${params.height}`);
        assert.equal(params.deviceScaleFactor, 1);
        if (failMetrics) { failMetrics = false; throw Error("CDP refused metrics"); }
        viewport = { width: params.width, height: params.height }; return {};
      }
      throw Error(`Unexpected CDP command ${method}`);
    } } } };
  const io = {
    async gate(command) { log.push(command); if (command === failGate) throw Error(`gate ${command}`); },
    mode(width, height) { log.push(`mode:${width}x${height}`);
      // A failed RandR command can still have partially changed the framebuffer.
      size = { width, height }; native.root = { xid: "0x1", ...size };
      if (failMode) { failMode = false; throw Error("xrandr: cannot find mode"); }
    },
    native(payload) {
      if (payload) {
        log.push(payload.restore ? "restore-native" : "native");
        assert.equal(payload.identity.window.xid, "0x2");
        native.window = { ...native.window, ...payload.window };
      }
      return structuredClone(native);
    },
    prune() {}, wait: async () => {},
  };
  return { page, io, log };
}

for (const [width, height] of [[320, 240], [390, 1200], [844, 338], [3840, 2160]]) {
  test(`current ${width}x${height} geometry reports DPR 1`, () => {
    assert.deepEqual(browserDisplayMetrics({ displaySurface: { stream: { width, height } } }),
      { width, height, deviceScaleFactor: 1, streamWidth: width, streamHeight: height });
  });
}

test("transaction orders writer, RandR, native window, CDP, verification and release", async () => {
  const { page, io, log } = fixture();
  for (const [sequence, width, height] of [[1, 390, 1200], [2, 844, 338], [3, 1000, 700]]) {
    const result = await resizeBrowserPage(page, { width, height, sequence }, io);
    assert.deepEqual(log.splice(0), ["acquire", "begin", `mode:${width}x${height}`, "native", `metrics:${width}x${height}`, "settle", "release"]);
    assert.equal(result.sequence, sequence);
    assert.equal(result.viewport.width, width);
    assert.equal(result.viewport.outerWidth, Math.max(500, width));
    assert.equal(result.display_session.width, width);
    assert.equal(page.config.displaySurface.stream.height, height);
    assert.equal(page.browserPage.target_id, "same-target");
  }
});

for (const failure of ["failMode", "failMetrics", "failVerify"]) {
  test(`${failure}: restore all geometry before writer settlement`, async () => {
    const { page, io, log } = fixture({ [failure]: true });
    await assert.rejects(resizeBrowserPage(page, { width: 390, height: 1200, sequence: 1 }, io));
    assert.deepEqual(log.slice(-5), ["mode:1000x700", "restore-native", "metrics:1000x700", "settle", "release"]);
    assert.deepEqual(page.config.displaySurface.stream, { width: 1000, height: 700 });
    assert.equal(page.displaySession.width, 1000);
    await assert.rejects(resizeBrowserPage(page, { width: 844, height: 338, sequence: 1 }, io), /Stale/);
  });
}

test("invalid sizes and sequences have no writer or display effects", async () => {
  const { page, io, log } = fixture();
  for (const [width, height] of [[319, 240], [3842, 2160], [390, 2162], [391, 700], [NaN, 700], ["390", 700], [390, 0]]) {
    assert.throws(() => validateBrowserGeometry(width, height));
    await assert.rejects(resizeBrowserPage(page, { width, height, sequence: 1 }, io));
  }
  for (const sequence of [0, -1, 1.5, Infinity, "1"]) await assert.rejects(resizeBrowserPage(page, { width: 390, height: 700, sequence }, io));
  assert.deepEqual(log, []);
});

test("queued older sequence is rejected after newer transaction settles", async () => {
  const { page, io, log } = fixture();
  const first = resizeBrowserPage(page, { width: 390, height: 1200, sequence: 4 }, io);
  const second = resizeBrowserPage(page, { width: 844, height: 338, sequence: 3 }, io);
  await first; await assert.rejects(second, /Stale/);
  assert.equal(log.filter(value => value === "acquire").length, 1);
});

for (const failGate of ["acquire", "begin", "settle", "release"]) {
  test(`gate ${failGate} failure reports failure and releases confirmed custody`, async () => {
    const { page, io, log } = fixture({ failGate });
    await assert.rejects(resizeBrowserPage(page, { width: 390, height: 1200, sequence: 1 }, io));
    if (failGate !== "acquire") assert.equal(log.at(-1), "release");
    if (["acquire", "begin"].includes(failGate)) assert.ok(!log.some(value => value.startsWith("mode:")));
  });
}

test("RandR uses explicit exact timings, measures actual size, and bounds mode retention", () => {
  let size = [3840, 2160]; const modes = new Set(), calls = [];
  const run = (program, args) => {
    assert.equal(program, "xrandr"); calls.push(args);
    if (args[0] === "--current") return `Screen 0: current ${size[0]} x ${size[1]}\nscreen connected\n ${[...modes].join("\n ")}`;
    if (args[0] === "--newmode") modes.add(args[1]);
    if (args[0] === "--fb") size = args[1].split("x").map(Number);
    if (args[0] === "--rmmode") modes.delete(args[1]);
    return "";
  };
  for (const [width, height] of [[1000, 700], [390, 1200], [844, 338]]) {
    const actual = applyBrowserDisplayMode(width, height, run);
    assert.equal(actual.width, width); assert.equal(actual.height, height);
    pruneBrowserDisplayModes([{ width, height }], run);
  }
  assert.equal(modes.size, 1);
  const portrait = calls.find(args => args[0] === "--newmode" && args[1] === "elastos_390x1200");
  assert.equal(portrait[3], "390"); assert.equal(portrait[7], "1200");
  assert.throws(() => applyBrowserDisplayMode(390, 1200, (program, args) => {
    if (args[0] === "--current") return "current 1000 x 700\nscreen connected\n elastos_390x1200";
    return "";
  }), /differs/);
});


test("RandR retires an unattached mode when addmode fails", () => {
  const calls = [];
  assert.throws(() => applyBrowserDisplayMode(390, 1200, (_program, args) => {
    calls.push(args);
    if (args[0] === "--current") return "current 1000 x 700\nscreen connected";
    if (args[0] === "--addmode") throw Error("mode refused");
    return "";
  }), /mode refused/);
  assert.deepEqual(calls.at(-1), ["--rmmode", "elastos_390x1200"]);
});
