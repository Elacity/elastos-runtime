import { BUTTON_BITS, gamepadMask as readGamepadMask } from "./gba-input.js";

const VIEWER_ID = "gba-emulator";
const MAX_ROM_BYTES = 64 * 1024 * 1024;
const query = new URLSearchParams(window.location.search);
const homeToken = query.get("home_token") || "";
const hostMode = window.self === window.top ? "standalone" : "embedded";
document.body.dataset.emulatorAccessMode = homeToken ? "shell" : "gateway";
document.body.dataset.emulatorHostMode = hostMode;
document.body.dataset.shellWindowFit = hostMode === "embedded" ? "fixed" : "auto";
const canvas = document.getElementById("canvas");
const emptyState = document.getElementById("drop-zone");
const installedGames = document.getElementById("rom-library-list");
const status = document.getElementById("status");
const pauseButton = document.getElementById("btn-pause");
const fastForwardButton = document.getElementById("btn-ff");
const volume = document.getElementById("volume-slider");
const fullscreenButton = document.getElementById("btn-fullscreen");
const screenBezel = document.getElementById("screen-bezel");
const powerLed = document.getElementById("power-led");

const KEY_BUTTONS = {
  KeyX: "a",
  KeyZ: "b",
  Backspace: "select",
  Enter: "start",
  ArrowRight: "right",
  ArrowLeft: "left",
  ArrowUp: "up",
  ArrowDown: "down",
  KeyS: "r",
  KeyA: "l",
};

let enginePromise = null;
let engine = null;
let gameLoaded = false;
let paused = false;
let fastForward = false;
let soundEnabled = false;
let keyboardMask = 0;
let gamepadMask = 0;
let appliedInputMask = 0;
let inputFrame = 0;
let saveTimer = 0;
let activeSaveName = "";
let activeRomId = "";
const touchPointers = new Map();

function launchHeaders() {
  return homeToken ? { "x-elastos-home-token": homeToken } : {};
}

function showStatus(message, error = false) {
  status.textContent = message;
  status.classList.toggle("error", error);
  status.hidden = !message;
}

function setIconButton(button, glyph, label) {
  button.querySelector(".icon-glyph").textContent = glyph;
  button.setAttribute("aria-label", label);
  button.title = label;
}

function syncPauseButton() {
  setIconButton(pauseButton, paused ? "▶" : "❚❚", paused ? "Resume" : "Pause");
  pauseButton.setAttribute("aria-pressed", String(paused));
}

function syncFullscreenButton() {
  const active = Boolean(document.fullscreenElement);
  setIconButton(fullscreenButton, active ? "⤡" : "⤢", active ? "Exit fullscreen" : "Fullscreen");
  fullscreenButton.setAttribute("aria-pressed", String(active));
}

function setSlotState(slot, saved) {
  const loadButton = document.getElementById(`btn-load${slot}`);
  const slotStatus = document.getElementById(`slot-status${slot}`);
  loadButton.dataset.saved = String(saved);
  loadButton.title = saved ? `Load state ${slot}` : `State ${slot} is empty`;
  loadButton.disabled = !gameLoaded || !saved;
  slotStatus.textContent = saved ? "Saved" : "Empty";
  slotStatus.dataset.slotState = saved ? "saved" : "empty";
}

function setGameControlsEnabled(enabled) {
  pauseButton.disabled = !enabled;
  fastForwardButton.disabled = !enabled;
  for (let slot = 1; slot <= 3; slot += 1) {
    document.getElementById(`btn-save${slot}`).disabled = !enabled;
    const loadButton = document.getElementById(`btn-load${slot}`);
    loadButton.disabled = !enabled || loadButton.dataset.saved !== "true";
  }
}

function withTimeout(promise, timeoutMs, message) {
  let timer = 0;
  return Promise.race([
    promise,
    new Promise((_, reject) => {
      timer = window.setTimeout(() => reject(new Error(message)), timeoutMs);
    }),
  ]).finally(() => window.clearTimeout(timer));
}

function assertPortableEngineSupport() {
  if (typeof WebAssembly !== "object" || typeof Worker !== "function") {
    throw new Error("This browser cannot run the GBA engine.");
  }
  if (!window.crossOriginIsolated || typeof SharedArrayBuffer !== "function") {
    throw new Error("This browser does not provide isolated WebAssembly threads.");
  }
  new WebAssembly.Memory({ initial: 1, maximum: 1, shared: true });
}

