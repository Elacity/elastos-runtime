import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const browserSource = readFileSync(new URL("../capsules/browser/browser/browser.js", import.meta.url), "utf8");
const geometrySource = readFileSync(new URL("../capsules/home-gui/browser/shell-window-geometry.js", import.meta.url), "utf8");
const coreSource = readFileSync(new URL("../capsules/home-gui/browser/shell-core.js", import.meta.url), "utf8");

function declaration(source, name) {
  const start = source.search(new RegExp(`(?:export )?function ${name}\\(`));
  assert.ok(start >= 0, name);
  const end = source.indexOf("\n}\n", start);
  assert.ok(end > start, name);
  return source.slice(start, end + 2).replace(/^export /, "");
}

function stringConstant(source, name) {
  const match = source.match(new RegExp(`(?:export )?const ${name} = "([^"]+)";`));
  assert.ok(match, name);
  return match[1];
}

const MESSAGE_TYPE = stringConstant(geometrySource, "WINDOW_DRAG_MESSAGE_TYPE");

test("Browser and Home name the same window-drag message", () => {
  assert.equal(stringConstant(browserSource, "WINDOW_DRAG_MESSAGE_TYPE"), MESSAGE_TYPE);
});

function browserToolbar({ launchToken = "frame-token" } = {}) {
  const posted = [];
  const listeners = {};
  const captured = [];
  const chrome = {
    addEventListener: (type, listener) => { listeners[type] = listener; },
    setPointerCapture: (id) => captured.push(id),
  };
  const context = vm.createContext({
    WINDOW_DRAG_MESSAGE_TYPE: MESSAGE_TYPE,
    launchToken,
    window: { parent: { postMessage: (message, origin) => posted.push({ message, origin }) } },
  });
  vm.runInContext(`${declaration(browserSource, "installChromeWindowDrag")}\nthis.install = installChromeWindowDrag;`, context);
  context.install(chrome);
  const press = (type, fields) => {
    let prevented = false;
    listeners[type]({
      button: 0, pointerId: 1, clientX: 0, clientY: 0,
      target: { closest: () => null },
      preventDefault: () => { prevented = true; },
      ...fields,
    });
    return prevented;
  };
  return { posted, captured, press, listeners };
}

test("an empty part of the Browser toolbar sends start, move and end to Home", () => {
  const toolbar = browserToolbar();
  assert.equal(toolbar.press("pointerdown", { clientX: 300, clientY: 20 }), true);
  toolbar.press("pointermove", { clientX: 180, clientY: 100 });
  toolbar.press("pointerup", { clientX: 180, clientY: 100 });
  toolbar.press("lostpointercapture", {});
  assert.deepEqual(toolbar.captured, [1]);
  assert.deepEqual(
    toolbar.posted.map(({ message, origin }) => [message.type, message.phase, message.clientX, message.clientY, origin]),
    [
      [MESSAGE_TYPE, "start", 300, 20, "*"],
      [MESSAGE_TYPE, "move", 180, 100, "*"],
      [MESSAGE_TYPE, "end", 180, 100, "*"],
    ],
  );
  assert.ok(toolbar.posted.every(({ message }) => message.homeToken === "frame-token"), "each message carries the frame's launch token");
});

test("a Browser frame without a launch token sends no drag messages", () => {
  const toolbar = browserToolbar({ launchToken: "" });
  assert.deepEqual(Object.keys(toolbar.listeners), []);
  assert.deepEqual(toolbar.posted, []);
});

test("toolbar controls, other buttons and a second pointer keep their own behavior", () => {
  const toolbar = browserToolbar();
  const control = { closest: (selector) => (selector.includes("input") ? {} : null) };
  assert.equal(toolbar.press("pointerdown", { target: control }), false);
  assert.equal(toolbar.press("pointerdown", { button: 2 }), false);
  toolbar.press("pointermove", {});
  assert.deepEqual(toolbar.posted, []);

  toolbar.press("pointerdown", {});
  assert.equal(toolbar.press("pointerdown", { pointerId: 2 }), false);
  toolbar.press("pointermove", { pointerId: 2 });
  toolbar.press("pointerup", { pointerId: 2 });
  assert.deepEqual(toolbar.posted.map(({ message }) => message.phase), ["start"]);
});

