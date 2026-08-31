import assert from "node:assert/strict";
import test from "node:test";

import { bindLibraryEvents } from "./events.js";

function fakeElement({ hidden = true } = {}) {
  return {
    listeners: new Map(),
    dataset: {},
    value: "",
    files: null,
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
  const selected = [{ uri: "localhost://Users/test/Documents/fixture.md", name: "fixture.md" }];
  const openCalls = [];
  let renameCount = 0;
  let deleteCount = 0;
  let trashCount = 0;
  let selectAllCount = 0;
  const windowListeners = new Map();
  const documentListeners = new Map();

  const previousWindow = globalThis.window;
  const previousDocument = globalThis.document;
  globalThis.window = {
    addEventListener(type, listener) {
      windowListeners.set(type, listener);
    },
    innerWidth: 800,
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

  bindLibraryEvents({
    bindDialogEvents() {},
    clearSelection() {},
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
    objectByUri(uri) {
      return selected.find((object) => object.uri === uri) || null;
    },
    openObject: async (object) => {
      openCalls.push(object.name);
    },
    prepareDragSelection() {},
    reorderPlace() {},
    scheduleContentRender() {},
    selectAllVisible() {
      selectAllCount += 1;
    },
    selectOnly() {},
    selectedObjects() {
      return selected;
    },
    selectRangeTo() {},
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
    state: { query: "", selectedUris: new Set(selected.map((object) => object.uri)) },
    stopLibraryEventStream() {},
    toggleSelected() {},
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