function ensureDirectory(path) {
  if (!engine.FS.analyzePath(path).exists) engine.FS.mkdir(path);
}

async function loadEngine() {
  if (!enginePromise) {
    enginePromise = (async () => {
      assertPortableEngineSupport();
      const { default: createMgba } = await import("./mgba.js");
      engine = await withTimeout(
        createMgba({ canvas }),
        15_000,
        "The GBA engine did not start.",
      );
      ensureDirectory("/data");
      ensureDirectory("/data/games");
      ensureDirectory("/data/saves");
      ensureDirectory("/data/states");
      ensureDirectory("/data/cheats");
      ensureDirectory("/autosave");
      engine.setVolume(0);
      return engine;
    })().catch((error) => {
      enginePromise = null;
      engine = null;
      throw error;
    });
  }
  return enginePromise;
}

function requestedGame() {
  const capsule = query.get("capsule");
  if (capsule) return { capsule };
  const objectUri = query.get("objectUri") || query.get("uri");
  return objectUri ? { objectUri } : null;
}

function fileNameFromUri(uri) {
  const segment = String(uri).split("/").filter(Boolean).pop() || "game.gba";
  try {
    return decodeURIComponent(segment);
  } catch {
    return segment;
  }
}

async function fetchBytes(path) {
  const response = await fetch(path, { headers: launchHeaders() });
  if (!response.ok) {
    const detail = await response.text().catch(() => "");
    throw new Error(detail || `Game data is unavailable (${response.status}).`);
  }
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (!bytes.length || bytes.length > MAX_ROM_BYTES) {
    throw new Error("The selected file is not a valid GBA game.");
  }
  return bytes;
}

async function readGame(request) {
  if (request.capsule) {
    return {
      bytes: await fetchBytes(
        `/api/viewers/${VIEWER_ID}/content/${encodeURIComponent(request.capsule)}`,
      ),
      fileName: `${request.capsule}.gba`,
    };
  }
  if (request.objectUri) {
    const search = new URLSearchParams({ uri: request.objectUri, raw: "true" });
    return {
      bytes: await fetchBytes(`/api/viewers/${VIEWER_ID}/library-object?${search}`),
      fileName: fileNameFromUri(request.objectUri),
    };
  }
  throw new Error("Choose a GBA game from Library.");
}

