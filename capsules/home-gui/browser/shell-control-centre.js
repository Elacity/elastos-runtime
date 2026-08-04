import {
  closeOtherShellPopovers,
  registerEscapeHandler,
  registerShellPopover,
} from "./shell-popovers.js?v=home-20260804at";
import {
  dismissWithMotion,
  prepareSurfaceOpen,
} from "./shell-motion.js?v=home-20260804at";
import {
  fetchJson,
  focusModeEnabled,
  formatBadgeCount,
  setDesktopIconsVisible,
  setFocusModeEnabled,
  shellState,
  targetById,
} from "./shell-core.js?v=home-20260804at";
import { uiSoundsEnabled, setUiSoundsEnabled, playUiSound } from "./shell-sounds.js?v=home-20260804at";
import {
  dockAutoHideEnabled,
  setDockAutoHide,
} from "./shell-surface.js?v=home-20260804at";
import { showWalletRail, walletRailAvailable } from "./shell-wallet-rail.js?v=home-20260804at";
import { showInboxRail } from "./shell-inbox-rail.js?v=home-20260804at";
import { showSpotlight } from "./shell-spotlight.js?v=home-20260804at";
import { openTarget } from "./shell-windows.js?v=home-20260804at";
import { openExpose } from "./shell-expose.js?v=home-20260804at";

/* Control Centre: the quick layer for controls that already have canonical
   stores — theme, sounds, focus, accent, dock, desktop icons — plus Nearby
   (shell-gated discovery), Mission Control, and session deep links. Owns no
   authority of its own. */

let panel = null;
let button = null;
let themeSegment = null;
let accentRow = null;
let accentCustomPanel = null;
let accentCustomPickerRoot = null;
let accentCustomPicker = null;
let accentCustomHex = null;
let accentCustomWriteTimer = 0;
let focusSwitch = null;
let approvalsRow = null;
let approvalsDetail = null;
let discoverySwitch = null;
let discoveryDetail = null;
let carrierDetail = null;
let soundsSwitch = null;
let dockSwitch = null;
let desktopIconsSwitch = null;
let showWindowsRow = null;
let whoamiDetail = null;
let walletRow = null;
let quickSpotlightRow = null;
let quickInboxRow = null;
let quickInboxDetail = null;
let quickWalletRow = null;
let quickWalletDetail = null;
let thisDeviceRow = null;
let systemRow = null;
let outsideDismissBound = false;
let registered = false;
let discoveryTick = 0;

