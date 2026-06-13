import mGBA from './mgba.js';

const canvas = document.getElementById('canvas');
const dropZone = document.getElementById('drop-zone');
const dropZoneCopy = document.getElementById('drop-zone-copy');
const fileInput = document.getElementById('file-input');
const romLibrary = document.getElementById('rom-library');
const romLibraryList = document.getElementById('rom-library-list');
const status = document.getElementById('status');
const btnPause = document.getElementById('btn-pause');
const btnFF = document.getElementById('btn-ff');
const volumeSlider = document.getElementById('volume-slider');
const emulatorCard = document.getElementById('emulator-card');
const utilityPanel = document.getElementById('utility-panel');
const utilityToggle = document.getElementById('utility-toggle');
const focusTarget = document.getElementById('screen-container');
const fullscreenTarget = document.getElementById('screen-bezel') || focusTarget;
const homeToken = currentHomeToken();
const compactUtilityQuery = window.matchMedia
  ? window.matchMedia('(max-width: 780px), (pointer: coarse) and (max-height: 760px)')
  : null;
const EMULATOR_INIT_TIMEOUT_MS = 10_000;
const VIEWER_ID = 'gba-emulator';
const UNSUPPORTED_EMULATOR_RUNTIME_MESSAGE =
  'This device cannot run the current GBA engine. It needs WebAssembly threads, which this browser or WebView does not expose.';
const INSERT_GAME_COPY = 'Insert Game';

let Module = null;
let paused = false;
let fastForward = false;
let currentLibrary = [];
let persistenceAutosaveBound = false;
const activeInputs = new Set();
const activeInputPointers = new Map();
let utilityPreferenceTouched = false;

// ElastOS capsule state (populated by bootstrap)
let capsuleToken = null;
let writeCapabilityToken = null;
let readCapabilityToken = null;
let capsuleName = null;
let storagePath = null;
let romFilename = null;
let activeRomCapsule = null;
let emulatorControlsEnabled = false;
let currentDisplayTitle = INSERT_GAME_COPY;

function assertEmulatorRuntimeSupported() {
  if (typeof WebAssembly !== 'object') {
    throw new Error('This browser does not support WebAssembly.');
  }
  if (typeof Worker !== 'function') {
    throw new Error('This browser cannot run the emulator. Web workers are unavailable.');
  }
  if (typeof SharedArrayBuffer !== 'function' || window.crossOriginIsolated !== true) {
    throw new Error(UNSUPPORTED_EMULATOR_RUNTIME_MESSAGE);
  }
  try {
    new WebAssembly.Memory({ initial: 1, maximum: 1, shared: true });
  } catch (_error) {
    throw new Error(UNSUPPORTED_EMULATOR_RUNTIME_MESSAGE);
  }
}

function withTimeout(promise, timeoutMs, timeoutMessage) {
  let timeoutId = null;
  const timeout = new Promise((_resolve, reject) => {
    timeoutId = window.setTimeout(() => reject(new Error(timeoutMessage)), timeoutMs);
  });
  return Promise.race([promise, timeout]).finally(() => {
    if (timeoutId !== null) {
      window.clearTimeout(timeoutId);
    }
  });
}

function formatInitError(error) {
  const message = error instanceof Error ? error.message : String(error || 'Unknown emulator error.');
  if (message.startsWith('This browser') || message.startsWith('This WebView') || message.startsWith('This device')) {
    return message;
  }
  return 'Failed to initialize emulator: ' + message;
}

function setStatus(msg, isError) {
  status.textContent = msg;
  status.classList.toggle('error', Boolean(isError));
  syncStatusVisibility();
}

function setScreenTitle(title) {
  currentDisplayTitle = title || INSERT_GAME_COPY;
  document.title = `${currentDisplayTitle} - GBA Emulator - ElastOS`;
  syncStatusVisibility();
}

function syncStatusVisibility() {
  if (!status) {
    return;
  }
  const text = status.textContent?.trim() || '';
  const title = currentDisplayTitle.trim();
  const duplicateTitle = !status.classList.contains('error') && text && title && text === title;
  status.hidden = !text || duplicateTitle;
}