async function sha256Hex(bytes) {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function saveUrl(name) {
  return `/api/viewers/${VIEWER_ID}/storage/${VIEWER_ID}/save/${encodeURIComponent(name)}`;
}

function stateName(slot) {
  return activeRomId ? `${activeRomId}.ss${slot}` : "";
}

function stateUrl(slot) {
  return `/api/viewers/${VIEWER_ID}/storage/${VIEWER_ID}/state/${encodeURIComponent(stateName(slot))}`;
}

function statePath(slot) {
  return `/data/states/${stateName(slot)}`;
}

async function restoreSave(name) {
  const response = await fetch(saveUrl(name), { headers: launchHeaders() });
  if (response.status === 404) return null;
  if (!response.ok) throw new Error(`Saved game data is unavailable (${response.status}).`);
  return new Uint8Array(await response.arrayBuffer());
}

async function persistSave(keepalive = false) {
  if (!engine || !gameLoaded || !activeSaveName) return;
  const bytes = engine.getSave();
  if (!bytes?.length) return;
  const response = await fetch(saveUrl(activeSaveName), {
    method: "PUT",
    headers: launchHeaders(),
    body: bytes,
    keepalive,
  });
  if (!response.ok) throw new Error(`Saved game data could not be stored (${response.status}).`);
}

async function readState(slot) {
  const response = await fetch(stateUrl(slot), { headers: launchHeaders() });
  if (response.status === 404) return null;
  if (!response.ok) throw new Error(`State ${slot} is unavailable (${response.status}).`);
  return new Uint8Array(await response.arrayBuffer());
}

async function waitForState(slot) {
  for (let attempt = 0; attempt < 30; attempt += 1) {
    try {
      const bytes = engine.FS.readFile(statePath(slot));
      if (bytes?.length) return bytes;
    } catch {
      // mGBA creates the state file asynchronously.
    }
    await new Promise((resolve) => window.setTimeout(resolve, 50));
  }
  throw new Error(`State ${slot} was not created.`);
}

async function saveState(slot) {
  if (!engine?.saveState(slot)) throw new Error(`State ${slot} could not be saved.`);
  const response = await fetch(stateUrl(slot), {
    method: "PUT",
    headers: launchHeaders(),
    body: await waitForState(slot),
  });
  if (!response.ok) throw new Error(`State ${slot} could not be stored (${response.status}).`);
  setSlotState(slot, true);
  showStatus(`State ${slot} saved`);
}

async function loadState(slot) {
  const bytes = await readState(slot);
  if (!bytes?.length) {
    setSlotState(slot, false);
    throw new Error(`State ${slot} is empty.`);
  }
  engine.FS.writeFile(statePath(slot), bytes);
  if (!engine.loadState(slot)) throw new Error(`State ${slot} could not be loaded.`);
  setSlotState(slot, true);
  showStatus(`State ${slot} loaded`);
}

async function refreshStateSlots() {
  await Promise.all(
    [1, 2, 3].map(async (slot) => {
      try {
        setSlotState(slot, Boolean((await readState(slot))?.length));
      } catch {
        setSlotState(slot, false);
      }
    }),
  );
}

function startSaveLifecycle() {
  window.clearInterval(saveTimer);
  saveTimer = window.setInterval(() => persistSave().catch(console.warn), 10_000);
}

async function openGame(request, title = "GBA Emulator") {
  if (!homeToken) throw new Error("Open GBA from Home or Library.");
  await persistSave().catch(console.warn);
  if (engine && gameLoaded) engine.pauseGame();
  gameLoaded = false;
  setGameControlsEnabled(false);
  for (let slot = 1; slot <= 3; slot += 1) setSlotState(slot, false);
  showStatus("Starting game");

  const [{ bytes, fileName }, module] = await Promise.all([readGame(request), loadEngine()]);
  const romId = await sha256Hex(bytes);
  const romPath = `/data/games/${romId}.gba`;
  const saveName = `${romId}.sav`;
  const savePath = `/data/saves/${saveName}`;
  const saved = await restoreSave(saveName);
  if (saved?.length) module.FS.writeFile(savePath, saved);
  module.FS.writeFile(romPath, bytes);
  if (!module.loadGame(romPath, savePath)) throw new Error("The GBA engine rejected this game.");

  activeSaveName = saveName;
  activeRomId = romId;
  gameLoaded = true;
  paused = false;
  fastForward = false;
  soundEnabled = false;
  module.setVolume(0);
  module.setFastForwardMultiplier(1);
  module.resumeGame();
  emptyState.hidden = true;
  powerLed.classList.remove("off");
  setGameControlsEnabled(true);
  syncPauseButton();
  fastForwardButton.classList.remove("active");
  document.title = `${title || fileName} - GBA Emulator - ElastOS`;
  showStatus("");
  await refreshStateSlots();
  startSaveLifecycle();
  startInputLoop();
  canvas.focus({ preventScroll: true });
}

async function loadInstalledGames() {
  if (!homeToken) return;
  const response = await fetch(`/api/viewers/${VIEWER_ID}/library`, {
    headers: launchHeaders(),
  });
  if (!response.ok) return;
  const payload = await response.json();
  for (const item of Array.isArray(payload.items) ? payload.items : []) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "rom-library-item";
    button.textContent = item.title || item.capsule;
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      openGame({ capsule: item.capsule }, item.title || item.capsule).catch((error) => {
        showStatus(error.message || String(error), true);
      });
    });
    installedGames.append(button);
    installedGames.closest("#rom-library").hidden = false;
  }
}

function currentTouchMask() {
  let mask = 0;
  for (const button of touchPointers.values()) mask |= BUTTON_BITS[button] || 0;
  return mask;
}

function syncInput() {
  if (!engine || !gameLoaded) return;
  const nextMask = keyboardMask | currentTouchMask() | gamepadMask;
  for (const [button, bit] of Object.entries(BUTTON_BITS)) {
    const wasPressed = Boolean(appliedInputMask & bit);
    const isPressed = Boolean(nextMask & bit);
    if (wasPressed !== isPressed) {
      engine[isPressed ? "buttonPress" : "buttonUnpress"](button);
      document.querySelector(`[data-key="${button}"]`)?.classList.toggle("pressed", isPressed);
    }
  }
  appliedInputMask = nextMask;
}

function pollInput() {
  gamepadMask = readGamepadMask(navigator.getGamepads?.()?.find(Boolean));
  syncInput();
  inputFrame = requestAnimationFrame(pollInput);
}

function startInputLoop() {
  cancelAnimationFrame(inputFrame);
  inputFrame = requestAnimationFrame(pollInput);
}

