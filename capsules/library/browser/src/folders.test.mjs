import assert from "node:assert/strict";
import test from "node:test";

import { createLibraryFolders } from "./folders.js";
import { cacheFolderListing, setLibraryObjects } from "./state.js";

const documents = "localhost://Users/fixture/Documents";
const photos = `${documents}/Photos`;
const oldItem = { uri: `${documents}/notes.txt`, name: "notes.txt", kind: "file" };
const newItem = { uri: `${photos}/photo.png`, name: "photo.png", kind: "file" };

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function fixture() {
  const state = {
    currentUri: documents, objectsUri: documents, currentObject: { uri: documents, kind: "directory" },
    loadSeq: 0, mode: "browse", loading: false, loadError: false,
    folderCache: new Map(), selectedUris: new Set([oldItem.uri]), selectionAnchorUri: oldItem.uri,
    objectsVersion: 0,
  };
  setLibraryObjects(state, [oldItem]);
  const perf = { folderCacheHits: 0 };
  const requests = [];
  const renders = [];
  const routes = [];
  const statuses = [];
  let initialActions = 0;
  const { loadCurrentFolder } = createLibraryFolders({
    state,
    perf,
    providerApi: (_op, { uri }) => {
      const request = { uri, ...deferred() };
      requests.push(request);
      return request.promise;
    },
    setObjects: (objects) => setLibraryObjects(state, objects),
    renderAll: () => renders.push({ uri: state.currentUri, objects: [...state.objects], loading: state.loading, loadError: state.loadError }),
    setStatus: (status) => statuses.push(status),
    setFolderStatus: (status) => statuses.push(status),
    homeNavigation: { setQuery: (query) => routes.push(query) },
    runInitialObjectAction: async () => { initialActions += 1; },
  });
  return { state, perf, requests, renders, routes, statuses, loadCurrentFolder, initialActions: () => initialActions };
}

function listing(uri, objects) {
  return { object: { uri, kind: "directory" }, objects };
}

test("an uncached folder immediately clears the previous folder and selection while loading", async () => {
  const f = fixture();
  f.state.currentUri = photos;
  const pending = f.loadCurrentFolder({ useCache: true });
  const beforeResponse = {
    objects: [...f.state.objects], selected: [...f.state.selectedUris], anchor: f.state.selectionAnchorUri,
    objectsByUri: f.state.objectsByUri.size, rendered: f.renders.at(-1),
  };
  f.requests[0].resolve(listing(photos, [newItem]));
  await pending;
  assert.deepEqual(beforeResponse.objects, []);
  assert.deepEqual(beforeResponse.selected, []);
  assert.equal(beforeResponse.anchor, "");
  assert.equal(beforeResponse.objectsByUri, 0);
  assert.equal(beforeResponse.rendered?.loading, true);
  assert.deepEqual(beforeResponse.rendered?.objects, []);
  assert.deepEqual(f.state.objects, [newItem]);
  assert.equal(f.state.objectsUri, photos);
  assert.equal(f.state.loading, false);
  assert.deepEqual(f.routes, [{ uri: photos }]);
});

test("a failed folder stays empty with a retry state and can load successfully on refresh", async () => {
  const f = fixture();
  f.state.currentUri = photos;
  const pending = f.loadCurrentFolder({ useCache: true });
  f.requests[0].reject(new Error("list failed"));
  await assert.rejects(pending, /list failed/);
  assert.deepEqual(f.state.objects, []);
  assert.equal(f.state.loading, false);
  assert.equal(f.state.loadError, true);
  assert.equal(f.renders.at(-1).loadError, true);
  assert.deepEqual(f.routes, []);
  assert.equal(f.initialActions(), 0, "a failed listing must not trigger an initial item action");

  const retry = f.loadCurrentFolder();
  assert.equal(f.state.loadError, false);
  assert.equal(f.state.loading, true);
  f.requests[1].resolve(listing(photos, [newItem]));
  await retry;
  assert.deepEqual(f.state.objects, [newItem]);
  assert.equal(f.state.loadError, false);
});

test("refresh preserves the current folder and selection during a delayed or failed request", async () => {
  const f = fixture();
  const pending = f.loadCurrentFolder();
  assert.deepEqual(f.state.objects, [oldItem]);
  assert.deepEqual([...f.state.selectedUris], [oldItem.uri]);
  f.requests[0].reject(new Error("refresh failed"));
  await assert.rejects(pending, /refresh failed/);
  assert.deepEqual(f.state.objects, [oldItem]);
  assert.deepEqual([...f.state.selectedUris], [oldItem.uri]);
  assert.equal(f.state.currentObject?.uri, documents);
});

for (const outcome of ["success", "failure"]) {
  test(`a superseded folder ${outcome} leaves the new folder and its status alone`, async () => {
    const f = fixture();
    f.state.currentUri = photos;
    const obsolete = f.loadCurrentFolder({ useCache: true });
    f.state.currentUri = documents;
    const current = f.loadCurrentFolder({ useCache: true });
    f.requests[1].resolve(listing(documents, [oldItem]));
    await current;
    const renderCount = f.renders.length;
    const statusCount = f.statuses.length;
    if (outcome === "success") f.requests[0].resolve(listing(photos, [newItem]));
    else f.requests[0].reject(new Error("old folder unavailable"));
    await assert.doesNotReject(obsolete);
    assert.deepEqual(f.state.objects, [oldItem]);
    assert.equal(f.state.loading, false);
    assert.equal(f.state.loadError, false);
    assert.equal(f.renders.length, renderCount);
    assert.equal(f.statuses.length, statusCount);
    assert.deepEqual(f.routes, [{ uri: documents }]);
  });
}

test("cached folder navigation is immediate and an equivalent response avoids a second render", async () => {
  const f = fixture();
  const data = listing(photos, [newItem]);
  cacheFolderListing(f.state, f.perf, photos, data.objects, data.object);
  f.state.currentUri = photos;
  const pending = f.loadCurrentFolder({ useCache: true });
  assert.deepEqual(f.state.objects, [newItem]);
  assert.equal(f.state.loading, false);
  await Promise.resolve();
  f.requests[0].resolve(structuredClone(data));
  await pending;
  assert.equal(f.perf.folderCacheHits, 1);
  assert.equal(f.renders.length, 1);
  assert.equal(f.state.objectsUri, photos);
});
