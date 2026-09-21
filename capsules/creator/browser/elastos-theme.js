/* GENERATED from capsules/_shared/elastos-theme.js — do not edit. Run `just vendor-ui`. */
/* ElastOS UI — shared theme runtime (single source of truth).
 *
 * Vendored into each participating capsule's browser/ dir by `just vendor-ui`,
 * next to elastos-ui.css. This module never reaches for browser-profile
 * storage. Capsule frames are opaque-sandboxed and must not own that storage,
 * and the Home host is the canonical store, so this module keeps an in-memory
 * view and accepts an optional persistence adapter that only a host installs:
 *
 *   elastos.ui.theme         = "dark" | "light" | "auto"
 *   elastos.ui.accent        = preset | "custom"
 *   elastos.ui.accentCustom  = "#rrggbb" (when custom)
 *
 * Dark is the default theme when unset; blue is the default accent. "auto"
 * follows prefers-color-scheme. The resolved theme is applied as
 * data-el-theme="light" on <html> (absence means dark); the accent as
 * data-el-accent (absence means blue) — both activate token blocks in
 * elastos-ui.css. Custom accent sets data-el-accent="custom" and writes
 * --el-accent / --el-accent-ink inline. Cross-document sync is the shell's
 * `elastos:ui-preference` message (opaque parent only); a host-installed
 * adapter plus `storage` events remain a best-effort path for standalone docs.
 */