function clearInput() {
  keyboardMask = 0;
  gamepadMask = 0;
  touchPointers.clear();
  syncInput();
}

function bindInput() {
  window.addEventListener("keydown", (event) => {
    const stateShortcut = {
      F1: [saveState, 1],
      F2: [saveState, 2],
      F3: [saveState, 3],
      F5: [loadState, 1],
      F6: [loadState, 2],
      F7: [loadState, 3],
    }[event.code];
    if (gameLoaded && stateShortcut) {
      event.preventDefault();
      stateShortcut[0](stateShortcut[1]).catch((error) => showStatus(error.message, true));
      return;
    }
    const button = KEY_BUTTONS[event.code];
    if (!button || event.repeat) return;
    event.preventDefault();
    keyboardMask |= BUTTON_BITS[button];
    syncInput();
    enableSound().catch(() => {});
  });
  window.addEventListener("keyup", (event) => {
    const button = KEY_BUTTONS[event.code];
    if (!button) return;
    event.preventDefault();
    keyboardMask &= ~BUTTON_BITS[button];
    syncInput();
  });
  window.addEventListener("blur", clearInput);
  document.querySelectorAll("[data-key]").forEach((element) => {
    const button = element.dataset.key;
    const release = (event) => {
      event.preventDefault();
      touchPointers.delete(event.pointerId);
      syncInput();
    };
    element.addEventListener("pointerdown", (event) => {
      event.preventDefault();
      element.setPointerCapture?.(event.pointerId);
      touchPointers.set(event.pointerId, button);
      syncInput();
      enableSound().catch(() => {});
    });
    element.addEventListener("pointerup", release);
    element.addEventListener("pointercancel", release);
    element.addEventListener("lostpointercapture", release);
  });
}

async function enableSound() {
  if (!engine || !gameLoaded || soundEnabled) return;
  engine.resumeAudio();
  engine.setVolume(Number(volume.value) / 100);
  soundEnabled = true;
}

function openLibrary() {
  if (window.parent && window.parent !== window) {
    window.parent.postMessage(
      { type: "home:open-target", target: "library", homeToken },
      window.location.origin,
    );
    return;
  }
  window.location.href = "/apps/home/";
}

pauseButton.addEventListener("click", () => {
  if (!engine || !gameLoaded) return;
  enableSound().catch(() => {});
  paused = !paused;
  engine[paused ? "pauseGame" : "resumeGame"]();
  syncPauseButton();
  if (paused) persistSave().catch((error) => showStatus(error.message, true));
});
fastForwardButton.addEventListener("click", () => {
  if (!engine || !gameLoaded) return;
  enableSound().catch(() => {});
  fastForward = !fastForward;
  engine.setFastForwardMultiplier(fastForward ? 4 : 1);
  fastForwardButton.classList.toggle("active", fastForward);
});
volume.addEventListener("input", () => {
  enableSound().catch(() => {});
  if (engine) engine.setVolume(Number(volume.value) / 100);
});
fullscreenButton.addEventListener("click", () => {
  const action = document.fullscreenElement
    ? document.exitFullscreen()
    : screenBezel.requestFullscreen();
  action?.catch(() => {});
});
document.addEventListener("fullscreenchange", () => {
  syncFullscreenButton();
  canvas.focus({ preventScroll: true });
});
emptyState.addEventListener("click", openLibrary);
emptyState.addEventListener("keydown", (event) => {
  if (event.key !== "Enter" && event.key !== " ") return;
  event.preventDefault();
  openLibrary();
});
for (let slot = 1; slot <= 3; slot += 1) {
  document.getElementById(`btn-save${slot}`).addEventListener("click", () => {
    saveState(slot).catch((error) => showStatus(error.message, true));
  });
  document.getElementById(`btn-load${slot}`).addEventListener("click", () => {
    loadState(slot).catch((error) => showStatus(error.message, true));
  });
}
document.addEventListener("visibilitychange", () => {
  if (document.hidden) persistSave().catch(console.warn);
});
window.addEventListener("pagehide", () => {
  window.clearInterval(saveTimer);
  cancelAnimationFrame(inputFrame);
  clearInput();
  persistSave(true).catch(() => {});
});

syncPauseButton();
syncFullscreenButton();
bindInput();
loadInstalledGames().catch(() => {});
const launch = requestedGame();
if (launch) {
  const title = query.get("name") || query.get("capsule") || fileNameFromUri(launch.objectUri);
  openGame(launch, title).catch((error) => showStatus(error.message || String(error), true));
}