export function bindControlCentre() {
  if (panel) {
    return;
  }
  panel = document.querySelector("#control-centre");
  button = document.querySelector("#toolbar-control-centre");
  themeSegment = document.querySelector("#control-centre-theme");
  accentRow = document.querySelector("#control-centre-accent");
  accentCustomPanel = document.querySelector("#control-centre-accent-custom");
  accentCustomPickerRoot = document.querySelector("#control-centre-accent-picker");
  accentCustomHex = document.querySelector("#control-centre-accent-hex");
  focusSwitch = document.querySelector("#control-centre-focus");
  approvalsRow = document.querySelector("#control-centre-approvals");
  approvalsDetail = document.querySelector("#control-centre-approvals-detail");
  discoverySwitch = document.querySelector("#control-centre-discovery");
  discoveryDetail = document.querySelector("#control-centre-discovery-detail");
  carrierDetail = document.querySelector("#control-centre-carrier-detail");
  soundsSwitch = document.querySelector("#control-centre-sounds");
  dockSwitch = document.querySelector("#control-centre-dock");
  desktopIconsSwitch = document.querySelector("#control-centre-desktop-icons");
  showWindowsRow = document.querySelector("#control-centre-show-windows");
  whoamiDetail = document.querySelector("#control-centre-whoami-detail");
  walletRow = document.querySelector("#control-centre-wallet");
  quickSpotlightRow = document.querySelector("#control-centre-spotlight");
  quickInboxRow = document.querySelector("#control-centre-inbox");
  quickInboxDetail = document.querySelector("#control-centre-inbox-detail");
  quickWalletRow = document.querySelector("#control-centre-quick-wallet");
  quickWalletDetail = document.querySelector("#control-centre-quick-wallet-detail");
  thisDeviceRow = document.querySelector("#control-centre-this-device");
  systemRow = document.querySelector("#control-centre-system");
  if (!panel || !button) {
    return;
  }
  if (!registered) {
    registerShellPopover("control-centre", () => hideControlCentre({ restoreFocus: false }));
    registerEscapeHandler("control-centre", {
      priority: 80,
      isActive: () => controlCentreOpen(),
      dismiss: () => hideControlCentre(),
    });
    registered = true;
  }

  buildAccentRow();

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
    window.dispatchEvent(new CustomEvent("elastos:ui-preference-changed", {
      detail: { key: "theme", value: option.dataset.themeOption },
    }));
  });

  accentRow?.addEventListener("click", (event) => {
    const option = event.target.closest("[data-accent-option]");
    if (!option || !window.elastosTheme?.setAccent) {
      return;
    }
    const next = option.dataset.accentOption;
    if (next === "custom") {
      const hex = window.elastosTheme.accentCustom?.() || "#4f7fff";
      window.elastosTheme.setAccentCustom?.(hex);
      window.elastosTheme.setAccent("custom");
      publishAccentCustom(hex);
      publishAccent("custom");
      syncAccentRow({ showCustomEditor: true });
      accentCustomPicker?.open?.();
      return;
    }
    window.elastosTheme.setAccent(next);
    publishAccent(next);
    syncAccentRow({ showCustomEditor: false });
  });

  if (accentCustomPickerRoot && window.elastosAccentPicker?.mount) {
    accentCustomPicker = window.elastosAccentPicker.mount(accentCustomPickerRoot, {
      getHex: () => window.elastosTheme?.accentCustom?.() || "#4f7fff",
      onChange: (hex) => {
        commitAccentCustom(hex, { fromWheel: true });
      },
    });
  }
  accentCustomHex?.addEventListener("change", () => {
    commitAccentCustom(accentCustomHex.value, { fromWheel: false });
  });
  accentCustomHex?.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      commitAccentCustom(accentCustomHex.value, { fromWheel: false });
    }
  });

  focusSwitch?.addEventListener("click", () => {
    const next = !focusModeEnabled();
    setFocusModeEnabled(next);
    syncFocusSwitch();
    window.dispatchEvent(new CustomEvent("elastos:ui-preference-changed", {
      detail: { key: "focusMode", value: next ? "on" : "off" },
    }));
  });

  approvalsRow?.addEventListener("click", () => {
    hideControlCentre({ restoreFocus: false });
    const pending = pendingApprovalEntries(shellState.currentSummary);
    if (pending.some((entry) => entry.kind === "wallet_approval_request")) {
      showWalletRail();
      return;
    }
    showInboxRail();
  });

  discoverySwitch?.addEventListener("click", () => {
    const on = discoverySwitch.getAttribute("aria-checked") === "true";
    setDiscoveryEnabled(!on).catch((error) => {
      console.warn("discovery toggle failed", error);
      playUiSound("error");
      syncNearby(shellState.currentSummary);
    });
  });

  soundsSwitch?.addEventListener("click", () => {
    const next = !uiSoundsEnabled();
    setUiSoundsEnabled(next);
    syncSoundsSwitch();
    window.dispatchEvent(new CustomEvent("elastos:ui-preference-changed", {
      detail: { key: "sounds", value: next ? "on" : "off" },
    }));
    if (next) {
      playUiSound("notification");
    }
  });

  dockSwitch?.addEventListener("click", () => {
    const next = !dockAutoHideEnabled();
    setDockAutoHide(next);
    syncDockSwitch();
    window.dispatchEvent(new CustomEvent("elastos:ui-preference-changed", {
      detail: { key: "dockAutoHide", value: next ? "on" : "off" },
    }));
  });

  desktopIconsSwitch?.addEventListener("click", () => {
    const visible = shellState.shellLayoutState.desktopIconsVisible !== false;
    if (setDesktopIconsVisible(!visible)) {
      syncDesktopIconsSwitch();
    }
  });

  showWindowsRow?.addEventListener("click", () => {
    hideControlCentre({ restoreFocus: false });
    openExpose();
  });

  /* Part XI: phone tray folded into CC — same openers as toolbar (UI ≠ authority). */
  quickSpotlightRow?.addEventListener("click", () => {
    hideControlCentre({ restoreFocus: false });
    showSpotlight();
  });

  quickInboxRow?.addEventListener("click", () => {
    hideControlCentre({ restoreFocus: false });
    showInboxRail();
  });

  quickWalletRow?.addEventListener("click", () => {
    if (!walletRailAvailable()) {
      return;
    }
    hideControlCentre({ restoreFocus: false });
    showWalletRail();
  });

  walletRow?.addEventListener("click", () => {
    if (!walletRailAvailable()) {
      return;
    }
    hideControlCentre({ restoreFocus: false });
    showWalletRail();
  });

  thisDeviceRow?.addEventListener("click", () => {
    hideControlCentre({ restoreFocus: false });
    openTarget("system", { query: { settings: "about" } });
  });

  systemRow?.addEventListener("click", () => {
    hideControlCentre({ restoreFocus: false });
    openTarget("system");
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
  closeOtherShellPopovers("control-centre");
  syncControlCentre(shellState.currentSummary);
  prepareSurfaceOpen(panel);
  panel.hidden = false;
  panel.inert = false;
  panel.setAttribute("aria-hidden", "false");
  button?.setAttribute("aria-expanded", "true");
  bindOutsideDismiss();
  panel.focus({ preventScroll: true });
  startDiscoveryTick();
}

export function hideControlCentre({ restoreFocus = true } = {}) {
  if (!controlCentreOpen()) {
    return;
  }
  stopDiscoveryTick();
  button?.setAttribute("aria-expanded", "false");
  dismissWithMotion(panel, {
    className: "menubar-card-leaving",
    ms: 120,
    hide: false,
    onDone: () => {
      panel.hidden = true;
      panel.inert = true;
      panel.setAttribute("aria-hidden", "true");
      if (restoreFocus) {
        button?.focus();
      }
    },
  });
}

export function toggleControlCentre() {
  if (controlCentreOpen()) {
    hideControlCentre();
  } else {
    showControlCentre();
  }
}

export function syncControlCentre(summary) {
  syncThemeSegment();
  syncAccentRow();
  syncFocusSwitch();
  syncApprovals(summary);
  syncNearby(summary);
  syncSoundsSwitch();
  syncDockSwitch();
  syncDesktopIconsSwitch();
  syncWhoami(summary);
  syncWalletRow();
  syncQuickOpen(summary);
}

function publishAccent(value) {
  window.dispatchEvent(new CustomEvent("elastos:ui-preference-changed", {
    detail: { key: "accent", value },
  }));
}

function publishAccentCustom(value) {
  window.dispatchEvent(new CustomEvent("elastos:ui-preference-changed", {
    detail: { key: "accentCustom", value },
  }));
}

function commitAccentCustom(raw, { fromWheel }) {
  if (!window.elastosTheme?.setAccentCustom) {
    return;
  }
  const hex = window.elastosTheme.normalizeHex?.(raw) || "";
  if (!hex) {
    if (accentCustomHex && !fromWheel) {
      accentCustomHex.value = window.elastosTheme.accentCustom?.() || "#4f7fff";
    }
    return;
  }
  window.elastosTheme.setAccentCustom(hex);
  if (window.elastosTheme.accent?.() !== "custom") {
    window.elastosTheme.setAccent("custom");
    publishAccent("custom");
  }
  if (accentCustomHex && !fromWheel) {
    accentCustomHex.value = hex;
  } else if (accentCustomHex && document.activeElement !== accentCustomHex) {
    accentCustomHex.value = hex;
  }
  if (!fromWheel) {
    accentCustomPicker?.setHex?.(hex);
  }
  paintCustomSwatch(hex);
  window.clearTimeout(accentCustomWriteTimer);
  accentCustomWriteTimer = window.setTimeout(() => {
    publishAccentCustom(hex);
  }, fromWheel ? 120 : 0);
}

function paintCustomSwatch(hex) {
  const swatch = accentRow?.querySelector('[data-accent-option="custom"]');
  if (!swatch) {
    return;
  }
  swatch.style.background = hex;
  swatch.style.color = hex;
}

function buildAccentRow() {
  if (!accentRow || accentRow.childElementCount > 0) {
    return;
  }
  const accents = window.elastosTheme?.accents || [
    "blue",
    "purple",
    "pink",
    "red",
    "orange",
    "yellow",
    "green",
    "graphite",
  ];
  for (const accent of accents) {
    const swatch = document.createElement("button");
    swatch.type = "button";
    swatch.className = "control-centre-accent-swatch";
    swatch.role = "radio";
    swatch.dataset.accentOption = accent;
    swatch.setAttribute("aria-label", accent.charAt(0).toUpperCase() + accent.slice(1));
    swatch.title = swatch.getAttribute("aria-label");
    accentRow.appendChild(swatch);
  }
  const custom = document.createElement("button");
  custom.type = "button";
  custom.className = "control-centre-accent-swatch control-centre-accent-swatch-custom";
  custom.role = "radio";
  custom.dataset.accentOption = "custom";
  custom.setAttribute("aria-label", "Custom");
  custom.title = "Custom color";
  accentRow.appendChild(custom);
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
      themeSegment.style.setProperty("--segment-index", String(index));
    }
  }
}

