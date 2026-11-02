import assert from "node:assert/strict";
import test from "node:test";

import { createLibraryActions } from "./actions.js";

function createUploadHarness(t) {
  const timers = [];
  const previousWindow = globalThis.window;
  globalThis.window = {
    setTimeout(callback) {
      timers.push(callback);
    },
  };
  t.after(() => {
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
  });

  const state = {
    currentUri: "localhost://Users/test/Documents",
    uploads: [],
  };
  const calls = [];
  const statuses = [];
  let readOnly = false;
  let refreshCount = 0;
  let refreshError = null;
  const actions = createLibraryActions({
    state,
    currentFolderReadOnly: () => readOnly,
    renderUploads() {},
    setStatus(status) {
      statuses.push(status);
    },
    setUploadProgress(id, patch) {
      const upload = state.uploads.find((entry) => entry.id === id);
      if (upload) Object.assign(upload, patch);
    },
    async loadCurrentFolder() {
      refreshCount += 1;
      if (refreshError) throw refreshError;
    },
    uploadObject(request) {
      return new Promise((resolve, reject) => {
        calls.push({ ...request, resolve, reject });
      });
    },
  });
  return {
    actions,
    calls,
    state,
    statuses,
    timers,
    refreshCount: () => refreshCount,
    setReadOnly(value) {
      readOnly = value;
    },
    setRefreshError(error) {
      refreshError = error;
    },
  };
}

const nextTurn = () => new Promise((resolve) => setImmediate(resolve));
const file = (name) => ({ name, type: "text/plain" });

test("Library keeps an upload batch bound to the folder chosen at start", async (t) => {
  const harness = createUploadHarness(t);
  const files = [file("first.txt"), file("second.txt")];
  const result = harness.actions.uploadFiles(files);

  assert.equal(harness.calls[0].uri, "localhost://Users/test/Documents/first.txt");
  harness.state.currentUri = "localhost://WebSpaces/ReadOnly";
  harness.setReadOnly(true);
  harness.calls[0].resolve();
  await nextTurn();

  assert.equal(harness.calls[1].uri, "localhost://Users/test/Documents/second.txt");
  assert.equal(harness.calls[1].file, files[1]);
  assert.equal(harness.calls[1].mime, "text/plain");
  harness.calls[1].resolve();
  await result;

  assert.equal(harness.refreshCount(), 1);
  assert.deepEqual(harness.state.uploads.map((upload) => upload.status), ["Complete", "Complete"]);
  harness.timers[0]();
  assert.deepEqual(harness.state.uploads, []);
});

test("Library keeps overlapping upload batches and their cleanup independent", async (t) => {
  const harness = createUploadHarness(t);
  const first = harness.actions.uploadFiles([file("first.txt"), file("second.txt")]);
  harness.state.currentUri = "localhost://Users/test/Desktop";
  const second = harness.actions.uploadFiles([file("third.txt")]);

  assert.equal(harness.state.uploads.length, 3);
  assert.equal(new Set(harness.state.uploads.map((upload) => upload.id)).size, 3);
  const rows = [...harness.state.uploads];
  harness.calls[0].onProgress(0.5);
  harness.calls[1].onProgress(0.25);
  assert.deepEqual(rows.map((row) => row.progress), [48, 0, 24]);

  harness.calls[0].resolve();
  await nextTurn();
  assert.equal(harness.calls[2].uri, "localhost://Users/test/Documents/second.txt");
  assert.equal(harness.calls[1].uri, "localhost://Users/test/Desktop/third.txt");
  harness.calls[1].resolve();
  await second;
  harness.timers.shift()();
  assert.deepEqual(harness.state.uploads, rows.slice(0, 2));
  assert.equal(rows[1].status, "Preparing");

  harness.calls[2].onProgress(0.75);
  assert.equal(rows[1].progress, 72);
  harness.calls[2].resolve();
  await first;
  harness.timers.shift()();
  assert.deepEqual(harness.state.uploads, []);
  assert.equal(harness.calls.length, 3);
  assert.equal(harness.refreshCount(), 2);
});

