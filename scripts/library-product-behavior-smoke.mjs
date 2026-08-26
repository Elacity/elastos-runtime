#!/usr/bin/env node
import { readFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

async function read(relativePath) {
  return readFile(path.resolve(relativePath), "utf8");
}

function createFakeClassList(initial = []) {
  const classes = new Set(initial);
  return {
    add(name) {
      classes.add(name);
    },
    remove(name) {
      classes.delete(name);
    },
    contains(name) {
      return classes.has(name);
    },
  };
}

function createFakePropertyCopyButton() {
  const attributes = new Map([
    ["data-prop-copy", "elastos://published"],
    ["data-copy-label", "published link"],
    ["data-copy-purpose", "resource.uri"],
  ]);
  const copyIcon = { hidden: false };
  const checkIcon = { hidden: true };
  return {
    dataset: {},
    classList: createFakeClassList(["props-copy-btn", "el-copy-btn"]),
    getAttribute(name) {
      return attributes.get(name) || "";
    },
    setAttribute(name, value) {
      attributes.set(name, String(value));
    },
    removeAttribute(name) {
      attributes.delete(name);
    },
    querySelector(selector) {
      if (selector === ".el-copy-icon") return copyIcon;
      if (selector === ".el-copy-check") return checkIcon;
      return null;
    },
    closest(selector) {
      return selector === "[data-prop-copy]" ? this : null;
    },
  };
}

function createFakeDialog() {
  const listeners = new Map();
  return {
    dataset: {},
    classList: createFakeClassList(["hidden"]),
    innerHTML: "",
    addEventListener(type, listener) {
      listeners.set(type, listener);
    },
    dispatch(type, event) {
      const listener = listeners.get(type);
      if (listener) {
        listener(event);
      }
    },
  };
}

function createDeferred() {
  let resolve = () => {};
  let reject = () => {};
  const promise = new Promise((nextResolve, nextReject) => {
    resolve = nextResolve;
    reject = nextReject;
  });
  return { promise, resolve, reject };
}

async function flushMicrotasks() {
  await Promise.resolve();
  await Promise.resolve();
}

async function assertDialogCopyFeedbackBehavior() {
  const dialogModuleUrl = `${pathToFileURL(path.resolve("capsules/library/browser/src/dialog.js")).href}?library-smoke=${Date.now()}`;
  const { createLibraryDialog } = await import(dialogModuleUrl);
  const realSetTimeout = globalThis.setTimeout;
  const realClearTimeout = globalThis.clearTimeout;
  const timers = [];
  globalThis.setTimeout = (callback, _delay) => {
    const handle = { callback, cleared: false };
    timers.push(handle);
    return handle;
  };
  globalThis.clearTimeout = (handle) => {
    if (handle) {
      handle.cleared = true;
    }
  };
  try {
    const successButton = createFakePropertyCopyButton();
    const successDialog = createFakeDialog();
    const successDeferred = createDeferred();
    const successCalls = [];
    const successApi = createLibraryDialog({
      copyText: (...args) => {
        successCalls.push(args);
        return successDeferred.promise;
      },
      dialog: successDialog,
      hideMenu: () => {},
      objectByUri: () => null,
      onBeforeClose: () => {},
    });
    successApi.bindDialogEvents();
    successDialog.dispatch("click", { target: successButton });
    assert(successCalls.length === 1, "Library Properties copy must call the existing clipboard path once.");
    assert(
      successCalls[0][0] === "elastos://published"
        && successCalls[0][1] === "published link"
        && successCalls[0][2] === "resource.uri",
      "Library Properties copy must keep the existing Home Clipboard purpose binding.",
    );
    assert(!successButton.classList.contains("copied"), "Library must not show copied feedback before copy succeeds.");
    successDeferred.resolve();
    await flushMicrotasks();
    assert(successButton.classList.contains("copied"), "Library must show copied feedback after copy succeeds.");
    assert(successButton.dataset.copied === "true", "Library must record copied feedback only after success.");
    assert(successButton.getAttribute("aria-label") === "Copied published link", "Library must expose copied feedback accessibly after success.");
    assert(successButton.querySelector(".el-copy-icon").hidden, "Library must hide the copy glyph after success.");
    assert(!successButton.querySelector(".el-copy-check").hidden, "Library must show the check glyph after success.");
    assert(timers.length === 1, "Library success feedback must schedule one reset timer.");
    timers[0].callback();
    assert(!successButton.classList.contains("copied"), "Library must clear copied feedback after the reset timer.");
    assert(successButton.dataset.copied === undefined, "Library must clear copied dataset state after the reset timer.");
    assert(successButton.getAttribute("aria-label") === "", "Library must clear copied aria state after the reset timer.");
    assert(!successButton.querySelector(".el-copy-icon").hidden, "Library must restore the copy glyph after the reset timer.");
    assert(successButton.querySelector(".el-copy-check").hidden, "Library must hide the check glyph after the reset timer.");

    const rejectButton = createFakePropertyCopyButton();
    const rejectDialog = createFakeDialog();
    const rejectDeferred = createDeferred();
    const rejectApi = createLibraryDialog({
      copyText: () => rejectDeferred.promise,
      dialog: rejectDialog,
      hideMenu: () => {},
      objectByUri: () => null,
      onBeforeClose: () => {},
    });
    rejectApi.bindDialogEvents();
    rejectDialog.dispatch("click", { target: rejectButton });
    assert(!rejectButton.classList.contains("copied"), "Library must keep normal copy state while a rejected write is still pending.");
    rejectDeferred.reject(new Error("clipboard denied"));
    await flushMicrotasks();
    assert(!rejectButton.classList.contains("copied"), "Library must not show copied feedback when the clipboard write fails.");
    assert(rejectButton.dataset.copied === undefined, "Library must keep copied dataset state clear on rejection.");
    assert(rejectButton.getAttribute("aria-label") === "", "Library must keep normal aria state on rejection.");
    assert(!rejectButton.querySelector(".el-copy-icon").hidden, "Library must keep the copy glyph visible on rejection.");
    assert(rejectButton.querySelector(".el-copy-check").hidden, "Library must keep the check glyph hidden on rejection.");
  } finally {
    globalThis.setTimeout = realSetTimeout;
    globalThis.clearTimeout = realClearTimeout;
  }
}

async function run() {
  const index = await read("capsules/library/browser/index.html");
  const app = await read("capsules/library/browser/src/app.js");
  const css = await read("capsules/library/browser/library.css");
  const dialog = await read("capsules/library/browser/src/dialog.js");
  const state = await read("capsules/library/browser/src/state.js");

  assert(
    index.includes('<script src="./elastos-theme.js"></script>') &&
      index.includes('<link rel="stylesheet" href="./elastos-ui.css">'),
    "Library must load the canonical shared theme and token sheet.",
  );
  assert(index.includes('id="search-toggle-button"'), "Library must expose a dedicated Search button.");
  assert(index.includes('id="more-button"'), "Library must expose a dedicated More button.");
  assert(index.includes('id="sidebar-resizer"'), "Library must expose a sidebar resizer.");
  assert(
    index.includes('aria-valuemin="180"') &&
      index.includes('aria-valuemax="320"') &&
      index.includes('aria-valuenow="220"'),
    "Library sidebar resize must expose bounded ARIA values.",
  );
  assert(
    index.includes('id="sort-select" class="select hidden"') &&
      index.includes('id="refresh-button" class="btn hidden"'),
    "Library must keep toolbar Sort and Refresh hidden when More and Home menus already expose them.",
  );
  assert(
    index.includes('id="search-toggle-button"') &&
      index.includes('icons/search.svg') &&
      index.includes('id="more-button"') &&
      index.includes('icons/more.svg') &&
      !index.includes(">Search</button>") &&
      !index.includes(">More</button>"),
    "Library Search and More must render as capsule-owned icon buttons, not text buttons.",
  );
  assert(
    index.includes('id="content" class="content" data-view="list"') &&
      state.includes('view: storage.getItem("library.view") || "list"'),
    "Library must open in list view by default while keeping the view session-only.",
  );
  assert(
    app.includes("function setFolderStatus(text)") &&
      app.includes("if (isAttachMode()) {") &&
      app.includes("setStatus(attachStatusText());") &&
      app.includes("setStatus(\"\");") &&
      !app.includes("setStatus(text);"),
    "Library must keep ordinary item counts in the footer and reserve toolbar status for picker and transient states.",
  );
  assert(
    app.includes("function placeIconMarkup(root)") &&
      app.includes("if (root?.id === \"trash\") {") &&
      app.includes("iconPlaceholder(placeIcon(root), \"place-icon window-sidebar-item-icon\")") &&
      app.includes("place-icon place-icon-accent window-sidebar-item-icon") &&
      app.includes("--place-mask: url('"),
    "Library places must use accent-masked sidebar icons while Trash keeps its rendered asset.",
  );
  assert(
    dialog.includes('class="props-copy-btn el-copy-btn"') &&
      dialog.includes('class="el-copy-icon"') &&
      dialog.includes('class="el-copy-check"') &&
      dialog.includes("setPropertyCopyFeedback(propertyCopy, label, true);") &&
      dialog.includes("setPropertyCopyFeedback(propertyCopy, label, false);"),
    "Library Properties copy buttons must keep the donor copy feedback without changing clipboard authority.",
  );

  assert(
    app.includes('window.top.postMessage({ type: "home:app-ready", homeToken: state.homeToken }, homeOrigin);') &&
      app.includes('if (!state.homeToken || !homeOrigin || window.top === window || !homeChromeReady) {') &&
      app.includes('if (signature === lastHomeMenuManifestSignature) {') &&
      app.includes('homeChromeReady = true;') &&
      app.includes('syncHomeMenuManifest();'),
    "Library must send Home ready before the first menu manifest and skip unchanged manifests.",
  );
  assert(
    app.includes('if (event.origin !== "null" || event.source !== window.parent) {') &&
      app.includes('message?.type !== "elastos:menu-command"'),
    "Library must accept shell menu commands only from the exact trusted parent frame.",
  );
  assert(
    app.includes('showBackgroundMenu(rect.left, rect.bottom + 8);') &&
      app.includes('event.stopPropagation();') &&
      app.includes('elements.moreButton.addEventListener("click"'),
    "Library More must reuse the current background menu path.",
  );
  assert(
    app.includes('elements.searchToggleButton.addEventListener("click"') &&
      app.includes('state.searchOpen = nextOpen;') &&
      app.includes('elements.toolbarSearch?.classList.toggle("open", searchOpen);') &&
      app.includes('elements.searchToggleButton.setAttribute("aria-expanded", searchOpen ? "true" : "false");') &&
      app.includes('elements.search.focus({ preventScroll: true });'),
    "Library Search must use the current in-app toggle path and focus the exact field.",
  );
  assert(
    app.includes('elements.sidebarResizer?.addEventListener("pointerdown"') &&
      app.includes('elements.libraryShell.style.setProperty("--library-sidebar-width"') &&
      app.includes('elements.sidebarResizer?.setAttribute("aria-valuenow", String(state.sidebarWidth));'),
    "Library sidebar resize must stay presentation-only in the current shell.",
  );
  assert(
    app.includes('elements.sortSelect.classList.toggle("hidden", true);') &&
      app.includes('elements.refreshButton.classList.toggle("hidden", true);'),
    "Library must keep duplicate toolbar actions hidden in source, not only by initial markup.",
  );
  assert(
    !app.includes('viewPreferenceStore.setItem("library.sidebarWidth"') &&
      !app.includes("localStorage") &&
      !app.includes("sessionStorage"),
    "Library UIUX must not add durable browser storage for layout or object state.",
  );
  assert(
    !app.includes('from "./tags.js"') &&
      !index.includes("tags.js"),
    "Library tags must stay deferred until a typed object metadata contract exists.",
  );
  assert(
    css.includes(".toolbar-search") &&
      css.includes(".toolbar-search.open .search") &&
      css.includes(".sidebar-resizer::after") &&
      css.includes(".place-icon-accent") &&
      css.includes('@media (prefers-reduced-motion: reduce)'),
    "Library UIUX CSS must include the donor search, motion, and resizer presentation behavior.",
  );
  await assertDialogCopyFeedbackBehavior();

  console.log("PASS Library product behavior smoke");
}

run().catch((error) => {
  console.error(error?.stack || String(error));
  process.exit(1);
});