function syncAccentRow(options = {}) {
  if (!accentRow) {
    return;
  }
  const accent = window.elastosTheme?.accent?.() || "blue";
  const hex = window.elastosTheme?.accentCustom?.() || "#4f7fff";
  for (const option of accentRow.querySelectorAll("[data-accent-option]")) {
    const active = option.dataset.accentOption === accent;
    option.setAttribute("aria-checked", active ? "true" : "false");
  }
  paintCustomSwatch(hex);
  accentCustomPicker?.setHex?.(hex);
  if (accentCustomHex && document.activeElement !== accentCustomHex) {
    accentCustomHex.value = hex;
  }
  const showEditor = options.showCustomEditor === true
    || (options.showCustomEditor !== false && accent === "custom");
  if (accentCustomPanel) {
    accentCustomPanel.hidden = !showEditor;
  }
  if (!showEditor) {
    accentCustomPicker?.close?.();
  }
}

function syncFocusSwitch() {
  if (!focusSwitch) {
    return;
  }
  focusSwitch.setAttribute("aria-checked", focusModeEnabled() ? "true" : "false");
}

function pendingApprovalEntries(summary) {
  const entries = Array.isArray(summary?.notifications?.entries)
    ? summary.notifications.entries
    : [];
  return entries.filter((entry) => {
    const status = String(entry?.status || "").toLowerCase();
    if (status && status !== "pending" && status !== "open" && status !== "unread") {
      return false;
    }
    return Boolean(entry?.action_ref?.action_id || entry?.kind);
  });
}

