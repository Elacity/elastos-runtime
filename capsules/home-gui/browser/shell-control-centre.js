import { uiSoundsEnabled, setUiSoundsEnabled, playUiSound } from "./shell-sounds.js?v=home-20260718n";
import { showWalletRail, walletRailAvailable } from "./shell-wallet-rail.js?v=home-20260718n";

/* Control Centre: the quick layer for controls that already have canonical
   stores — theme (elastos-theme.js), UI sounds (shell-sounds.js), fullscreen
   (home-gui facade) — plus the wallet entry point. The System app keeps the
   deep versions; this panel only projects and toggles existing state, it
   owns none of its own.

   Same popover contract as the notification center: starts hidden, outside
   pointerdown or Escape dismisses, focus returns to the bar button. */

let panel = null;
let button = null;
let themeSegment = null;
let soundsSwitch = null;
let walletRow = null;
let outsideDismissBound = false;

export function bindControlCentre() {
  if (panel) {
    return;
  }
  panel = document.querySelector("#control-centre");
  button = document.querySelector("#toolbar-control-centre");
  themeSegment = document.querySelector("#control-centre-theme");
  soundsSwitch = document.querySelector("#control-centre-sounds");
  walletRow = document.querySelector("#control-centre-wallet");
  if (!panel || !button) {
    return;
  }

  button.addEventListener("click", () => {
    toggleControlCentre();
  });

  themeSegment?.addEventListener("click", (event) => {
    const option = event.target.closest("[data-theme-option]");
    if (!option || !window.elastosTheme) {
      return;
    }
    window.elastosTheme.set(option.dataset.themeOption);
    syncThemeSegment();
  });

  soundsSwitch?.addEventListener("click", () => {
    const next = !uiSoundsEnabled();
    setUiSoundsEnabled(next);
    syncSoundsSwitch();
    if (next) {
      playUiSound("notification");
    }
  });

  walletRow?.addEventListener("click", () => {
    if (!walletRailAvailable()) {
      return;
    }
    hideControlCentre({ restoreFocus: false });
    showWalletRail();
  });

  panel.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      hideControlCentre();
    }
  });
}

export function controlCentreOpen() {
  return Boolean(panel) && !panel.hidden;
}

export function showControlCentre() {
  if (!panel) {
    return;
  }
  syncThemeSegment();
  syncSoundsSwitch();
  syncWalletRow();
  panel.hidden = false;
  panel.inert = false;
  panel.setAttribute("aria-hidden", "false");
  button?.setAttribute("aria-expanded", "true");
  bindOutsideDismiss();
  panel.focus({ preventScroll: true });
}

export function hideControlCentre({ restoreFocus = true } = {}) {
  if (!controlCentreOpen()) {
    return;
  }
  panel.hidden = true;
  panel.inert = true;
  panel.setAttribute("aria-hidden", "true");
  button?.setAttribute("aria-expanded", "false");
  if (restoreFocus) {
    button?.focus();
  }
}

export function toggleControlCentre() {
  if (controlCentreOpen()) {
    hideControlCentre();
  } else {
    showControlCentre();
  }
}

function syncThemeSegment() {
  if (!themeSegment) {
    return;
  }
  const preference = window.elastosTheme?.preference() || "dark";
  const options = Array.from(themeSegment.querySelectorAll("[data-theme-option]"));
  for (const [index, option] of options.entries()) {
    const active = option.dataset.themeOption === preference;
    option.setAttribute("aria-checked", active ? "true" : "false");
    option.classList.toggle("active", active);
    if (active) {
      // Drives the sliding thumb; while the panel is display:none the value
      // still lands, so reopening snaps into place without a phantom slide.
      themeSegment.style.setProperty("--segment-index", String(index));
    }
  }
}

function syncSoundsSwitch() {
  if (!soundsSwitch) {
    return;
  }
  soundsSwitch.setAttribute("aria-checked", uiSoundsEnabled() ? "true" : "false");
}

function syncWalletRow() {
  if (!walletRow) {
    return;
  }
  // No wallet target in this home means no wallet affordance — never a dead
  // button (fail-closed, matching the capability discipline everywhere else).
  walletRow.hidden = !walletRailAvailable();
}

function bindOutsideDismiss() {
  if (outsideDismissBound) {
    return;
  }
  outsideDismissBound = true;
  document.addEventListener("pointerdown", (event) => {
    if (
      controlCentreOpen() &&
      !panel.contains(event.target) &&
      !button.contains(event.target)
    ) {
      hideControlCentre({ restoreFocus: false });
    }
  });
}
