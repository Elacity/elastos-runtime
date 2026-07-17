const XTERM_MODULE_URL = "./vendor/xterm/xterm.mjs?v=xterm-6.0.0";

const outputNode = document.querySelector("#terminal-output");
const xtermNode = document.querySelector("#xterm-terminal");
const terminalPanel = document.querySelector("#terminal-panel");
const terminalScreen = document.querySelector(".terminal-screen");
const homeToken = readLaunchToken();
const homeParentOrigin = readQueryParam("home_origin");
const HOST_INTENT_OSC_PREFIX = "\x1b]777;elastos-home-intent=";
const HOST_INTENT_OSC_SUFFIX = "\x07";
const PTY_VISIBLE_ROW_GUARD = 2;

let runtimeTerminal = null;
let xtermModulePromise = null;
let xtermInstance = null;
let xtermInputDisposable = null;
let xtermResizeObserver = null;
let xtermSendQueue = Promise.resolve();
let hostIntentControlBuffer = "";
let pendingHomeReturn = false;
let signingOut = false;
let terminalRestartCount = 0;
const TERMINAL_RESTART_LIMIT = 1;

terminalPanel?.addEventListener("click", () => {
  xtermInstance?.focus?.();
});

globalThis.addEventListener?.("pagehide", () => {
  closeRuntimeTerminal({ fireAndForget: true });
});

setRuntimeTerminalMode(false);
boot().catch((error) => {
  console.error("Home CLI terminal could not start", error);
  showFallback("Home CLI could not start. Return to the Desktop and try again.");
});

async function boot() {
  if (!homeToken || !homeParentOrigin || window.parent === window) {
    showFallback("Open Home CLI from Home.");
    return;
  }
  showFallback("Starting Home CLI terminal...");
  await startRuntimeTerminal();
}

async function startRuntimeTerminal() {
  if (runtimeTerminal) {
    return;
  }
  if (!globalThis.EventSource) {
    throw new Error("This browser cannot open Home CLI.");
  }

  const size = measureTerminalSize();
  const session = await fetchJson("/api/apps/home-cli/terminal/sessions", {
    method: "POST",
    headers: homeHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({
      schema: "elastos.home-cli.terminal-start/v1",
      cols: size.cols,
      rows: size.rows,
    }),
  });
  if (session?.schema !== "elastos.home-cli.terminal-session/v1") {
    throw new Error("Home CLI could not start.");
  }

  const stream = session.stream && typeof session.stream === "object" ? session.stream : {};
  const eventsUrl = readText(stream.events_url);
  const inputUrl = readText(stream.input_url);
  const resizeUrl = readText(stream.resize_url);
  const intentUrl = readText(stream.intent_url);
  const closeUrl = readText(stream.close_url);
  if (!eventsUrl || !inputUrl || !resizeUrl || !intentUrl || !closeUrl) {
    throw new Error("Home CLI could not start.");
  }
  if (eventsUrl.includes("home_token=")) {
    throw new Error("Home CLI could not start securely.");
  }

  await attachXtermTerminal(size);
  const source = new EventSource(eventsUrl);
  runtimeTerminal = {
    closeSent: false,
    closeUrl,
    eventsUrl,
    inputUrl,
    intentUrl,
    resizeUrl,
    sessionId: readText(session.session_id),
    size,
    source,
  };
  setRuntimeTerminalMode(true);

  source.addEventListener("terminal", (event) => {
    handleRuntimeTerminalEvent(event);
  });
  source.onerror = () => {
    if (runtimeTerminal?.source !== source) {
      return;
    }
    console.warn("Home CLI terminal stream interrupted; waiting for EventSource reconnect");
  };

  xtermInstance?.focus?.();
}