function currentCapsuleFromQuery() {
  try {
    const params = new URL(window.location.href).searchParams;
    return params.get('capsule');
  } catch (_error) {
    return null;
  }
}

function currentHomeToken() {
  try {
    const params = new URL(window.location.href).searchParams;
    return params.get('home_token');
  } catch (_error) {
    return null;
  }
}

function homeLaunchHeaders() {
  return homeToken ? { 'x-elastos-home-token': homeToken } : {};
}

function setActiveRomContext({ capsule = null, displayName = null, fileName = null } = {}) {
  activeRomCapsule = capsule;
  capsuleName = displayName;
  romFilename = fileName;
  setScreenTitle(displayName || INSERT_GAME_COPY);
}

function clearRuntimeStorageContext() {
  capsuleToken = null;
  writeCapabilityToken = null;
  readCapabilityToken = null;
  storagePath = null;
}

const buttonIdsByInput = {
  up: 'btn-dpad-up',
  down: 'btn-dpad-down',
  left: 'btn-dpad-left',
  right: 'btn-dpad-right',
  a: 'btn-a',
  b: 'btn-b',
  start: 'btn-start',
  select: 'btn-select',
  l: 'btn-l',
  r: 'btn-r',
};

const keyboardInputByKey = {
  ArrowUp: 'up',
  ArrowDown: 'down',
  ArrowLeft: 'left',
  ArrowRight: 'right',
  KeyX: 'a',
  KeyZ: 'b',
  Enter: 'start',
  Backspace: 'select',
  KeyA: 'l',
  KeyS: 'r',
};

function setDropZoneCopy(text) {
  if (dropZoneCopy) {
    dropZoneCopy.textContent = text;
  }
}

function slotStatusElement(slot) {
  return document.getElementById('slot-status' + slot);
}

function setSlotState(slot, state) {
  const element = slotStatusElement(slot);
  if (!element) {
    return;
  }
  element.dataset.slotState = state;
  element.textContent = state === 'saved' ? 'Saved' : 'Empty';
  const loadButton = document.getElementById('btn-load' + slot);
  if (loadButton) {
    loadButton.disabled = !emulatorControlsEnabled || state !== 'saved';
  }
}

function clearSlotStates() {
  for (let slot = 1; slot <= 3; slot += 1) {
    setSlotState(slot, 'empty');
  }
}

function focusGameSurface() {
  if (focusTarget instanceof HTMLElement) {
    focusTarget.tabIndex = -1;
    focusTarget.focus({ preventScroll: true });
  }
  window.focus();
}

function setIconButtonState(button, glyph, label) {
  if (!button) {
    return;
  }
  const icon = button.querySelector('.icon-glyph');
  if (icon) {
    icon.textContent = glyph;
  }
  button.setAttribute('aria-label', label);
  button.title = label;
}

function syncPauseButton() {
  setIconButtonState(btnPause, paused ? '▶' : '❚❚', paused ? 'Resume' : 'Pause');
}

function syncFullscreenButton() {
  const active = Boolean(document.fullscreenElement);
  btnFullscreen.classList.toggle('active', active);
  setIconButtonState(btnFullscreen, active ? '⤡' : '⤢', active ? 'Exit fullscreen' : 'Fullscreen');
}

function setUtilitySidebarCollapsed(collapsed) {
  if (!emulatorCard || !utilityPanel || !utilityToggle) {
    return;
  }
  emulatorCard.classList.toggle('sidebar-collapsed', collapsed);
  utilityPanel.hidden = collapsed;
  utilityToggle.setAttribute('aria-expanded', String(!collapsed));
  utilityToggle.setAttribute('aria-label', collapsed ? 'Show sidebar' : 'Collapse sidebar');
  utilityToggle.title = collapsed ? 'Show sidebar' : 'Collapse sidebar';
  const label = utilityToggle.querySelector('.utility-toggle-label');
  if (label) {
    label.textContent = collapsed ? 'Show' : 'Hide';
  }
}

function shouldCollapseUtilityByDefault() {
  return Boolean(compactUtilityQuery?.matches);
}

