/* Optional UI sounds — three only, off by default (macOS restraint). Pure
 * Web Audio oscillators; no network, no asset fetch. Persisted under
 * elastos.ui.sounds. Triggered from shell toast / trash empty / refusal paths.
 */

const KEY = "elastos.ui.sounds";
let context = null;
// Opaque-sandboxed GUI frame: localStorage throws, so the toggle keeps its
// value in memory and the Home host persists it (home:ui-preference).
let memoryState = "";

function enabled() {
  if (memoryState) {
    return memoryState === "on";
  }
  try {
    return localStorage.getItem(KEY) === "on";
  } catch (_error) {
    return false;
  }
}

function audioContext() {
  if (!context) {
    const Ctor = window.AudioContext || window.webkitAudioContext;
    if (!Ctor) {
      return null;
    }
    context = new Ctor();
  }
  if (context.state === "suspended") {
    context.resume().catch(() => {});
  }
  return context;
}

function tone({ frequency, duration, type = "sine", gain = 0.08, when = 0 }) {
  const ctx = audioContext();
  if (!ctx) {
    return;
  }
  const osc = ctx.createOscillator();
  const amp = ctx.createGain();
  osc.type = type;
  osc.frequency.value = frequency;
  const t0 = ctx.currentTime + when;
  amp.gain.setValueAtTime(0, t0);
  amp.gain.linearRampToValueAtTime(gain, t0 + 0.01);
  amp.gain.exponentialRampToValueAtTime(0.0001, t0 + duration);
  osc.connect(amp);
  amp.connect(ctx.destination);
  osc.start(t0);
  osc.stop(t0 + duration + 0.02);
}

export function playUiSound(kind) {
  if (!enabled()) {
    return;
  }
  switch (kind) {
    case "notification":
      tone({ frequency: 880, duration: 0.09, gain: 0.06 });
      tone({ frequency: 1175, duration: 0.12, gain: 0.05, when: 0.08 });
      return;
    case "trash":
      tone({ frequency: 220, duration: 0.08, type: "triangle", gain: 0.05 });
      tone({ frequency: 140, duration: 0.14, type: "triangle", gain: 0.04, when: 0.06 });
      return;
    case "error":
      tone({ frequency: 180, duration: 0.16, type: "square", gain: 0.04 });
      return;
    default:
  }
}

export function uiSoundsEnabled() {
  return enabled();
}

export function setUiSoundsEnabled(on) {
  memoryState = on ? "on" : "off";
  try {
    localStorage.setItem(KEY, memoryState);
  } catch (_error) {}
}
