import assert from "node:assert/strict";
import test from "node:test";

import { createLibraryEditor } from "./editor.js";

class FakeLabel {
  constructor(onReplace) {
    this.onReplace = onReplace;
  }

  replaceWith(node) {
    this.onReplace(node);
  }
}

class FakeInput {
  constructor() {
    this.className = "";
    this.value = "";
    this.listeners = new Map();
  }

  focus() {}

  select() {}

  addEventListener(type, listener) {
    const listeners = this.listeners.get(type) || [];
    listeners.push(listener);
    this.listeners.set(type, listeners);
  }

  dispatch(type, event = {}) {
    for (const listener of this.listeners.get(type) || []) {
      listener({
        preventDefault() {},
        ...event,
      });
    }
  }
}

function flush() {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

test("Library editor lets a new-folder name retry after a failed validation", async () => {
  const statuses = [];
  const requests = [];
  let input = null;
  let reloads = 0;
  const state = {
    currentUri: "localhost://Users/test/Documents",
    draftCounter: 0,
    objects: [],
    selectedUris: new Set(),
  };

  const previousWindow = globalThis.window;
  const previousDocument = globalThis.document;
  const previousCss = globalThis.CSS;
  globalThis.window = {
    requestAnimationFrame(callback) {
      callback();
    },
  };
  globalThis.document = {
    createElement(tag) {
      assert.equal(tag, "input");
      input = new FakeInput();
      return input;
    },
  };
  globalThis.CSS = { escape(value) { return value; } };

  try {
    const editor = createLibraryEditor({
      content: {
        querySelector(selector) {
          assert.match(selector, /^\[data-name-uri="draft:/);
          return new FakeLabel(() => {});
        },
      },
      async loadCurrentFolder() {
        reloads += 1;
      },
      async providerApi(op, payload) {
        requests.push({ op, payload });
        if (op === "mkdir" && payload.name === "/") {
          throw new Error("Library action could not be completed.");
        }
        return { object: { uri: `${payload.parent_uri}/${payload.name}` } };
      },
      renderContent() {},
      setObjects(objects) {
        state.objects = objects;
      },
      setStatus(status) {
        statuses.push(status);
      },
      showError(error) {
        statuses.push(error.message);
      },
      state,
    });

    editor.startCreateObject("directory");
    assert.equal(statuses.at(-1), "Name the new folder.");
    assert.ok(input, "rename editor should open");

    input.value = "/";
    input.dispatch("keydown", { key: "Enter" });
    await flush();
    assert.equal(statuses.at(-1), "Library action could not be completed.");

    input.value = "audit-retry";
    input.dispatch("input");
    assert.equal(statuses.at(-1), "Name the new folder.");

    input.dispatch("keydown", { key: "Enter" });
    await flush();
    assert.deepEqual(
      requests.map(({ op, payload }) => ({ op, name: payload.name })),
      [
        { op: "mkdir", name: "/" },
        { op: "mkdir", name: "audit-retry" },
      ],
    );
    assert.equal(statuses.at(-1), "Created audit-retry.");
    assert.equal(reloads, 1);
  } finally {
    globalThis.window = previousWindow;
    globalThis.document = previousDocument;
    globalThis.CSS = previousCss;
  }
});