function syncUtilityDefaultForViewport() {
  if (!utilityPreferenceTouched) {
    setUtilitySidebarCollapsed(shouldCollapseUtilityByDefault());
  }
}

function resumeRuntime() {
  if (!Module) {
    return;
  }
  try {
    Module.resumeAudio();
  } catch (_error) {
    // Best-effort; browsers may still gate audio until explicit interaction.
  }
  try {
    Module.resumeGame();
  } catch (_error) {
    // Best-effort; if the core is already running this is a no-op.
  }
}

function setControlPressed(inputName, pressed) {
  const buttonId = buttonIdsByInput[inputName];
  if (!buttonId) {
    return;
  }
  const button = document.getElementById(buttonId);
  if (button) {
    button.classList.toggle('pressed', pressed);
  }
}

function pressInput(inputName) {
  if (!Module || activeInputs.has(inputName)) {
    return;
  }
  Module.buttonPress(inputName);
  activeInputs.add(inputName);
  setControlPressed(inputName, true);
}

function releaseInput(inputName) {
  if (!Module || !activeInputs.has(inputName)) {
    return;
  }
  Module.buttonUnpress(inputName);
  activeInputs.delete(inputName);
  setControlPressed(inputName, false);
}

function releaseAllInputs() {
  for (const inputName of [...activeInputs]) {
    releaseInput(inputName);
  }
}

function keyboardInputBlocked(event) {
  const target = event.target;
  return target instanceof HTMLElement
    && target.matches('input[type="range"], input[type="file"]');
}

async function slotHasSavedState(slot) {
  const stateFile = preferredStateFile(slot);
  if (!stateFile) {
    return false;
  }
  try {
    const stateData = Module.FS.readFile(stateFsPath(stateFile));
    if (stateData && stateData.length > 0) {
      return true;
    }
  } catch (_error) {
    // Ignore local-miss; persistence check below is the source of truth for reopened sessions.
  }
  try {
    const response = await readPersistenceFile('state', stateFile);
    if (!response) {
      return false;
    }
    const data = new Uint8Array(await response.arrayBuffer());
    return data.length > 0;
  } catch (_error) {
    return false;
  }
}

async function refreshSlotStates() {
  for (let slot = 1; slot <= 3; slot += 1) {
    setSlotState(slot, (await slotHasSavedState(slot)) ? 'saved' : 'empty');
  }
}

function showRomLibrary(items) {
  currentLibrary = Array.isArray(items) ? items : [];
  if (!romLibrary || !romLibraryList) {
    return;
  }
  romLibraryList.replaceChildren();
  for (const item of currentLibrary) {
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'rom-library-item';
    button.dataset.capsule = item.capsule;
    button.innerHTML = `
      <strong>${escapeHtml(item.title)}</strong>
      <span>${escapeHtml(item.description)}</span>
    `;
    button.addEventListener('click', (event) => {
      event.stopPropagation();
      loadLibraryCapsule(item.capsule).catch((error) => {
        setStatus('Failed to load ROM: ' + error.message, true);
      });
    });
    romLibraryList.appendChild(button);
  }
  romLibrary.hidden = currentLibrary.length === 0;
}

function escapeHtml(value) {
  return String(value || '')
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;');
}

// --- ElastOS Storage API helpers ---

async function requestCapability(resource, action) {
  // Request a capability token from the runtime
  const resp = await fetch('/api/capability/request', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': 'Bearer ' + capsuleToken,
    },
    body: JSON.stringify({ resource, action }),
  });
  if (!resp.ok) return null;
  const data = await resp.json();
  // Auto-granted immediately?
  if (data.status === 'granted' && data.token) return data.token;
  const requestId = data.request_id;
  if (!requestId) return null;

  // Poll for grant (shell auto-grants)
  for (let i = 0; i < 30; i++) {
    await new Promise(r => setTimeout(r, 200));
    const poll = await fetch('/api/capability/request/' + requestId, {
      headers: { 'Authorization': 'Bearer ' + capsuleToken },
    });
    if (!poll.ok) continue;
    const status = await poll.json();
    if (status.status === 'granted') return status.token;
    if (status.status === 'denied') return null;
  }
  return null;
}