function syncApprovals(summary) {
  if (!approvalsDetail) {
    return;
  }
  const count = pendingApprovalEntries(summary).length;
  approvalsDetail.textContent = count > 0 ? `${formatBadgeCount(count)} pending` : "None";
}

function discoveryRemainingSeconds(discovery) {
  const until = Number(discovery?.expires_at || discovery?.enabled_until || 0);
  if (!Number.isFinite(until) || until <= 0) {
    return 0;
  }
  const remaining = until - Math.floor(Date.now() / 1000);
  return Math.max(0, remaining);
}

function formatMmSs(totalSeconds) {
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}

function syncNearby(summary) {
  const discovery = summary?.people?.discovery;
  const remaining = discoveryRemainingSeconds(discovery);
  const on = discovery?.enabled === true && remaining > 0;
  discoverySwitch?.setAttribute("aria-checked", on ? "true" : "false");
  if (discoveryDetail) {
    discoveryDetail.hidden = !on;
    discoveryDetail.textContent = on ? `Discoverable · ${formatMmSs(remaining)}` : "";
  }
  if (carrierDetail) {
    const status = discovery?.status;
    carrierDetail.textContent =
      status === "visible" ? "Online" :
      status === "runtime_unavailable" ? "Unavailable" : "Idle";
  }
}