async function attachXtermTerminal(size) {
  if (!xtermNode) {
    throw new Error("Home CLI display is unavailable.");
  }

  detachXtermTerminal();
  const { Terminal } = await loadXtermModule();
  xtermNode.hidden = false;
  xtermNode.replaceChildren?.();
  xtermInstance = new Terminal({
    cols: size.cols,
    rows: size.rows,
    convertEol: false,
    cursorBlink: true,
    fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace',
    fontSize: 14,
    scrollback: 5000,
    theme: {
      background: "#050608",
      foreground: "#d7f5ea",
      cursor: "#f6921a",
      selectionBackground: "#39485f",
      black: "#0d1117",
      red: "#ff7b72",
      green: "#7ee787",
      yellow: "#f2cc60",
      blue: "#79c0ff",
      magenta: "#d2a8ff",
      cyan: "#76e3ea",
      white: "#f0f6fc",
      brightBlack: "#8b949e",
      brightRed: "#ff7b72",
      brightGreen: "#7ee787",
      brightYellow: "#f2cc60",
      brightBlue: "#79c0ff",
      brightMagenta: "#d2a8ff",
      brightCyan: "#76e3ea",
      brightWhite: "#ffffff",
    },
  });
  xtermInstance.open(xtermNode);
  xtermInstance.resize?.(size.cols, size.rows);
  xtermInputDisposable = xtermInstance.onData?.((data) => {
    queueRuntimeTerminalInput(data);
  });

  if (globalThis.ResizeObserver) {
    xtermResizeObserver = new ResizeObserver(() => {
      const next = measureTerminalSize();
      xtermInstance?.resize?.(next.cols, next.rows);
      resizeRuntimeTerminal(next).catch((error) => {
        console.error("Home CLI resize failed", error);
        writeXtermStatus("Home CLI could not resize.");
      });
    });
    xtermResizeObserver.observe(xtermNode);
  }
}

function detachXtermTerminal() {
  xtermInputDisposable?.dispose?.();
  xtermInputDisposable = null;
  xtermResizeObserver?.disconnect?.();
  xtermResizeObserver = null;
  xtermInstance?.dispose?.();
  xtermInstance = null;
  xtermSendQueue = Promise.resolve();
  if (xtermNode) {
    xtermNode.hidden = true;
    xtermNode.replaceChildren?.();
  }
}

async function loadXtermModule() {
  if (globalThis.__ELASTOS_TEST_XTERM__) {
    return { Terminal: globalThis.__ELASTOS_TEST_XTERM__ };
  }
  if (!xtermModulePromise) {
    xtermModulePromise = import(XTERM_MODULE_URL);
  }
  return xtermModulePromise;
}

function handleRuntimeTerminalEvent(event) {
  let payload = null;
  try {
    payload = JSON.parse(event?.data || "{}");
  } catch (_error) {
    writeXtermStatus("Home CLI received an unreadable update.");
    return;
  }
  if (payload?.schema !== "elastos.home-cli.terminal-event/v1") {
    writeXtermStatus("Home CLI received an unreadable update.");
    return;
  }

  const stream = readText(payload.stream);
  if ((stream === "stdout" || stream === "stderr") && payload.data != null) {
    writeRuntimeBytes(payload.data);
    return;
  }
  if (stream === "error") {
    console.error("Home CLI terminal error", readText(payload.message));
    writeXtermStatus("Home CLI stopped unexpectedly.");
    return;
  }
  if (stream !== "lifecycle") {
    return;
  }

  const message = readText(payload.message) || "lifecycle";
  if (message === "exited" || message.startsWith("closed")) {
    cleanupRuntimeTerminal();
    if (pendingHomeReturn || signingOut) {
      return;
    }
    reattachHomeCliTerminal("Home CLI closed unexpectedly. Reconnecting...");
    return;
  }

  writeXtermStatus(`Home CLI ${message}`);
}

function writeRuntimeBytes(data) {
  const text = consumeHostIntentControls(String(data || ""));
  if (text) {
    xtermInstance?.write?.(text);
  }
}

function consumeHostIntentControls(data) {
  let text = `${hostIntentControlBuffer}${data}`;
  hostIntentControlBuffer = "";
  let output = "";
  let cursor = 0;

  while (cursor < text.length) {
    const start = text.indexOf(HOST_INTENT_OSC_PREFIX, cursor);
    if (start === -1) {
      output += text.slice(cursor);
      break;
    }
    output += text.slice(cursor, start);
    const payloadStart = start + HOST_INTENT_OSC_PREFIX.length;
    const end = text.indexOf(HOST_INTENT_OSC_SUFFIX, payloadStart);
    if (end === -1) {
      hostIntentControlBuffer = text.slice(start);
      break;
    }
    handleHostIntentControl(text.slice(payloadStart, end));
    cursor = end + HOST_INTENT_OSC_SUFFIX.length;
  }

  return output;
}

