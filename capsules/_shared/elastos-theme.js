/* ElastOS UI — shared theme runtime (single source of truth).
 *
 * Vendored into each participating capsule's browser/ dir by `just vendor-ui`,
 * next to elastos-ui.css. Under the opaque-sandbox shell model capsule frames
 * have NO localStorage (every access throws), so the Home host is the
 * canonical store and preferences travel by message:
 *
 *   host localStorage["elastos.ui.theme"]  = "dark" | "light" | "auto"
 *   host localStorage["elastos.ui.accent"] = "blue" | "purple" | "pink" |
 *       "red" | "orange" | "yellow" | "green" | "graphite"
 *
 * Dark is the default theme when unset; blue is the default accent. "auto"
 * follows prefers-color-scheme. The resolved theme is applied as
 * data-el-theme="light" on <html> (absence means dark); the accent as
 * data-el-accent (absence means blue) — both activate token blocks in
 * elastos-ui.css. Cross-document sync is the shell's `elastos:ui-preference`
 * message (opaque parent only); localStorage + `storage` events remain a
 * best-effort path for standalone same-origin documents.
 */
(function () {
  const KEY = "elastos.ui.theme";
  const ACCENT_KEY = "elastos.ui.accent";
  const ACCENTS = [
    "blue",
    "purple",
    "pink",
    "red",
    "orange",
    "yellow",
    "green",
    "graphite",
  ];
  const media = window.matchMedia
    ? window.matchMedia("(prefers-color-scheme: light)")
    : null;
  // Opaque-sandboxed frames throw on every localStorage access, which used to
  // make set()/setAccent() no-ops there (write fails, the reader uses the
  // default). The in-memory override keeps the current document honest;
  // persistence stays with the shell's canonical store.
  let memoryPreference = "";
  let memoryAccent = "";

  function preference() {
    if (memoryPreference) {
      return memoryPreference;
    }
    try {
      const value = localStorage.getItem(KEY);
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

  function accentPreference() {
    if (memoryAccent) {
      return memoryAccent;
    }
    try {
      const value = localStorage.getItem(ACCENT_KEY);
      if (ACCENTS.includes(value)) {
        return value;
      }
    } catch (_error) {
      // Storage may be unavailable; fall through to the default.
    }
    return "blue";
  }

  function apply() {
    if (resolve(preference()) === "light") {
      document.documentElement.setAttribute("data-el-theme", "light");
    } else {
      document.documentElement.removeAttribute("data-el-theme");
    }
    const accent = accentPreference();
    if (accent === "blue") {
      document.documentElement.removeAttribute("data-el-accent");
    } else {
      document.documentElement.setAttribute("data-el-accent", accent);
    }
  }

  apply();

  window.addEventListener("storage", (event) => {
    if (event.key === KEY || event.key === ACCENT_KEY || event.key === null) {
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

  // Opaque-sandboxed frames (no allow-same-origin) throw on every
  // localStorage access, so persistence is best-effort and the shell relays
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
    if (typeof preferences.accent === "string") {
      window.elastosTheme.setAccent(preferences.accent);
    }
  });

  window.elastosTheme = {
    preference,
    resolved: () => resolve(preference()),
    set(value) {
      const pref = value === "light" || value === "auto" ? value : "dark";
      memoryPreference = pref;
      try {
        localStorage.setItem(KEY, pref);
      } catch (_error) {
        // Storage unavailable (opaque frame) — the memory override applies.
      }
      apply();
    },
    accents: ACCENTS.slice(),
    accent: accentPreference,
    setAccent(value) {
      const accent = ACCENTS.includes(value) ? value : "blue";
      memoryAccent = accent;
      try {
        localStorage.setItem(ACCENT_KEY, accent);
      } catch (_error) {
        // Storage unavailable (opaque frame) — the memory override applies.
      }
      apply();
    },
  };
})();
