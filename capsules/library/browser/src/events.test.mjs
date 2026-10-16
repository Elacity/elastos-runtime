import assert from "node:assert/strict";
import test from "node:test";

import { bindLibraryEvents } from "./events.js";
import { createLibrarySelection } from "./selection.js";

function fakeElement({ hidden = true } = {}) {
  return {
    listeners: new Map(),
    dataset: {},
    value: "",
    files: null,
    attributes: new Map(),
    getAttribute(name) { return this.attributes.get(name) || null; },
    setAttribute(name, value) { this.attributes.set(name, value); },
    classList: {
      contains(name) {
        return hidden && name === "hidden";
      },
      add() {},
      remove() {},
    },
    addEventListener(type, listener) {
      const listeners = this.listeners.get(type) || [];
      listeners.push(listener);
      this.listeners.set(type, listeners);
    },
    click() {},
    contains(target) {
      return !!target?.__insideContent;
    },
    getBoundingClientRect() {
      return { left: 0, top: 0, width: 640, height: 480 };
    },
    querySelector() {
      return null;
    },
    querySelectorAll() {
      return [];
    },
  };
}

function fakeTarget({ insideContent = false, editable = false } = {}) {
  return {
    __insideContent: insideContent,
    closest(selector) {
      if (
        editable
        && selector === "input, textarea, select, [contenteditable='true']"
      ) {
        return this;
      }
      return null;
    },
  };
}

function dispatchWindowKey(windowListener, key, target, extras = {}) {
  let prevented = false;
  windowListener({
    key,
    target,
    ctrlKey: false,
    metaKey: false,
    shiftKey: false,
    preventDefault() {
      prevented = true;
    },
    ...extras,
  });
  return prevented;
}

function createHarness() {
  const objects = ["fixture.md", "second.md", "third.md", "fourth.md", "last.md"].map((name) => ({
    uri: `localhost://Users/test/Documents/${name}`, name,
  }));
  const state = {
    query: "", selectedUris: new Set([objects[0].uri]), selectionAnchorUri: objects[0].uri,
    objectsByUri: new Map(objects.map((object) => [object.uri, object])),
  };
  const openCalls = [];
  let renameCount = 0;
  let deleteCount = 0;
  let trashCount = 0;
  let selectAllCount = 0;
  const windowListeners = new Map();
  const documentListeners = new Map();
  const animationFrames = [];

  const previousWindow = globalThis.window;
  const previousDocument = globalThis.document;
  globalThis.window = {
    addEventListener(type, listener) {
      windowListeners.set(type, listener);
    },
    innerWidth: 800,
    requestAnimationFrame(callback) { animationFrames.push(callback); },
  };
  globalThis.document = {
    addEventListener(type, listener) {
      documentListeners.set(type, listener);
    },
  };

  const elements = {
    sidebar: fakeElement(),
    places: fakeElement(),
    breadcrumbs: fakeElement(),
    search: fakeElement(),
    sortSelect: fakeElement(),
    backButton: fakeElement(),
    forwardButton: fakeElement(),
    upButton: fakeElement(),
    refreshButton: fakeElement(),
    gridButton: fakeElement(),
    listButton: fakeElement(),
    uploadButton: fakeElement(),
    newFolderButton: fakeElement(),
    fileInput: fakeElement(),
    content: fakeElement(),
    dialog: fakeElement({ hidden: true }),
    contextMenu: fakeElement({ hidden: true }),
  };
  const items = objects.map((object, index) => ({
    __insideContent: true,
    dataset: { uri: object.uri },
    tabIndex: index === 0 ? 0 : -1,
    offsetTop: Math.floor(index / 2) * 100,
    attributes: new Map(),
    closest(selector) {
      return selector === ".item" || selector === ".item[data-uri]" ? this : null;
    },
    getAttribute(name) { return this.attributes.get(name) || null; },
    setAttribute(name, value) { this.attributes.set(name, value); },
    focus() {
      globalThis.document.activeElement = this;
      elements.content.listeners.get("focusin")?.forEach((listener) => listener({ target: this }));
    },
    scrollIntoView() { this.scrolled = true; },
  }));
  elements.content.querySelectorAll = (selector) => selector === '.item[tabindex="0"]'
    ? items.filter((item) => item.tabIndex === 0) : items;
  elements.content.querySelector = (selector) => selector === '.item[tabindex="0"]'
    ? items.find((item) => item.tabIndex === 0) : null;
  const selection = createLibrarySelection({ content: elements.content, state,
    visibleObjects: () => objects, renderFooter() {} });
  selection.syncSelectionDom();

  bindLibraryEvents({
    ...selection,
    bindDialogEvents() {},
    copySelectedObjectsTo() {},
    createFolder: async () => {},
    deleteSelectedObjects: async () => {
      deleteCount += 1;
    },
    elements,
    handleBrowserPopState: async () => {},
    hideDialog() {},
    hideMenu() {},
    loadCurrentFolder: async () => {},
    moveSelectedObjectsTo() {},
    navigate: async () => {},
    navigateBack: async () => {},
    navigateForward: async () => {},
    navigateUp: async () => {},
    openObject: async (object) => {
      openCalls.push(object.name);
    },
    reorderPlace() {},
    scheduleContentRender() {},
    selectAllVisible() {
      selectAllCount += 1;
      selection.selectAllVisible();
    },
    setSort() {},
    setView() {},
    showBackgroundMenu() {},
    showError(error) {
      throw error;
    },
    showMenuForObject() {},
    showPlaceMenu() {},
    startRename() {
      renameCount += 1;
    },
    state,
    stopLibraryEventStream() {},
    trashSelectedObjects: async () => {
      trashCount += 1;
    },
    uploadFiles: async () => {},
  });

  return {
    cleanup() {
      globalThis.window = previousWindow;
      globalThis.document = previousDocument;
    },
    deleteCount: () => deleteCount,
    elements,
    items,
    selection,
    state,
    animationFrames,
    pointerDown() { windowListeners.get("pointerdown")?.(); },
    openCalls,
    renameCount: () => renameCount,
    selectAllCount: () => selectAllCount,
    trashCount: () => trashCount,
    windowKeydown() {
      return windowListeners.get("keydown");
    },
  };
}