function homeWindow({ mode = "unified-toolbar", maximized = false, hidden = false, frameToken = "frame-token" } = {}) {
  const listeners = new Set();
  const interactions = { begun: 0, ended: 0, saved: 0, focused: [] };
  const frameWindow = {};
  const classes = new Set(hidden ? ["window", "hidden"] : ["window"]);
  const nodeListeners = new Set();
  const node = {
    isConnected: true,
    addEventListener: (type, listener, capture) => type === "load" && capture === true && nodeListeners.add(listener),
    removeEventListener: (type, listener, capture) => type === "load" && capture === true && nodeListeners.delete(listener),
    offsetWidth: 800,
    classList: { contains: (name) => classes.has(name), add: (name) => classes.add(name) },
    dataset: { windowId: "browser-1", windowChromeMode: mode, ...(maximized ? { maximized: "true" } : {}) },
    style: { left: "100px", top: "50px" },
  };
  const frame = {
    classList: { contains: (name) => name === "window-frame" },
    contentWindow: frameWindow,
    getBoundingClientRect: () => ({ left: parseFloat(node.style.left) + 1, top: parseFloat(node.style.top) + 1 }),
  };
  node.querySelector = (selector) => (selector === ".window-frame" ? frame : null);
  const context = vm.createContext({
    WINDOW_DRAG_MESSAGE_TYPE: MESSAGE_TYPE,
    WINDOW_CHROME_UNIFIED_TOOLBAR: stringConstant(coreSource, "WINDOW_CHROME_UNIFIED_TOOLBAR"),
    window: {
      addEventListener: (type, listener) => type === "message" && listeners.add(listener),
      removeEventListener: (type, listener) => listeners.delete(listener),
    },
    desktop: { getBoundingClientRect: () => ({ left: 0, top: 30 }) },
    clamp: (value, min, max) => Math.min(Math.max(value, min), max),
    normalWindowBounds: (windowNode) => ({ x: parseFloat(windowNode.style.left), y: parseFloat(windowNode.style.top) }),
    partialDragBoundsForWindow: () => ({ minX: -1000, maxX: 5000, minY: 0, maxY: 5000 }),
    hideWindowSnapPreview: () => {},
    beginShellInteraction: () => { interactions.begun += 1; },
    endShellInteraction: () => { interactions.ended += 1; },
  });
  vm.runInContext(
    `${declaration(geometrySource, "safeClamp")}\n${declaration(geometrySource, "attachForwardedWindowDrag")}\nthis.attach = attachForwardedWindowDrag;`,
    context,
  );
  const detach = context.attach(
    node,
    (id) => interactions.focused.push(id),
    () => { interactions.saved += 1; },
    () => frameToken,
  );
  // The Browser reports positions inside its own frame, which moves with the window.
  const send = (phase, screenX, screenY, source = frameWindow, { origin = "null", homeToken = "frame-token", extra = {} } = {}) => {
    const rect = frame.getBoundingClientRect();
    for (const listener of [...listeners]) {
      listener({
        source,
        origin,
        data: { type: MESSAGE_TYPE, homeToken, phase, clientX: screenX - rect.left, clientY: screenY - rect.top, ...extra },
      });
    }
  };
  const position = () => [parseFloat(node.style.left), parseFloat(node.style.top)];
  const loadFrame = (target = frame) => {
    for (const listener of [...nodeListeners]) {
      listener({ target });
    }
  };
  return { node, interactions, send, position, detach, listeners, nodeListeners, loadFrame };
}

test("Home moves the Browser window with the pointer and saves the place on release", () => {
  const home = homeWindow();
  home.send("start", 400, 80);
  home.send("move", 340, 120);
  home.send("move", 280, 160);
  home.send("end", 280, 160);
  assert.deepEqual(home.position(), [-20, 130]);
  assert.deepEqual(home.interactions, { begun: 1, ended: 1, saved: 1, focused: ["browser-1"] });
});