async function storageGet(path) {
  if (!readCapabilityToken) {
    throw new Error('localhost read capability was not granted for ' + path);
  }
  const resp = await fetch('/api/localhost/' + path, {
    headers: {
      'Authorization': 'Bearer ' + capsuleToken,
      'X-Capability-Token': readCapabilityToken,
    },
  });
  if (resp.status === 404) return null;
  if (!resp.ok) {
    const detail = await resp.text().catch(() => '');
    throw new Error('storage read failed for ' + path + ': ' + resp.status + ' ' + detail);
  }
  return resp;
}

async function storagePut(path, data) {
  if (!writeCapabilityToken) {
    throw new Error('localhost write capability was not granted for ' + path);
  }
  const resp = await fetch('/api/localhost/' + path, {
    method: 'PUT',
    headers: {
      'Authorization': 'Bearer ' + capsuleToken,
      'X-Capability-Token': writeCapabilityToken,
    },
    body: data,
  });
  if (!resp.ok) {
    const detail = await resp.text().catch(() => '');
    throw new Error('storage write failed for ' + path + ': ' + resp.status + ' ' + detail);
  }
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function stateBaseName() {
  return romFilename ? romFilename.replace(/\.[^.]+$/, '') : null;
}

function preferredStateFile(slot) {
  const baseName = stateBaseName();
  return baseName ? baseName + '.ss' + slot : null;
}

function stateFsPath(stateFile) {
  return '/data/states/' + stateFile;
}

async function waitForLocalStateFile(slot, timeoutMs = 1500) {
  const preferred = preferredStateFile(slot);
  if (!preferred) return null;

  const deadline = Date.now() + timeoutMs;
  let lastError = null;

  while (Date.now() < deadline) {
    try {
      const stateData = Module.FS.readFile(stateFsPath(preferred));
      if (stateData && stateData.length > 0) {
        return { stateFile: preferred, stateData };
      }
    } catch (e) {
      lastError = e;
    }
    await sleep(50);
  }

  if (lastError) {
    throw lastError;
  }
  return null;
}

// --- Save/Load persistence ---

function saveName() {
  // Derive save filename from ROM: "Game.gba" -> "Game.sav"
  if (!romFilename) return null;
  return romFilename.replace(/\.[^.]+$/, '.sav');
}

async function readPersistenceFile(scope, fileName) {
  if (!fileName) return null;
  if (activeRomCapsule && homeToken) {
    const resp = await fetch(
      '/api/viewers/' + encodeURIComponent(VIEWER_ID) + '/storage/'
        + encodeURIComponent(activeRomCapsule)
        + '/'
        + encodeURIComponent(scope)
        + '/'
        + encodeURIComponent(fileName),
      {
        headers: {
          ...homeLaunchHeaders(),
        },
      },
    );
    if (resp.status === 404) return null;
    if (!resp.ok) {
      const detail = await resp.text().catch(() => '');
      throw new Error('persistence read failed for ' + fileName + ': ' + resp.status + ' ' + detail);
    }
    return resp;
  }
  if (!storagePath) return null;
  const relative = scope === 'state' ? 'states/' + fileName : fileName;
  return storageGet(storagePath + relative);
}

async function writePersistenceFile(scope, fileName, data) {
  if (!fileName) return;
  if (activeRomCapsule && homeToken) {
    const resp = await fetch(
      '/api/viewers/' + encodeURIComponent(VIEWER_ID) + '/storage/'
        + encodeURIComponent(activeRomCapsule)
        + '/'
        + encodeURIComponent(scope)
        + '/'
        + encodeURIComponent(fileName),
      {
        method: 'PUT',
        headers: {
          ...homeLaunchHeaders(),
        },
        body: data,
      },
    );
    if (!resp.ok) {
      const detail = await resp.text().catch(() => '');
      throw new Error('persistence write failed for ' + fileName + ': ' + resp.status + ' ' + detail);
    }
    return;
  }
  if (!storagePath) return;
  const relative = scope === 'state' ? 'states/' + fileName : fileName;
  await storagePut(storagePath + relative, data);
}

async function syncSavesToPersistence() {
  if (!activeRomCapsule && !storagePath) return;

  // Get in-game save data from mGBA via direct API
  try {
    const saveData = Module.getSave();
    if (saveData && saveData.length > 0) {
      const name = saveName();
      if (name) {
        await writePersistenceFile('save', name, saveData);
      }
    }
  } catch (e) {
    console.warn('Save sync failed:', e);
  }
}

async function restoreSavesFromPersistence() {
  const name = saveName();
  if (!name) return;

  try {
    const resp = await readPersistenceFile('save', name);
    if (resp) {
      const data = new Uint8Array(await resp.arrayBuffer());
      if (data.length > 0) {
        // mGBA reads .sav files from /data/saves/
        Module.FS.writeFile('/data/saves/' + name, data);
      }
    }
  } catch (e) {
    console.warn('Save restore failed:', e);
  }
}

async function saveStateToPersistence(slot) {
  try {
    const saved = await waitForLocalStateFile(slot);
    if (saved && saved.stateData.length > 0) {
      await writePersistenceFile('state', saved.stateFile, saved.stateData);
      return true;
    }
  } catch (e) {
    console.warn('State', slot, 'sync failed:', e);
    throw e;
  }
  throw new Error('state file for slot ' + slot + ' was not created');
}

async function loadStateFromPersistence(slot) {
  const stateFile = preferredStateFile(slot);
  if (!stateFile) return false;
  try {
    const resp = await readPersistenceFile('state', stateFile);
    if (resp) {
      const data = new Uint8Array(await resp.arrayBuffer());
      if (data.length > 0) {
        // mGBA reads save states from /data/states/
        Module.FS.writeFile(stateFsPath(stateFile), data);
        return true;
      }
    }
  } catch (e) {
    console.warn('State restore failed:', e);
  }
  return false;
}

function canPersistSession() {
  return Boolean(writeCapabilityToken || (activeRomCapsule && homeToken));
}

function ensurePersistenceAutosave() {
  if (persistenceAutosaveBound || !canPersistSession()) {
    return;
  }

  const autoSave = () => {
    syncSavesToPersistence();
    if (Module.saveState(0)) {
      saveStateToPersistence(0).catch((err) => {
        console.warn('Auto-save state sync failed:', err);
      });
    }
  };

  setInterval(autoSave, 30000);
  document.addEventListener('visibilitychange', () => {
    if (document.hidden) {
      autoSave();
    }
  });
  window.addEventListener('beforeunload', autoSave);
  persistenceAutosaveBound = true;
}

// --- Bootstrap ---

async function bootstrap() {
  try {
    const resp = await fetch('/api/capsule/bootstrap');
    if (!resp.ok) return null;
    return await resp.json();
  } catch (_) {
    return null;
  }
}

async function fetchRomLibrary() {
  const response = await fetch('/api/viewers/' + encodeURIComponent(VIEWER_ID) + '/library', {
    headers: {
      ...homeLaunchHeaders(),
    },
  });
  if (!response.ok) {
    throw new Error('library unavailable: ' + response.status);
  }
  const payload = await response.json();
  return Array.isArray(payload.items) ? payload.items : [];
}

async function loadBootstrappedCapsule(info) {
  capsuleToken = info.token;
  clearSlotStates();
  setActiveRomContext({
    capsule: currentCapsuleFromQuery(),
    displayName: info.name,
    fileName: info.rom,
  });

  if (info.storage && info.storage.length > 0) {
    let s = info.storage[0];
    const prefix = 'localhost://';
    s = s.startsWith(prefix) ? s.slice(prefix.length) : s;
    storagePath = s.replace(/\*$/, '');
  }

  setStatus('Loading ' + capsuleName + '...');

  if (storagePath) {
    writeCapabilityToken = await requestCapability(info.storage[0], 'write');
    readCapabilityToken = await requestCapability(info.storage[0], 'read');
    if (writeCapabilityToken) {
      const mkdirResp = await fetch('/api/localhost/' + storagePath + 'states/?mkdir=true', {
        method: 'POST',
        headers: {
          'Authorization': 'Bearer ' + capsuleToken,
          'X-Capability-Token': writeCapabilityToken,
        },
      });
      if (!mkdirResp.ok) {
        const detail = await mkdirResp.text().catch(() => '');
        throw new Error('failed to initialize state storage: ' + mkdirResp.status + ' ' + detail);
      }
    }
    if (!writeCapabilityToken || !readCapabilityToken) {
      console.warn('GBA state persistence unavailable: localhost storage capability was not granted');
    }
  }

  const romUrl = '/capsule-data/' + encodeURIComponent(romFilename)
    + '?capsule=' + encodeURIComponent(capsuleName);
  const romResp = await fetch(romUrl);
  if (!romResp.ok) {
    throw new Error('failed to fetch ROM: ' + romResp.statusText);
  }
  const romData = new Uint8Array(await romResp.arrayBuffer());
  await loadPersistentRomSession(romFilename, romData, capsuleName);
}

async function loadLibraryCapsule(capsule) {
  let item = currentLibrary.find((entry) => entry.capsule === capsule);
  if (!item) {
    currentLibrary = await fetchRomLibrary();
    item = currentLibrary.find((entry) => entry.capsule === capsule);
  }
  if (!item) {
    throw new Error('ROM capsule not found: ' + capsule);
  }

  setStatus('Loading ' + item.title + '...');
  clearSlotStates();
  const response = await fetch(
    '/api/viewers/' + encodeURIComponent(VIEWER_ID) + '/content/' + encodeURIComponent(capsule),
    {
      headers: {
        ...homeLaunchHeaders(),
      },
    },
  );
  if (!response.ok) {
    throw new Error('ROM content unavailable: ' + response.status);
  }
  const romData = new Uint8Array(await response.arrayBuffer());
  clearRuntimeStorageContext();
  setActiveRomContext({
    capsule: item.capsule,
    displayName: item.title,
    fileName: item.entrypoint || (capsule + '.gba'),
  });
  await loadPersistentRomSession(romFilename, romData, item.title);
}

async function showLibraryOrDropZone() {
  setActiveRomContext();
  clearSlotStates();
  const library = await fetchRomLibrary().catch(() => []);
  showRomLibrary(library);
  setDropZoneCopy(INSERT_GAME_COPY);
  setStatus('');
}

async function loadRomBytes(fileName, romData, displayName) {
  const romPath = '/data/games/' + fileName;
  Module.FS.writeFile(romPath, romData);
  if (!Module.loadGame(romPath)) {
    throw new Error('Failed to load ROM.');
  }
  dropZone.classList.add('hidden');
  enableControls(true);
  resumeRuntime();
  setStatus(displayName);
  focusGameSurface();
}

async function loadPersistentRomSession(fileName, romData, displayName) {
  await restoreSavesFromPersistence();
  await loadRomBytes(fileName, romData, displayName);

  const resumed = await loadStateFromPersistence(0);
  if (resumed && Module.loadState(0)) {
    syncStatusVisibility();
  }

  await refreshSlotStates();
  ensurePersistenceAutosave();
}

// --- Emulator init ---

async function initEmulator() {
  setStatus('Initializing emulator...');
  try {
    assertEmulatorRuntimeSupported();
    Module = await withTimeout(
      mGBA({ canvas }),
      EMULATOR_INIT_TIMEOUT_MS,
      UNSUPPORTED_EMULATOR_RUNTIME_MESSAGE,
    );
    await Module.FSInit();

    Module.setVolume(0.5);

    const info = await bootstrap();
    if (info && info.rom) {
      await loadBootstrappedCapsule(info);
    } else {
      const requestedCapsule = currentCapsuleFromQuery();
      if (requestedCapsule) {
        await loadLibraryCapsule(requestedCapsule);
      } else {
        await showLibraryOrDropZone();
      }
    }
  } catch (e) {
    setStatus(formatInitError(e), true);
    console.error(e);
  }
}

// Load a ROM file into the emulator (drag-and-drop mode)
function loadRom(file) {
  if (!Module) return;
  clearSlotStates();
  setActiveRomContext({
    capsule: null,
    displayName: file.name,
    fileName: file.name,
  });
  clearRuntimeStorageContext();
  setStatus('Loading ' + file.name + '...');
  Module.uploadRom(file, () => {
    const romPath = '/data/games/' + file.name;
    if (Module.loadGame(romPath)) {
      dropZone.classList.add('hidden');
      enableControls(true);
      resumeRuntime();
      setStatus(file.name);
      focusGameSurface();
    } else {
      setStatus('Failed to load ROM.', true);
    }
  });
}

function enableControls(enabled) {
  emulatorControlsEnabled = enabled;
  btnPause.disabled = !enabled;
  btnFF.disabled = !enabled;
  document.getElementById('btn-save1').disabled = !enabled;
  document.getElementById('btn-save2').disabled = !enabled;
  document.getElementById('btn-save3').disabled = !enabled;
  for (let slot = 1; slot <= 3; slot += 1) {
    const loadButton = document.getElementById('btn-load' + slot);
    if (!loadButton) {
      continue;
    }
    const state = slotStatusElement(slot)?.dataset.slotState || 'empty';
    loadButton.disabled = !enabled || state !== 'saved';
  }
}

async function saveSlot(slot) {
  if (!Module) return false;
  if (!Module.saveState(slot)) {
    setStatus('Save failed', true);
    return false;
  }
  try {
    await saveStateToPersistence(slot);
    setSlotState(slot, 'saved');
    setStatus('State saved to slot ' + slot);
    return true;
  } catch (e) {
    setStatus('State sync failed for slot ' + slot + ': ' + e.message, true);
    return false;
  }
}

async function loadSlot(slot) {
  if (!Module) return false;
  try {
    await loadStateFromPersistence(slot);
    if (Module.loadState(slot)) {
      setSlotState(slot, 'saved');
      setStatus('State loaded from slot ' + slot);
      return true;
    }
    setSlotState(slot, 'empty');
    setStatus('No state in slot ' + slot, true);
  } catch (e) {
    setStatus('State restore failed for slot ' + slot + ': ' + e.message, true);
  }
  return false;
}

// Drop zone events
dropZone.addEventListener('click', () => fileInput.click());

dropZone.addEventListener('dragover', (e) => {
  e.preventDefault();
  dropZone.classList.add('drag-over');
});

dropZone.addEventListener('dragleave', () => {
  dropZone.classList.remove('drag-over');
});

dropZone.addEventListener('drop', (e) => {
  e.preventDefault();
  dropZone.classList.remove('drag-over');
  const file = e.dataTransfer.files[0];
  if (file) loadRom(file);
});

fileInput.addEventListener('change', (e) => {
  const file = e.target.files[0];
  if (file) loadRom(file);
});

// Pause / resume
btnPause.addEventListener('click', () => {
  if (!Module) return;
  if (paused) {
    resumeRuntime();
    Module.resumeGame();
    paused = false;
  } else {
    Module.pauseGame();
    paused = true;
  }
  syncPauseButton();
});

// Fullscreen
const btnFullscreen = document.getElementById('btn-fullscreen');
btnFullscreen.addEventListener('click', async () => {
  try {
    if (document.fullscreenElement) {
      await document.exitFullscreen();
    } else {
      await fullscreenTarget.requestFullscreen();
    }
  } catch (error) {
    setStatus('Fullscreen failed: ' + error.message, true);
  }
});
document.addEventListener('fullscreenchange', () => {
  syncFullscreenButton();
  focusGameSurface();
});

if (utilityToggle && emulatorCard && utilityPanel) {
  utilityToggle.addEventListener('click', () => {
    utilityPreferenceTouched = true;
    setUtilitySidebarCollapsed(!emulatorCard.classList.contains('sidebar-collapsed'));
  });
  syncUtilityDefaultForViewport();
  compactUtilityQuery?.addEventListener('change', syncUtilityDefaultForViewport);
  window.addEventListener('resize', syncUtilityDefaultForViewport);
}

// Fast forward
btnFF.addEventListener('click', () => {
  if (!Module) return;
  resumeRuntime();
  fastForward = !fastForward;
  Module.setFastForwardMultiplier(fastForward ? 4 : 1);
  btnFF.classList.toggle('active', fastForward);
});

// Volume
volumeSlider.addEventListener('input', () => {
  if (!Module) return;
  Module.setVolume(volumeSlider.value / 100);
});

// Save / load states (with storage sync)
for (let slot = 1; slot <= 3; slot++) {
  document.getElementById('btn-save' + slot).addEventListener('click', () => {
    saveSlot(slot);
  });
  document.getElementById('btn-load' + slot).addEventListener('click', () => {
    loadSlot(slot);
  });
}

// Keyboard shortcuts for save/load states
document.addEventListener('keydown', (event) => {
  if (!Module || !dropZone.classList.contains('hidden') || keyboardInputBlocked(event)) {
    return;
  }
  resumeRuntime();
  if (event.code === 'F1') { event.preventDefault(); saveSlot(1); return; }
  if (event.code === 'F2') { event.preventDefault(); saveSlot(2); return; }
  if (event.code === 'F3') { event.preventDefault(); saveSlot(3); return; }
  if (event.code === 'F5') { event.preventDefault(); loadSlot(1); return; }
  if (event.code === 'F6') { event.preventDefault(); loadSlot(2); return; }
  if (event.code === 'F7') { event.preventDefault(); loadSlot(3); return; }

  const inputName = keyboardInputByKey[event.code] || keyboardInputByKey[event.key];
  if (!inputName) {
    return;
  }
  event.preventDefault();
  pressInput(inputName);
});

document.addEventListener('keyup', (event) => {
  const inputName = keyboardInputByKey[event.code] || keyboardInputByKey[event.key];
  if (!inputName) {
    return;
  }
  event.preventDefault();
  releaseInput(inputName);
});

window.addEventListener('blur', releaseAllInputs);
document.addEventListener('visibilitychange', () => {
  if (document.hidden) {
    releaseAllInputs();
  }
});

// Start
initEmulator();
syncPauseButton();
syncFullscreenButton();

// === GBA Shell Button Handlers ===
// Map shell buttons to emulator inputs

function setupShellButtons() {
  const buttonMap = {
    'btn-dpad-up': 'up',
    'btn-dpad-down': 'down',
    'btn-dpad-left': 'left',
    'btn-dpad-right': 'right',
    'btn-a': 'a',
    'btn-b': 'b',
    'btn-start': 'start',
    'btn-select': 'select',
    'btn-l': 'l',
    'btn-r': 'r',
  };

  function releasePointerInput(pointerId) {
    const inputName = activeInputPointers.get(pointerId);
    if (!inputName) {
      return;
    }
    activeInputPointers.delete(pointerId);
    releaseInput(inputName);
  }

  Object.entries(buttonMap).forEach(([btnId, inputName]) => {
    const btn = document.getElementById(btnId);
    if (!btn) return;

    // Prevent context menu on long press
    btn.addEventListener('contextmenu', (e) => e.preventDefault());

    btn.addEventListener('pointerdown', (e) => {
      if (e.pointerType === 'mouse' && e.button !== 0) {
        return;
      }
      e.preventDefault();
      if (!activeInputPointers.has(e.pointerId)) {
        activeInputPointers.set(e.pointerId, inputName);
      }
      try {
        btn.setPointerCapture(e.pointerId);
      } catch (_error) {
        // Pointer capture may be unavailable in some embedded browsers.
      }
      focusGameSurface();
      resumeRuntime();
      pressInput(inputName);
    });

    btn.addEventListener('pointerup', (e) => {
      e.preventDefault();
      releasePointerInput(e.pointerId);
    });

    btn.addEventListener('pointercancel', (e) => {
      releasePointerInput(e.pointerId);
    });

    btn.addEventListener('lostpointercapture', (e) => {
      releasePointerInput(e.pointerId);
    });
  });
}

// Initialize shell buttons after DOM is ready
if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', setupShellButtons);
} else {
  setupShellButtons();
}