test("Library shortcuts stay on the content surface and keep toolbar Enter native", async () => {
  const harness = createHarness();
  try {
    const onKeydown = harness.windowKeydown();
    const toolbarTarget = fakeTarget();
    const itemTarget = fakeTarget({ insideContent: true });

    assert.equal(dispatchWindowKey(onKeydown, "Enter", toolbarTarget), false);
    assert.deepEqual(harness.openCalls, []);

    assert.equal(dispatchWindowKey(onKeydown, "Enter", harness.elements.backButton), false);
    assert.deepEqual(harness.openCalls, []);

    assert.equal(dispatchWindowKey(onKeydown, "Enter", harness.elements.newFolderButton), false);
    assert.deepEqual(harness.openCalls, []);

    assert.equal(dispatchWindowKey(onKeydown, "Enter", itemTarget), true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.deepEqual(harness.openCalls, ["fixture.md"]);
  } finally {
    harness.cleanup();
  }
});

test("Library file shortcuts do not hijack toolbar controls", () => {
  const harness = createHarness();
  try {
    const onKeydown = harness.windowKeydown();
    const toolbarTarget = fakeTarget();
    assert.equal(dispatchWindowKey(onKeydown, "F2", toolbarTarget), false);
    assert.equal(dispatchWindowKey(onKeydown, "Delete", toolbarTarget), false);
    assert.equal(
      dispatchWindowKey(onKeydown, "a", toolbarTarget, { ctrlKey: true }),
      false,
    );
    assert.equal(harness.renameCount(), 0);
    assert.equal(harness.deleteCount(), 0);
    assert.equal(harness.trashCount(), 0);
    assert.equal(harness.selectAllCount(), 0);
  } finally {
    harness.cleanup();
  }
});

test("Library file shortcuts still work from the content surface only", async () => {
  const harness = createHarness();
  try {
    const onKeydown = harness.windowKeydown();
    const itemTarget = fakeTarget({ insideContent: true });

    assert.equal(dispatchWindowKey(onKeydown, "F2", itemTarget), true);
    assert.equal(dispatchWindowKey(onKeydown, "Delete", itemTarget), true);
    assert.equal(
      dispatchWindowKey(onKeydown, "a", itemTarget, { ctrlKey: true }),
      true,
    );
    assert.equal(harness.renameCount(), 1);
    assert.equal(harness.trashCount(), 1);
    assert.equal(harness.selectAllCount(), 1);
    assert.equal(harness.deleteCount(), 0);
  } finally {
    harness.cleanup();
  }
});

test("Library file shortcuts pause while the context menu is open", () => {
  const harness = createHarness();
  try {
    const onKeydown = harness.windowKeydown();
    const itemTarget = fakeTarget({ insideContent: true });
    harness.elements.contextMenu.classList.contains = (name) => name !== "hidden";

    assert.equal(dispatchWindowKey(onKeydown, "Enter", itemTarget), false);
    assert.equal(dispatchWindowKey(onKeydown, "F2", itemTarget), false);
    assert.equal(dispatchWindowKey(onKeydown, "Delete", itemTarget), false);
    assert.equal(harness.renameCount(), 0);
    assert.equal(harness.trashCount(), 0);
    assert.deepEqual(harness.openCalls, []);
  } finally {
    harness.cleanup();
  }
});

test("Library opens the focused item and preserves a selection that includes it", async () => {
  const harness = createHarness();
  try {
    const onKeydown = harness.windowKeydown();
    assert.equal(dispatchWindowKey(onKeydown, "Enter", harness.items[1]), true);
    await Promise.resolve();
    assert.deepEqual(harness.openCalls, ["second.md"]);
    assert.deepEqual([...harness.state.selectedUris], [harness.items[1].dataset.uri]);
    harness.selection.toggleSelected(harness.items[2].dataset.uri);
    dispatchWindowKey(onKeydown, "Enter", harness.items[2]);
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.deepEqual(harness.openCalls, ["second.md", "second.md", "third.md"]);
  } finally {
    harness.cleanup();
  }
});

test("Library list keys move one tab stop, extend a range, and toggle selection", () => {
  const harness = createHarness();
  try {
    const onKeydown = harness.windowKeydown();
    const { items, state } = harness;
    items[0].focus();
    assert.equal(dispatchWindowKey(onKeydown, "ArrowDown", items[0]), true);
    assert.equal(document.activeElement, items[1]);
    assert.equal(items[1].scrolled, true);
    assert.deepEqual(items.filter((item) => item.tabIndex === 0), [items[1]]);
    assert.deepEqual([...state.selectedUris], [items[1].dataset.uri]);
    dispatchWindowKey(onKeydown, "ArrowDown", items[1], { shiftKey: true });
    assert.deepEqual([...state.selectedUris], [items[1].dataset.uri, items[2].dataset.uri]);
    dispatchWindowKey(onKeydown, "ArrowUp", items[2], { shiftKey: true });
    assert.deepEqual([...state.selectedUris], [items[1].dataset.uri]);
    dispatchWindowKey(onKeydown, "End", items[1], { ctrlKey: true });
    assert.equal(document.activeElement, items.at(-1));
    assert.deepEqual([...state.selectedUris], [items[1].dataset.uri]);
    dispatchWindowKey(onKeydown, " ", items.at(-1));
    assert.deepEqual([...state.selectedUris], [items[1].dataset.uri, items.at(-1).dataset.uri]);
    assert.equal(items.at(-1).attributes.get("aria-selected"), "true");
    dispatchWindowKey(onKeydown, " ", items.at(-1));
    assert.equal(items.at(-1).attributes.get("aria-selected"), "false");
    dispatchWindowKey(onKeydown, "Home", items.at(-1));
    assert.equal(document.activeElement, items[0]);
    assert.deepEqual([...state.selectedUris], [items[0].dataset.uri]);
    assert.equal(dispatchWindowKey(onKeydown, "ArrowLeft", items[0]), false);
  } finally {
    harness.cleanup();
  }
});

test("Library grid arrows follow columns and stay within the first and last rows", () => {
  const harness = createHarness();
  try {
    const onKeydown = harness.windowKeydown();
    const { items } = harness;
    harness.elements.content.dataset.view = "grid";
    items[1].focus();
    dispatchWindowKey(onKeydown, "ArrowUp", items[1]);
    assert.equal(document.activeElement, items[1]);
    dispatchWindowKey(onKeydown, "ArrowDown", items[1]);
    assert.equal(document.activeElement, items[3]);
    dispatchWindowKey(onKeydown, "ArrowDown", items[3]);
    assert.equal(document.activeElement, items[4]);
    dispatchWindowKey(onKeydown, "ArrowDown", items[4]);
    assert.equal(document.activeElement, items[4]);
    dispatchWindowKey(onKeydown, "ArrowUp", items[4]);
    assert.equal(document.activeElement, items[2]);
    dispatchWindowKey(onKeydown, "ArrowRight", items[2]);
    assert.equal(document.activeElement, items[3]);
    dispatchWindowKey(onKeydown, "ArrowLeft", items[3]);
    assert.equal(document.activeElement, items[2]);
  } finally {
    harness.cleanup();
  }
});

test("Library rename and delete use the focused item when it is outside the selection", () => {
  const harness = createHarness();
  try {
    const onKeydown = harness.windowKeydown();
    dispatchWindowKey(onKeydown, "F2", harness.items[1]);
    assert.equal(harness.renameCount(), 1);
    assert.deepEqual([...harness.state.selectedUris], [harness.items[1].dataset.uri]);
    dispatchWindowKey(onKeydown, "Delete", harness.items[2]);
    assert.equal(harness.trashCount(), 1);
    assert.deepEqual([...harness.state.selectedUris], [harness.items[2].dataset.uri]);
  } finally {
    harness.cleanup();
  }
});

test("Library navigation keys leave editing, toolbar, dialog, and menu input native", () => {
  const harness = createHarness();
  try {
    const onKeydown = harness.windowKeydown();
    for (const key of ["ArrowDown", "Home", "End", " ", "Enter", "F2", "Delete"]) {
      assert.equal(dispatchWindowKey(onKeydown, key, fakeTarget({ insideContent: true, editable: true })), false);
      assert.equal(dispatchWindowKey(onKeydown, key, harness.elements.backButton), false);
      harness.elements.dialog.classList.contains = () => false;
      assert.equal(dispatchWindowKey(onKeydown, key, harness.items[1]), false);
      harness.elements.dialog.classList.contains = () => true;
      harness.elements.contextMenu.classList.contains = () => false;
      assert.equal(dispatchWindowKey(onKeydown, key, harness.items[1]), false);
      harness.elements.contextMenu.classList.contains = () => true;
    }
    assert.deepEqual([...harness.state.selectedUris], [harness.items[0].dataset.uri]);
    assert.deepEqual(harness.openCalls, []);
  } finally {
    harness.cleanup();
  }
});

test("Library End waits for all rows and cancels when user intent or folder changes", () => {
  const harness = createHarness();
  try {
    const onKeydown = harness.windowKeydown();
    const { items, elements, animationFrames, state } = harness;
    state.contentRenderJob = 1;
    items[0].focus();
    elements.content.setAttribute("aria-busy", "true");
    dispatchWindowKey(onKeydown, "End", items[0]);
    animationFrames.shift()();
    assert.equal(document.activeElement, items[0]);
    elements.content.setAttribute("aria-busy", "false");
    animationFrames.shift()();
    assert.equal(document.activeElement, items.at(-1));
    assert.deepEqual([...state.selectedUris], [items.at(-1).dataset.uri]);

    for (const cancel of [
      () => dispatchWindowKey(onKeydown, "Home", items[0]),
      () => harness.pointerDown(),
      () => { state.contentRenderJob += 1; },
      () => { document.activeElement = elements.backButton; },
    ]) {
      items[0].focus();
      elements.content.setAttribute("aria-busy", "true");
      dispatchWindowKey(onKeydown, "End", items[0]);
      cancel();
      const afterCancelFocus = document.activeElement;
      elements.content.setAttribute("aria-busy", "false");
      animationFrames.shift()();
      assert.equal(document.activeElement, afterCancelFocus, "Cancelled End must keep the user's later focus target");
    }
  } finally {
    harness.cleanup();
  }
});

test("Library pending End takes focus from the content placeholder during a background render", () => {
  const harness = createHarness();
  try {
    const { items, elements, animationFrames, state } = harness;
    state.contentRenderJob = 1;
    harness.selection.selectOnly(items[2].dataset.uri);
    document.activeElement = elements.content;
    elements.content.setAttribute("aria-busy", "true");
    dispatchWindowKey(harness.windowKeydown(), "End", elements.content);
    assert.equal(document.activeElement, items[0], "A mounted item must keep automatic chunk restoration from cancelling End");
    assert.deepEqual([...state.selectedUris], [items[2].dataset.uri]);
    elements.content.setAttribute("aria-busy", "false");
    animationFrames.shift()();
    assert.equal(document.activeElement, items.at(-1));
    assert.deepEqual([...state.selectedUris], [items.at(-1).dataset.uri]);
  } finally {
    harness.cleanup();
  }
});