(function () {
  const KEY = "elastos.ui.theme";
  const ACCENT_KEY = "elastos.ui.accent";
  const ACCENT_CUSTOM_KEY = "elastos.ui.accentCustom";
  const ACCENT_PRESETS = [
    "blue",
    "purple",
    "pink",
    "red",
    "orange",
    "yellow",
    "green",
    "graphite",
  ];
  const DEFAULT_CUSTOM_HEX = "#4f7fff";
  const media = window.matchMedia
    ? window.matchMedia("(prefers-color-scheme: light)")
    : null;
  // The in-memory override keeps the current document honest. Persistence is
  // the host's, supplied through setPersistence; without an adapter this module
  // simply holds the preference for the life of the document.
  let memoryPreference = "";
  let memoryAccent = "";
  let memoryAccentCustom = "";
  let persistence = null;

  function readStored(key) {
    if (!persistence || typeof persistence.get !== "function") {
      return "";
    }
    try {
      return persistence.get(key) || "";
    } catch (_error) {
      // A host adapter may refuse; the in-memory view still applies.
      return "";
    }
  }

  function writeStored(key, value) {
    if (!persistence || typeof persistence.set !== "function") {
      return;
    }
    try {
      persistence.set(key, value);
    } catch (_error) {
      // A host adapter may refuse; the in-memory view still applies.
    }
  }

  function preference() {
    if (memoryPreference) {
      return memoryPreference;
    }
    try {
      const value = readStored(KEY);
      if (value === "light" || value === "dark" || value === "auto") {
        return value;
      }
    } catch (_error) {
      // Storage may be unavailable; fall through to the default.
    }
    return "dark";
  }

  function resolve(pref) {
    if (pref === "auto") {
      return media && media.matches ? "light" : "dark";
    }
    return pref;
  }

  function normalizeHex(value) {
    if (typeof value !== "string") {
      return "";
    }
    let hex = value.trim();
    if (/^#[0-9A-Fa-f]{3}$/.test(hex)) {
      hex = `#${hex[1]}${hex[1]}${hex[2]}${hex[2]}${hex[3]}${hex[3]}`;
    }
    if (!/^#[0-9A-Fa-f]{6}$/.test(hex)) {
      return "";
    }
    return hex.toLowerCase();
  }

  function accentInkForHex(hex) {
    const normalized = normalizeHex(hex);
    if (!normalized) {
      return "#ffffff";
    }
    const channel = (offset) => Number.parseInt(normalized.slice(offset, offset + 2), 16) / 255;
    const linearize = (channelValue) => (
      channelValue <= 0.03928
        ? channelValue / 12.92
        : ((channelValue + 0.055) / 1.055) ** 2.4
    );
    const luminance = (
      0.2126 * linearize(channel(1))
      + 0.7152 * linearize(channel(3))
      + 0.0722 * linearize(channel(5))
    );
    return luminance > 0.55 ? "#161616" : "#ffffff";
  }

  function accentPreference() {
    if (memoryAccent) {
      return memoryAccent;
    }
    try {
      const value = readStored(ACCENT_KEY);
      if (ACCENT_PRESETS.includes(value) || value === "custom") {
        return value;
      }
    } catch (_error) {
      // Storage may be unavailable; fall through to the default.
    }
    return "blue";
  }

  function accentCustomPreference() {
    if (memoryAccentCustom) {
      return memoryAccentCustom;
    }
    try {
      const value = normalizeHex(readStored(ACCENT_CUSTOM_KEY));
      if (value) {
        return value;
      }
    } catch (_error) {
      // Storage may be unavailable; fall through to the default.
    }
    return DEFAULT_CUSTOM_HEX;
  }

  function clearInlineAccent() {
    document.documentElement.style.removeProperty("--el-accent");
    document.documentElement.style.removeProperty("--el-accent-ink");
  }

  function applyCustomAccentVars(hex) {
    const normalized = normalizeHex(hex) || DEFAULT_CUSTOM_HEX;
    document.documentElement.setAttribute("data-el-accent", "custom");
    document.documentElement.style.setProperty("--el-accent", normalized);
    document.documentElement.style.setProperty("--el-accent-ink", accentInkForHex(normalized));
  }

  function apply() {
    if (resolve(preference()) === "light") {
      document.documentElement.setAttribute("data-el-theme", "light");
    } else {
      document.documentElement.removeAttribute("data-el-theme");
    }
    const accent = accentPreference();
    if (accent === "custom") {
      applyCustomAccentVars(accentCustomPreference());
      return;
    }
    clearInlineAccent();
    if (accent === "blue") {
      document.documentElement.removeAttribute("data-el-accent");
    } else {
      document.documentElement.setAttribute("data-el-accent", accent);
    }
  }

  apply();

  window.addEventListener("storage", (event) => {
    if (
      event.key === KEY
      || event.key === ACCENT_KEY
      || event.key === ACCENT_CUSTOM_KEY
      || event.key === null
    ) {
      apply();
    }
  });

  if (media && typeof media.addEventListener === "function") {
    media.addEventListener("change", () => {
      if (preference() === "auto") {
        apply();
      }
    });
  }

  // Opaque-sandboxed frames (no allow-same-origin) own no browser-profile
  // storage, so persistence stays with the host and the shell relays
  // preferences by message instead. Cross-frame updates arrive as
  // `elastos:ui-preference` from the embedding shell (an opaque parent whose
  // security origin serializes to "null").
  window.addEventListener("message", (event) => {
    if (event.source !== window.parent || event.origin !== "null") {
      return;
    }
    const message = event.data || {};
    if (message.type !== "elastos:ui-preference") {
      return;
    }
    const preferences = message.preferences && typeof message.preferences === "object"
      ? message.preferences
      : {};
    if (typeof preferences.theme === "string") {
      window.elastosTheme.set(preferences.theme);
    }
    if (typeof preferences.accentCustom === "string") {
      window.elastosTheme.setAccentCustom(preferences.accentCustom);
    }
    if (typeof preferences.accent === "string") {
      window.elastosTheme.setAccent(preferences.accent);
    }
  });

  window.elastosTheme = {
    // Installed by a host that is permitted to own browser-profile storage.
    // Capsule frames never call this, so they never touch that storage.
    setPersistence(adapter) {
      persistence = adapter && typeof adapter === "object" ? adapter : null;
      apply();
    },
    preference,
    resolved: () => resolve(preference()),
    set(value) {
      const pref = value === "light" || value === "auto" ? value : "dark";
      memoryPreference = pref;
      writeStored(KEY, pref);
      apply();
    },
    accents: ACCENT_PRESETS.slice(),
    accent: accentPreference,
    accentCustom: accentCustomPreference,
    normalizeHex,
    accentInkForHex,
    setAccent(value) {
      const accent = ACCENT_PRESETS.includes(value) || value === "custom" ? value : "blue";
      memoryAccent = accent;
      writeStored(ACCENT_KEY, accent);
      apply();
    },
    setAccentCustom(value) {
      const hex = normalizeHex(value);
      if (!hex) {
        return false;
      }
      memoryAccentCustom = hex;
      writeStored(ACCENT_CUSTOM_KEY, hex);
      if (accentPreference() === "custom") {
        apply();
      }
      return true;
    },
  };
})();
