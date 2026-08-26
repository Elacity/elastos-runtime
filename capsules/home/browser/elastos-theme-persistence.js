/* ElastOS UI — host-side theme persistence.
 *
 * Host-local and deliberately not vendored: the shared theme runtime holds an
 * in-memory view and never reaches for browser-profile storage, because capsule
 * frames are opaque-sandboxed and must not own it. The Home host is the one
 * surface permitted to persist, so it installs the adapter here and relays the
 * resolved preference to embedded capsules by message.
 *
 * Load after elastos-theme.js.
 */
(function () {
  const theme = window.elastosTheme;
  if (!theme || typeof theme.setPersistence !== "function") {
    return;
  }
  theme.setPersistence({
    get(key) {
      return localStorage.getItem(key);
    },
    set(key, value) {
      localStorage.setItem(key, value);
    },
  });
})();
