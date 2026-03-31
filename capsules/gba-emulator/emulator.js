import mGBA from './mgba.js';

const canvas = document.getElementById('canvas');
const dropZone = document.getElementById('drop-zone');
const fileInput = document.getElementById('file-input');
const status = document.getElementById('status');
const btnPause = document.getElementById('btn-pause');
const btnFF = document.getElementById('btn-ff');
const volumeSlider = document.getElementById('volume-slider');

let Module = null;
let paused = false;
let fastForward = false;

// ElastOS capsule state (populated by bootstrap)
let capsuleToken = null;
let writeCapabilityToken = null;
let readCapabilityToken = null;
let capsuleName = null;
let storagePath = null;
let romFilename = null;

function setStatus(msg, isError) {
  status.textContent = msg;
  status.className = isError ? 'error' : '';
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

async function storageList(path) {
  if (!readCapabilityToken) {
    throw new Error('localhost read capability was not granted for ' + path);
  }
  const resp = await fetch('/api/localhost/' + path + '?list=true', {
    headers: {
      'Authorization': 'Bearer ' + capsuleToken,
      'X-Capability-Token': readCapabilityToken,
    },
  });
  if (!resp.ok) return [];
  const data = await resp.json();
  return data.entries || [];
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

  try {
    const entries = Module.FS.readdir('/data/states/')
      .filter((name) => name !== '.' && name !== '..');
    const suffix = '.ss' + slot;
    const baseName = stateBaseName();
    const fallback = entries.find((name) => name === preferred)
      || entries.find((name) => baseName && name.startsWith(baseName) && name.endsWith(suffix))
      || entries.find((name) => name.endsWith(suffix));
    if (fallback) {
      const stateData = Module.FS.readFile(stateFsPath(fallback));
      if (stateData && stateData.length > 0) {
        return { stateFile: fallback, stateData };
      }
    }
  } catch (e) {
    lastError = lastError || e;
  }

  if (lastError) {
    throw lastError;
  }
  return null;
}

async function findStoredStateFile(slot) {
  const preferred = preferredStateFile(slot);
  if (!preferred) return null;

  const exact = await storageGet(storagePath + 'states/' + preferred);
  if (exact) {
    return preferred;
  }

  const entries = await storageList(storagePath + 'states/');
  const suffix = '.ss' + slot;
  const baseName = stateBaseName();
  const fallback = entries.find((entry) => entry.name === preferred)
    || entries.find((entry) => baseName && entry.name.startsWith(baseName) && entry.name.endsWith(suffix))
    || entries.find((entry) => entry.name.endsWith(suffix));
  return fallback ? fallback.name : null;
}

// --- Save/Load persistence ---

function saveName() {
  // Derive save filename from ROM: "Game.gba" -> "Game.sav"
  if (!romFilename) return null;
  return romFilename.replace(/\.[^.]+$/, '.sav');
}

async function syncSavesToStorage() {
  if (!storagePath) return;

  // Get in-game save data from mGBA via direct API
  try {
    const saveData = Module.getSave();
    if (saveData && saveData.length > 0) {
      const name = saveName();
      if (name) {
        await storagePut(storagePath + name, saveData);
        console.log('Save synced to storage:', name, saveData.length, 'bytes');
      }
    }
  } catch (e) {
    console.warn('Save sync failed:', e);
  }
}

async function restoreSavesFromStorage() {
  if (!storagePath) return;

  const name = saveName();
  if (!name) return;

  try {
    const resp = await storageGet(storagePath + name);
    if (resp) {
      const data = new Uint8Array(await resp.arrayBuffer());
      if (data.length > 0) {
        // mGBA reads .sav files from /data/saves/
        Module.FS.writeFile('/data/saves/' + name, data);
        console.log('Save restored from storage:', name, data.length, 'bytes');
      }
    }
  } catch (e) {
    console.warn('Save restore failed:', e);
  }
}

async function saveStateToStorage(slot) {
  if (!storagePath) return;
  try {
    const saved = await waitForLocalStateFile(slot);
    if (saved && saved.stateData.length > 0) {
      await storagePut(storagePath + 'states/' + saved.stateFile, saved.stateData);
      console.log('State', slot, 'synced to storage as', saved.stateFile, ':', saved.stateData.length, 'bytes');
      return true;
    }
  } catch (e) {
    console.warn('State', slot, 'sync failed:', e);
    throw e;
  }
  throw new Error('state file for slot ' + slot + ' was not created');
}

async function loadStateFromStorage(slot) {
  if (!storagePath) return false;
  try {
    const stateFile = await findStoredStateFile(slot);
    if (!stateFile) return false;
    const resp = await storageGet(storagePath + 'states/' + stateFile);
    if (resp) {
      const data = new Uint8Array(await resp.arrayBuffer());
      if (data.length > 0) {
        // mGBA reads save states from /data/states/
        Module.FS.writeFile(stateFsPath(stateFile), data);
        console.log('State', slot, 'restored from storage as', stateFile, ':', data.length, 'bytes');
        return true;
      }
    }
  } catch (e) {
    console.warn('State restore failed:', e);
  }
  return false;
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

// --- Emulator init ---

async function initEmulator() {
  setStatus('Initializing emulator...');
  try {
    Module = await mGBA({ canvas });
    await Module.FSInit();

    // Key bindings
    Module.bindKey('Up', 'up');
    Module.bindKey('Down', 'down');
    Module.bindKey('Left', 'left');
    Module.bindKey('Right', 'right');
    Module.bindKey('z', 'a');
    Module.bindKey('x', 'b');
    Module.bindKey('Return', 'start');
    Module.bindKey('Backspace', 'select');
    Module.bindKey('a', 'l');
    Module.bindKey('s', 'r');

    Module.setVolume(0.5);

    // Try bootstrap — if ROM info is available, auto-load it
    const info = await bootstrap();
    if (info && info.rom) {
      capsuleToken = info.token;
      capsuleName = info.name;
      romFilename = info.rom;

      // Derive storage path from manifest permissions
      if (info.storage && info.storage.length > 0) {
        // "localhost://Users/self/.AppData/LocalHost/GBA/gba-ucity/*"
        // -> "Users/self/.AppData/LocalHost/GBA/gba-ucity/"
        let s = info.storage[0];
        const prefix = 'localhost://';
        s = s.startsWith(prefix) ? s.slice(prefix.length) : s;
        // Strip wildcard suffix, keep trailing slash
        storagePath = s.replace(/\*$/, '');
      }

      setStatus('Loading ' + capsuleName + '...');

      // Request storage capabilities (shell auto-grants)
      if (storagePath) {
        writeCapabilityToken = await requestCapability(info.storage[0], 'write');
        readCapabilityToken = await requestCapability(info.storage[0], 'read');
        if (writeCapabilityToken) {
          console.log('Storage write capability granted');
          // Ensure states subdirectory exists
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
        if (readCapabilityToken) {
          console.log('Storage read capability granted');
        }
        if (!writeCapabilityToken || !readCapabilityToken) {
          console.warn('GBA state persistence unavailable: localhost storage capability was not granted');
        }
      }

      // Fetch ROM from /capsule-data/ with capsule name as cache key.
      // Different games use the same filename (rom.gba) on the same URL,
      // so we add the capsule name to differentiate them in the browser cache.
      const romUrl = '/capsule-data/' + encodeURIComponent(romFilename)
        + '?capsule=' + encodeURIComponent(capsuleName);
      const romResp = await fetch(romUrl);
      if (!romResp.ok) {
        setStatus('Failed to fetch ROM: ' + romResp.statusText, true);
        return;
      }
      const romData = new Uint8Array(await romResp.arrayBuffer());

      // Restore saves before loading game
      await restoreSavesFromStorage();

      // Write ROM to mGBA virtual FS and load
      const romPath = '/data/games/' + romFilename;
      Module.FS.writeFile(romPath, romData);
      if (Module.loadGame(romPath)) {
        dropZone.classList.add('hidden');
        enableControls(true);
        setStatus(capsuleName);
        Module.resumeAudio();

        // Try to restore auto-save state (slot 0) for instant resume on refresh
        const resumed = await loadStateFromStorage(0);
        if (resumed && Module.loadState(0)) {
          console.log('Resumed from auto-save state');
        }

        // Set up periodic save sync (every 30 seconds) and auto-save state
        if (writeCapabilityToken) {
          setInterval(() => {
            syncSavesToStorage();
            // Auto-save state to slot 0 for resume on refresh
            if (Module.saveState(0)) {
              saveStateToStorage(0).catch((err) => {
                console.warn('Auto-save state sync failed:', err);
              });
            }
          }, 30000);

          // Save state on visibility change (tab switch / close) and before unload
          const autoSave = () => {
            syncSavesToStorage();
            if (Module.saveState(0)) {
              saveStateToStorage(0).catch((err) => {
                console.warn('Visibility-change auto-save failed:', err);
              });
            }
          };
          document.addEventListener('visibilitychange', () => {
            if (document.hidden) autoSave();
          });
          window.addEventListener('beforeunload', autoSave);
        }
      } else {
        setStatus('Failed to load ROM.', true);
      }
    } else {
      // No bootstrap info — standalone mode, show drag-and-drop
      setStatus('Ready. Drop a ROM to play.');
    }
  } catch (e) {
    setStatus('Failed to initialize: ' + e.message, true);
    console.error(e);
  }
}

// Load a ROM file into the emulator (drag-and-drop mode)
function loadRom(file) {
  if (!Module) return;
  romFilename = file.name;
  setStatus('Loading ' + file.name + '...');
  Module.uploadRom(file, () => {
    const romPath = '/data/games/' + file.name;
    if (Module.loadGame(romPath)) {
      dropZone.classList.add('hidden');
      enableControls(true);
      setStatus(file.name);
      Module.resumeAudio();
    } else {
      setStatus('Failed to load ROM.', true);
    }
  });
}

function enableControls(enabled) {
  btnPause.disabled = !enabled;
  btnFF.disabled = !enabled;
  document.getElementById('btn-save1').disabled = !enabled;
  document.getElementById('btn-save2').disabled = !enabled;
  document.getElementById('btn-save3').disabled = !enabled;
  document.getElementById('btn-load1').disabled = !enabled;
  document.getElementById('btn-load2').disabled = !enabled;
  document.getElementById('btn-load3').disabled = !enabled;
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
    Module.resumeGame();
    btnPause.textContent = 'Pause';
    paused = false;
  } else {
    Module.pauseGame();
    btnPause.textContent = 'Resume';
    paused = true;
  }
});

// Fullscreen
const btnFullscreen = document.getElementById('btn-fullscreen');
const screenContainer = document.getElementById('screen-container');
btnFullscreen.addEventListener('click', () => {
  if (document.fullscreenElement) {
    document.exitFullscreen();
  } else {
    screenContainer.requestFullscreen();
  }
});
document.addEventListener('fullscreenchange', () => {
  btnFullscreen.textContent = document.fullscreenElement ? 'Exit FS' : 'FS';
});

// Fast forward
btnFF.addEventListener('click', () => {
  if (!Module) return;
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
  document.getElementById('btn-save' + slot).addEventListener('click', async () => {
    if (!Module) return;
    if (Module.saveState(slot)) {
      try {
        await saveStateToStorage(slot);
        setStatus('State saved to slot ' + slot);
      } catch (e) {
        setStatus('State sync failed for slot ' + slot + ': ' + e.message, true);
      }
    } else {
      setStatus('Save failed', true);
    }
  });
  document.getElementById('btn-load' + slot).addEventListener('click', async () => {
    if (!Module) return;
    try {
      // Try loading from storage first, then from local mGBA state
      await loadStateFromStorage(slot);
      if (Module.loadState(slot)) {
        setStatus('State loaded from slot ' + slot);
      } else {
        setStatus('No state in slot ' + slot, true);
      }
    } catch (e) {
      setStatus('State restore failed for slot ' + slot + ': ' + e.message, true);
    }
  });
}

// Keyboard shortcuts for save/load states
document.addEventListener('keydown', (e) => {
  if (!Module || dropZone.classList.contains('hidden') === false) return;

  if (e.key === 'F1') { e.preventDefault(); Module.saveState(1); saveStateToStorage(1).then(() => setStatus('State saved to slot 1')).catch((err) => setStatus('State sync failed for slot 1: ' + err.message, true)); }
  if (e.key === 'F2') { e.preventDefault(); Module.saveState(2); saveStateToStorage(2).then(() => setStatus('State saved to slot 2')).catch((err) => setStatus('State sync failed for slot 2: ' + err.message, true)); }
  if (e.key === 'F3') { e.preventDefault(); Module.saveState(3); saveStateToStorage(3).then(() => setStatus('State saved to slot 3')).catch((err) => setStatus('State sync failed for slot 3: ' + err.message, true)); }
  if (e.key === 'F5') { e.preventDefault(); loadStateFromStorage(1).then((found) => { if (found && Module.loadState(1)) { setStatus('State loaded from slot 1'); } else { setStatus('No state in slot 1', true); } }).catch((err) => setStatus('State restore failed for slot 1: ' + err.message, true)); }
  if (e.key === 'F6') { e.preventDefault(); loadStateFromStorage(2).then((found) => { if (found && Module.loadState(2)) { setStatus('State loaded from slot 2'); } else { setStatus('No state in slot 2', true); } }).catch((err) => setStatus('State restore failed for slot 2: ' + err.message, true)); }
  if (e.key === 'F7') { e.preventDefault(); loadStateFromStorage(3).then((found) => { if (found && Module.loadState(3)) { setStatus('State loaded from slot 3'); } else { setStatus('No state in slot 3', true); } }).catch((err) => setStatus('State restore failed for slot 3: ' + err.message, true)); }
});

// Start
initEmulator();

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

  // Mouse/touch handlers for each button
  Object.entries(buttonMap).forEach(([btnId, inputName]) => {
    const btn = document.getElementById(btnId);
    if (!btn) return;

    // Prevent context menu on long press
    btn.addEventListener('contextmenu', (e) => e.preventDefault());

    // Mouse events
    btn.addEventListener('mousedown', (e) => {
      e.preventDefault();
      if (Module) Module.buttonPress(inputName);
      btn.classList.add('pressed');
    });

    btn.addEventListener('mouseup', (e) => {
      e.preventDefault();
      if (Module) Module.buttonUnpress(inputName);
      btn.classList.remove('pressed');
    });

    btn.addEventListener('mouseleave', () => {
      if (Module) Module.buttonUnpress(inputName);
      btn.classList.remove('pressed');
    });

    // Touch events
    btn.addEventListener('touchstart', (e) => {
      e.preventDefault();
      if (Module) Module.buttonPress(inputName);
      btn.classList.add('pressed');
    }, { passive: false });

    btn.addEventListener('touchend', (e) => {
      e.preventDefault();
      if (Module) Module.buttonUnpress(inputName);
      btn.classList.remove('pressed');
    }, { passive: false });

    btn.addEventListener('touchcancel', () => {
      if (Module) Module.buttonUnpress(inputName);
      btn.classList.remove('pressed');
    });
  });
}

// Initialize shell buttons after DOM is ready
if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', setupShellButtons);
} else {
  setupShellButtons();
}
