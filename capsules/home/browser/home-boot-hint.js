(() => {
  try {
    const hintKey = "elastos.home.active-shell-hint";
    const hintedShell = window.localStorage.getItem(hintKey);
    if (hintedShell !== "home-cli") {
      window.localStorage.removeItem(hintKey);
      return;
    }
    document.documentElement.dataset.homeShellHint = "alternate";
    document.documentElement.dataset.homeShellBoot = "alternate";
  } catch (_error) {}
})();