function handleHostIntentControl(rawPayload) {
  let payload = null;
  try {
    payload = JSON.parse(rawPayload || "{}");
  } catch (error) {
    console.error("Home CLI ignored an unreadable action", error);
    return;
  }
  if (payload?.schema !== "elastos.home.terminal-host-intent/v1") {
    return;
  }
  if (payload.action === "active-shell" && payload.target === "home-gui") {
    pendingHomeReturn = true;
  }
  authorizeRuntimeHostIntent(payload)
    .then((authorized) => {
      applyRuntimeHostIntent(authorized);
    })
    .catch((error) => {
      pendingHomeReturn = false;
      console.error("Home CLI action was not authorized", error);
      writeXtermStatus("This action is not available.");
    });
}

async function authorizeRuntimeHostIntent(payload) {
  if (!runtimeTerminal?.intentUrl) {
    throw new Error("Home CLI cannot run this action.");
  }
  const response = await fetchJson(runtimeTerminal.intentUrl, {
    method: "POST",
    headers: homeHeaders({ "content-type": "application/json" }),
    body: JSON.stringify(payload),
  });
  if (response?.schema !== "elastos.home-cli.terminal-intent/v1") {
    throw new Error("Home could not complete this action.");
  }
  const intent = response.intent && typeof response.intent === "object" ? response.intent : null;
  if (intent?.schema !== "elastos.home.terminal-host-intent/v1") {
    throw new Error("Home could not complete this action.");
  }
  return intent;
}

function applyRuntimeHostIntent(payload) {
  const action = readText(payload.action);
  const target = readText(payload.target);
  if (action === "sign-out" && target === "home") {
    requestHomeSignOut();
    return;
  }
  if (action === "active-shell" && target) {
    requestHomeActiveShell(target).catch((error) => {
      pendingHomeReturn = false;
      console.error("Home CLI shell switch failed", error);
      reattachHomeCliTerminal("The Home view did not change. Try again.");
    });
    return;
  }
  if (action !== "switch-shell-open-target" || !target) {
    return;
  }
  pendingHomeReturn = true;
  requestHomeGuiTarget(target, payload.query && typeof payload.query === "object" ? payload.query : {});
}

function requestHomeSignOut() {
  if (signingOut) {
    return;
  }
  signingOut = true;
  closeRuntimeTerminal({ fireAndForget: true });
  if (!window.parent || window.parent === window) {
    showFallback("Open Home CLI from Home.");
    return;
  }
  window.parent.postMessage({
    type: "home:sign-out",
    homeToken,
  }, homeParentOrigin);
}

async function requestHomeActiveShell(target) {
  if (target !== "home-gui") {
    writeXtermStatus("That Home view is not available.");
    return;
  }
  const summary = await fetchJson("/api/apps/home/active-shell", {
    method: "POST",
    headers: homeHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ active: target }),
  });
  if (readText(summary?.active) !== target) {
    throw new Error("Runtime did not apply the selected Home view.");
  }
  window.parent.postMessage({
    type: "home:active-shell-applied",
    activeShell: target,
    homeToken,
  }, homeParentOrigin);
}

function reattachHomeCliTerminal(message = "") {
  if (terminalRestartCount >= TERMINAL_RESTART_LIMIT) {
    showFallback("Home CLI terminal stopped. Refresh Home CLI or switch shell explicitly from System.");
    return;
  }
  terminalRestartCount += 1;
  showFallback(message);
  globalThis.setTimeout?.(() => {
    startRuntimeTerminal().catch((error) => {
      console.error("Home CLI could not restart", error);
      showFallback("Home CLI could not reconnect. Return to the Desktop and try again.");
    });
  }, 0);
}

function requestHomeGuiTarget(target, query = {}) {
  if (!window.parent || window.parent === window) {
    showFallback("Open Home CLI from Home.");
    return;
  }
  window.parent.postMessage({
    type: "home:switch-shell-and-open-target",
    requestId: window.crypto?.randomUUID?.() || `home-cli-${Date.now()}`,
    target,
    query,
    homeToken,
  }, homeParentOrigin);
}

function queueRuntimeTerminalInput(data) {
  if (!data) {
    return;
  }
  xtermSendQueue = xtermSendQueue
    .then(() => sendRuntimeTerminalInput(data))
    .catch((error) => {
      console.error("Home CLI input failed", error);
      writeXtermStatus("Home CLI could not send that input.");
    });
}