test("Home ignores drag messages from other frames, other chrome modes, maximized and hidden windows", () => {
  const stranger = homeWindow();
  stranger.send("start", 400, 80, {});
  stranger.send("move", 280, 160, {});
  assert.deepEqual(stranger.position(), [100, 50]);

  const standard = homeWindow({ mode: "standard" });
  standard.send("start", 400, 80);
  standard.send("move", 280, 160);
  assert.deepEqual(standard.position(), [100, 50]);

  const maximized = homeWindow({ maximized: true });
  maximized.send("start", 400, 80);
  maximized.send("move", 280, 160);
  assert.deepEqual(maximized.position(), [100, 50]);
  assert.equal(maximized.interactions.begun, 0);

  const hidden = homeWindow({ hidden: true });
  hidden.send("start", 400, 80);
  hidden.send("move", 280, 160);
  assert.deepEqual(hidden.position(), [100, 50]);
  assert.deepEqual(hidden.interactions, { begun: 0, ended: 0, saved: 0, focused: [] });
});

test("a window that is snapped, maximized or hidden during a drag stops following the pointer", () => {
  for (const change of [
    (node) => { node.dataset.snap = "left"; },
    (node) => { node.dataset.maximized = "true"; },
    (node) => { node.classList.add("hidden"); },
  ]) {
    const home = homeWindow();
    home.send("start", 400, 80);
    home.send("move", 340, 120);
    const placed = home.position();
    change(home.node);
    home.send("move", 280, 160);
    home.send("end", 280, 160);
    assert.deepEqual(home.position(), placed);
    assert.deepEqual([home.interactions.begun, home.interactions.ended, home.interactions.saved], [1, 1, 1]);
  }
});

test("closing the window removes its drag listener and ends an active drag", () => {
  const home = homeWindow();
  home.send("start", 400, 80);
  home.detach();
  assert.equal(home.listeners.size, 0);
  assert.equal(home.nodeListeners.size, 0);
  assert.equal(home.interactions.ended, 1);
});

test("a frame that loads again mid-drag ends the drag", () => {
  const home = homeWindow();
  home.send("start", 400, 80);
  home.send("move", 340, 120);
  home.loadFrame({ classList: { contains: () => false } });
  assert.equal(home.interactions.ended, 0, "a load inside another element is not the frame");
  home.loadFrame();
  const placed = home.position();
  assert.deepEqual([home.interactions.begun, home.interactions.ended, home.interactions.saved], [1, 1, 1]);
  home.send("move", 280, 160);
  assert.deepEqual(home.position(), placed, "moves after the reload are ignored until a new start");
});

test("Home ignores drag messages that are not bound to this frame's launch", () => {
  const cases = [
    ["another origin", { origin: "https://evil.example" }],
    ["the Home origin", { origin: "http://127.0.0.1:3000" }],
    ["another launch token", { homeToken: "other-token" }],
    ["no launch token", { homeToken: "" }],
    ["an extra field", { extra: { windowId: "browser-2" } }],
  ];
  for (const [name, fields] of cases) {
    const home = homeWindow();
    home.send("start", 400, 80, undefined, fields);
    home.send("move", 280, 160, undefined, fields);
    home.send("end", 280, 160, undefined, fields);
    assert.deepEqual(home.position(), [100, 50], name);
    assert.deepEqual(home.interactions, { begun: 0, ended: 0, saved: 0, focused: [] }, name);
  }

  // A window whose frame Home holds no launch token for cannot be moved at all.
  const unlaunched = homeWindow({ frameToken: "" });
  unlaunched.send("start", 400, 80, undefined, { homeToken: "" });
  unlaunched.send("move", 280, 160, undefined, { homeToken: "" });
  assert.deepEqual(unlaunched.position(), [100, 50]);

  const badPhase = homeWindow();
  badPhase.send("start", 400, 80);
  badPhase.send("teleport", 0, 0);
  assert.deepEqual(badPhase.position(), [100, 50], "an unknown phase is ignored, not treated as a move");
  assert.equal(badPhase.interactions.ended, 0, "and does not end the drag");
});
