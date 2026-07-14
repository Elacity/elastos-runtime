const XTERM_MODULE_URL = "./vendor/xterm/xterm.mjs?v=xterm-6.0.0";

const outputNode = document.querySelector("#terminal-output");
const xtermNode = document.querySelector("#xterm-terminal");
const terminalPanel = document.querySelector("#terminal-panel");
const terminalScreen = document.querySelector(".terminal-screen");
const homeToken = readQueryParam("home_token");
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
let pendingHostIntentTarget = "";
let pendingHostIntentTimer = 0;
let returningHome = false;

terminalPanel?.addEventListener("click", () => {
  xtermInstance?.focus?.();
});

globalThis.addEventListener?.("pagehide", () => {
  closeRuntimeTerminal({ fireAndForget: true });
});

setRuntimeTerminalMode(false);
boot().catch((error) => {
  showFallback(`Home CLI terminal could not start: ${error.message || error}`);
  requestHomeClose();
});

async function boot() {
  showFallback("Starting Home CLI terminal...");
  await startRuntimeTerminal();
}

async function startRuntimeTerminal() {
  if (runtimeTerminal) {
    return;
  }
  if (!globalThis.EventSource) {
    throw new Error("Runtime terminal stream requires EventSource support");
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
    throw new Error("Runtime terminal returned an invalid session schema");
  }

  const stream = session.stream && typeof session.stream === "object" ? session.stream : {};
  const eventsUrl = readText(stream.events_url);
  const inputUrl = readText(stream.input_url);
  const resizeUrl = readText(stream.resize_url);
  const closeUrl = readText(stream.close_url);
  if (!eventsUrl || !inputUrl || !resizeUrl || !closeUrl) {
    throw new Error("Runtime terminal session is missing stream routes");
  }
  if (eventsUrl.includes("home_token=")) {
    throw new Error("Runtime terminal refused an event stream URL containing a Home token");
  }

  await attachXtermTerminal(size);
  const source = new EventSource(eventsUrl, { withCredentials: true });
  runtimeTerminal = {
    closeSent: false,
    closeUrl,
    eventsUrl,
    inputUrl,
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
    const openedTarget = takePendingHostIntentTarget();
    cleanupRuntimeTerminal();
    if (openedTarget) {
      reattachHomeCliTerminal(openedTarget);
      return;
    }
    showFallback("Home CLI terminal stream closed. Returning to Home GUI.");
    requestHomeClose();
  };

  xtermInstance?.focus?.();
}

async function attachXtermTerminal(size) {
  if (!xtermNode) {
    throw new Error("Runtime terminal mount is missing");
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
        writeXtermStatus(`terminal resize error: ${error.message || error}`);
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
    writeXtermStatus("runtime terminal emitted invalid JSON");
    return;
  }
  if (payload?.schema !== "elastos.home-cli.terminal-event/v1") {
    writeXtermStatus("runtime terminal emitted an unsupported event schema");
    return;
  }

  const stream = readText(payload.stream);
  if ((stream === "stdout" || stream === "stderr") && payload.data != null) {
    writeRuntimeBytes(payload.data);
    return;
  }
  if (stream === "error") {
    writeXtermStatus(`runtime terminal error: ${readText(payload.message) || "unknown"}`);
    return;
  }
  if (stream !== "lifecycle") {
    return;
  }

  const message = readText(payload.message) || "lifecycle";
  if (message === "exited" || message.startsWith("closed")) {
    const code = payload.exit_code == null ? "" : ` ${payload.exit_code}`;
    const openedTarget = takePendingHostIntentTarget();
    cleanupRuntimeTerminal();
    if (openedTarget) {
      reattachHomeCliTerminal(openedTarget);
      return;
    }
    showFallback(`Home CLI terminal ${message}${code}. Returning to Home GUI.`);
    requestHomeClose();
    return;
  }

  writeXtermStatus(`runtime terminal ${message}`);
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
    writeXtermStatus(`ignored malformed host intent: ${error.message || error}`);
    return;
  }
  if (payload?.schema !== "elastos.home.terminal-host-intent/v1") {
    return;
  }
  const action = readText(payload.action);
  const target = readText(payload.target);
  if (action === "active-shell" && target) {
    requestHomeActiveShell(target);
    return;
  }
  if (action !== "open-target" || !target) {
    return;
  }
  markPendingHostIntentTarget(target);
  requestHomeOpenTarget(target, payload.query && typeof payload.query === "object" ? payload.query : {});
}

function requestHomeActiveShell(target) {
  if (target !== "home-gui") {
    writeXtermStatus(`ignored unsupported Home shell target: ${target}`);
    return;
  }
  requestHomeClose();
}

function markPendingHostIntentTarget(target) {
  pendingHostIntentTarget = target;
  if (pendingHostIntentTimer) {
    globalThis.clearTimeout?.(pendingHostIntentTimer);
  }
  pendingHostIntentTimer = globalThis.setTimeout?.(() => {
    pendingHostIntentTarget = "";
    pendingHostIntentTimer = 0;
  }, 5_000) || 0;
}

function takePendingHostIntentTarget() {
  const target = pendingHostIntentTarget;
  pendingHostIntentTarget = "";
  if (pendingHostIntentTimer) {
    globalThis.clearTimeout?.(pendingHostIntentTimer);
    pendingHostIntentTimer = 0;
  }
  return target;
}

function reattachHomeCliTerminal(target) {
  showFallback(`Opened ${target}. Reattaching Home CLI terminal...`);
  globalThis.setTimeout?.(() => {
    startRuntimeTerminal().catch((error) => {
      showFallback(`Home CLI terminal could not restart after opening ${target}: ${error.message || error}`);
    });
  }, 0);
}

function requestHomeOpenTarget(target, query = {}) {
  if (!window.parent || window.parent === window) {
    window.location.href = "/apps/home/";
    return;
  }
  window.parent.postMessage({
    type: "home:open-target",
    target,
    query,
    homeToken,
  }, window.location.origin);
}

function queueRuntimeTerminalInput(data) {
  if (!data) {
    return;
  }
  xtermSendQueue = xtermSendQueue
    .then(() => sendRuntimeTerminalInput(data))
    .catch((error) => {
      writeXtermStatus(`terminal input error: ${error.message || error}`);
    });
}

async function sendRuntimeTerminalInput(data) {
  if (!runtimeTerminal) {
    throw new Error("No Runtime terminal is attached");
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

function requestHomeClose() {
  if (returningHome) {
    return;
  }
  returningHome = true;
  if (!window.parent || window.parent === window) {
    window.location.href = "/apps/home/";
    return;
  }
  window.parent.postMessage({
    type: "home:close-self",
    activeShell: "home-gui",
    homeToken,
  }, window.location.origin);
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

function readText(value) {
  return typeof value === "string" ? value.trim() : "";
}