async function setDiscoveryEnabled(enabled) {
  await fetchJson("/api/apps/home/discovery", {
    method: "POST",
    body: JSON.stringify({ enabled }),
  });
  await shellState.requestSummaryRefresh?.();
  syncNearby(shellState.currentSummary);
}

function startDiscoveryTick() {
  stopDiscoveryTick();
  discoveryTick = window.setInterval(() => {
    if (!controlCentreOpen()) {
      stopDiscoveryTick();
      return;
    }
    syncNearby(shellState.currentSummary);
  }, 1000);
}

function stopDiscoveryTick() {
  if (discoveryTick) {
    window.clearInterval(discoveryTick);
    discoveryTick = 0;
  }
}

function syncSoundsSwitch() {
  if (!soundsSwitch) {
    return;
  }
  soundsSwitch.setAttribute("aria-checked", uiSoundsEnabled() ? "true" : "false");
}

function syncDockSwitch() {
  if (!dockSwitch) {
    return;
  }
  dockSwitch.setAttribute("aria-checked", dockAutoHideEnabled() ? "true" : "false");
}

function syncDesktopIconsSwitch() {
  if (!desktopIconsSwitch) {
    return;
  }
  const visible = shellState.shellLayoutState.desktopIconsVisible !== false;
  desktopIconsSwitch.setAttribute("aria-checked", visible ? "true" : "false");
}

function syncWhoami(summary) {
  if (!whoamiDetail) {
    return;
  }
  const name =
    summary?.identity?.profile_card?.display_name ||
    summary?.identity?.handle ||
    summary?.authority?.principal_id ||
    "Signed in";
  whoamiDetail.textContent = String(name);
}

function syncWalletRow() {
  if (!walletRow) {
    return;
  }
  // Toolbar Wallet is the primary door — keep CC Wallet hidden when the rail
  // is available (Jobs: quiet money). Phone uses Quick open instead (Part XI).
  walletRow.hidden = true;
}

function walletApprovalCount(summary) {
  const entries = Array.isArray(summary?.notifications?.entries)
    ? summary.notifications.entries
    : [];
  return entries.filter((entry) => {
    const actionId = entry?.action_ref?.action_id;
    return (
      entry?.kind === "wallet_approval_request" &&
      typeof actionId === "string" &&
      actionId.startsWith("wallet-approve-request:")
    );
  }).length;
}

function syncQuickOpen(summary) {
  const inboxTarget = targetById(summary, "inbox");
  if (quickInboxRow) {
    quickInboxRow.hidden = !inboxTarget;
    quickInboxRow.disabled = !inboxTarget;
  }
  if (quickInboxDetail) {
    if (!inboxTarget) {
      quickInboxDetail.textContent = "Unavailable";
    } else {
      const notifications = summary?.notifications || {};
      const entries = Array.isArray(notifications.entries) ? notifications.entries : [];
      const semanticCount =
        Number(notifications.attention_count || 0) || Number(notifications.unread_count || 0);
      const badgeCount = Math.max(0, semanticCount || entries.length);
      quickInboxDetail.textContent =
        badgeCount > 0 ? `${formatBadgeCount(badgeCount)} pending` : "Open";
    }
  }

  const walletOk = walletRailAvailable();
  if (quickWalletRow) {
    quickWalletRow.hidden = !walletOk;
  }
  if (quickWalletDetail) {
    const count = walletApprovalCount(summary);
    quickWalletDetail.textContent =
      count > 0 ? `${formatBadgeCount(count)} pending` : "Open";
  }
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