test("Library preserves an uncertain upload and marks undispatched files as not started", async (t) => {
  const harness = createUploadHarness(t);
  const result = harness.actions.uploadFiles([
    file("completed.txt"),
    file("uncertain.txt"),
    file("pending.txt"),
  ]);
  harness.calls[0].resolve();
  await nextTurn();
  harness.calls[1].onProgress(1);
  const error = new Error("Connection interrupted after dispatch");
  const rejected = assert.rejects(result, (actual) => actual === error);
  harness.calls[1].reject(error);
  await rejected;

  assert.equal(harness.calls.length, 2);
  assert.equal(harness.state.uploads[0].status, "Complete");
  assert.equal(harness.state.uploads[1].status, "Upload not confirmed. Check the destination before retrying.");
  assert.equal(harness.state.uploads[2].status, "Not started");
  assert.equal(harness.state.uploads[2].progress, 0);
  harness.timers.shift()();
  assert.equal(harness.state.uploads.length, 3);

  const later = harness.actions.uploadFiles([file("later.txt")]);
  assert.deepEqual(harness.state.uploads.map((row) => row.name), ["later.txt"]);
  harness.calls[2].resolve();
  await later;
  harness.timers.shift()();
  assert.deepEqual(harness.state.uploads, []);
  assert.equal(harness.calls.length, 3);
});

test("Library retires inactive upload results while preserving a concurrent active batch", async (t) => {
  const harness = createUploadHarness(t);
  const active = harness.actions.uploadFiles([file("active.txt")]);
  const activeRow = harness.state.uploads[0];
  for (let index = 0; index < 5; index += 1) {
    const result = harness.actions.uploadFiles([file(`failed-${index}.txt`), file(`pending-${index}.txt`)]);
    assert.equal(harness.state.uploads.length, 3);
    assert.equal(harness.state.uploads[0], activeRow);
    const rejected = assert.rejects(result, /Upload interrupted/);
    harness.calls.at(-1).reject(new Error("Upload interrupted"));
    await rejected;
    harness.timers.shift()();
    assert.equal(harness.state.uploads.length, 3);
    assert.equal(activeRow.status, "Preparing");
  }
  harness.calls[0].onProgress(0.5);
  assert.equal(activeRow.progress, 48);
  harness.calls[0].resolve();
  await active;
  harness.timers.shift()();
  assert.deepEqual(harness.state.uploads.map((row) => row.name), ["failed-4.txt", "pending-4.txt"]);
});

test("Library preserves existing upload state when a new selection is empty or read-only", async (t) => {
  const harness = createUploadHarness(t);
  const existing = [{ id: "existing", name: "previous.txt", status: "Complete", progress: 100 }];
  harness.state.uploads = existing;
  await harness.actions.uploadFiles([]);
  harness.setReadOnly(true);
  await harness.actions.uploadFiles([file("denied.txt")]);

  assert.equal(harness.state.uploads, existing);
  assert.deepEqual(harness.calls, []);
  assert.deepEqual(harness.statuses, ["This Space is read-only."]);
  assert.deepEqual(harness.timers, []);
});

test("Library reports confirmed uploads separately from a failed folder refresh", async (t) => {
  const harness = createUploadHarness(t);
  harness.setRefreshError(new Error("Folder listing unavailable"));
  const result = harness.actions.uploadFiles([file("saved.txt")]);
  harness.calls[0].resolve();
  await result;

  assert.equal(harness.calls.length, 1);
  assert.equal(harness.state.uploads[0].status, "Complete");
  assert.equal(harness.statuses.at(-1), "Uploaded 1 file. Refresh Library to see the latest items.");
  harness.timers[0]();
  assert.deepEqual(harness.state.uploads, []);
});