async function sendRuntimeTerminalInput(data) {
  if (!runtimeTerminal) {
    throw new Error("Home CLI is not connected.");
  }
  await fetchJson(runtimeTerminal.inputUrl, {
    method: "POST",
    headers: homeHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({
      schema: "elastos.home-cli.terminal-input/v1",
      data,
    }),
  });
}

async function resizeRuntimeTerminal(size) {
  if (!runtimeTerminal) {
    return;
  }
  if (runtimeTerminal.size?.cols === size.cols && runtimeTerminal.size?.rows === size.rows) {
    return;
  }
  await fetchJson(runtimeTerminal.resizeUrl, {
    method: "POST",
    headers: homeHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({
      schema: "elastos.home-cli.terminal-resize/v1",
      cols: size.cols,
      rows: size.rows,
    }),
  });
  if (runtimeTerminal) {
    runtimeTerminal.size = size;
  }
}

function closeRuntimeTerminal(options = {}) {
  const terminal = runtimeTerminal;
  if (!terminal) {
    return Promise.resolve();
  }

  runtimeTerminal = null;
  terminal.source?.close?.();
  setRuntimeTerminalMode(false);

  if (terminal.closeSent) {
    return Promise.resolve();
  }
  terminal.closeSent = true;
  const closeRequest = fetchJson(terminal.closeUrl, {
    method: "POST",
    headers: homeHeaders({ "content-type": "application/json" }),
    keepalive: options.fireAndForget === true,
  });
  if (options.fireAndForget === true) {
    closeRequest.catch(() => {});
    return Promise.resolve();
  }
  return closeRequest;
}

function cleanupRuntimeTerminal() {
  if (runtimeTerminal?.source) {
    runtimeTerminal.source.close?.();
  }
  runtimeTerminal = null;
  setRuntimeTerminalMode(false);
}

function setRuntimeTerminalMode(attached) {
  if (document.body?.dataset) {
    document.body.dataset.runtimeTerminal = attached ? "attached" : "idle";
  }
  if (terminalPanel?.dataset) {
    terminalPanel.dataset.runtimeTerminal = attached ? "attached" : "idle";
  }
  if (xtermNode) {
    xtermNode.hidden = !attached;
  }
  if (!attached) {
    detachXtermTerminal();
  }
}

function showFallback(message) {
  if (document.body?.dataset) {
    document.body.dataset.runtimeTerminal = "idle";
  }
  if (terminalPanel?.dataset) {
    terminalPanel.dataset.runtimeTerminal = "idle";
  }
  if (outputNode) {
    outputNode.textContent = `${String(message || "").trim()}\n`;
  }
}

function writeXtermStatus(message) {
  const text = String(message || "").trim();
  if (!text) {
    return;
  }
  if (xtermInstance) {
    xtermInstance.writeln?.(`\r\n${text}`);
    return;
  }
  showFallback(text);
}

async function fetchJson(url, init = {}) {
  const response = await fetch(url, init);
  if (!response.ok) {
    const detail = await response.text().catch(() => "");
    throw new Error(detail || `request failed: ${response.status}`);
  }
  return response.json();
}

function homeHeaders(extra = {}) {
  return {
    ...extra,
    ...(homeToken ? { "x-elastos-home-token": homeToken } : {}),
  };
}

function measureTerminalSize() {
  const rect = xtermNode?.getBoundingClientRect?.() || terminalScreen?.getBoundingClientRect?.() || {};
  const width = Number(rect.width) || Number(terminalScreen?.clientWidth) || 1000;
  const height = Number(rect.height) || Number(terminalScreen?.clientHeight) || 520;
  return {
    cols: clamp(Math.floor((width - 24) / 8.4), 40, 180),
    rows: clamp(Math.floor((height - 20) / 18) - PTY_VISIBLE_ROW_GUARD, 12, 80),
  };
}

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, Number.isFinite(value) ? value : min));
}

function readQueryParam(name) {
  try {
    return new URL(window.location.href).searchParams.get(name) || "";
  } catch {
    return "";
  }
}

function readLaunchToken() {
  try {
    return new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
  } catch {
    return "";
  }
}

function readText(value) {
  return typeof value === "string" ? value.trim() : "";
}
