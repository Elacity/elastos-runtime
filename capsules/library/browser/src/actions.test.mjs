import assert from "node:assert/strict";
import test from "node:test";

import { createLibraryActions } from "./actions.js";

function makeActions(overrides = {}) {
  return createLibraryActions({
    clearSelection() {},
    closeSelf() {},
    confirmDestructive: async () => true,
    currentFolderReadOnly: () => false,
    deliverToTarget: () => false,
    downloadObjectRaw: async () => ({ blob: new Blob([]), filename: "item.bin" }),
    loadCurrentFolder: async () => {},
    loadRoots: async () => {},
    navigate: async () => {},
    openPublishedUri() {},
    openTarget: () => false,
    previewObject: async () => {},
    providerApi: {},
    renderUploads() {},
    selectedObjects: () => [],
    setStatus() {},
    setUploadProgress() {},
    showMenuForObject() {},
    showObjectStatus() {},
    showProperties() {},
    showShareDialog() {},
    showShareReceipt() {},
    showSharedAccessReceipt() {},
    startCreateObject() {},
    state: {},
    uploadObject: async () => {},
    writeResourceIdentifier: async () => {},
    writeResourceUri: async () => {},
    ...overrides,
  });
}

test("Library routes identifiers and resource URIs through distinct closed writers", async () => {
  const identifiers = [];
  const uris = [];
  const statuses = [];
  const actions = makeActions({
    setStatus(status) {
      statuses.push(status);
    },
    async writeResourceIdentifier(value) {
      identifiers.push(value);
    },
    async writeResourceUri(value) {
      uris.push(value);
    },
  });

  await actions.copyText(
    "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
    "content CID",
    "resource.identifier",
  );
  await actions.copyText(
    "elastos://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
    "published link",
    "resource.uri",
  );

  assert.deepEqual(identifiers, [
    "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
  ]);
  assert.deepEqual(uris, [
    "elastos://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
  ]);
  assert.deepEqual(statuses, [
    "Copied content CID.",
    "Copied published link.",
  ]);
  await assert.rejects(
    actions.copyText("arbitrary text", "value", "browser.text"),
    /purpose is denied/,
  );
});

test("Library launches protected video in elacity-player with mint_id only", async () => {
  const launches = [];
  const previews = [];
  const actions = makeActions({
    openTarget(target, query) {
      launches.push({ target, query });
      return true;
    },
    async previewObject(object) {
      previews.push(object.uri);
    },
  });

  await actions.openObject({
    uri: "localhost://Library/movie.mp4",
    name: "movie.mp4",
    mime: "video/mp4",
    metadata: {
      protected_content: {
        schema: "elastos.library.protected-content-identity/v1",
        mint_id: "ab".repeat(32),
      },
    },
  });

  assert.deepEqual(launches, [
    {
      target: "elacity-player",
      query: { mint_id: "ab".repeat(32) },
    },
  ]);
  assert.deepEqual(previews, []);
});

test("Library keeps ordinary video preview behavior", async () => {
  const launches = [];
  const previews = [];
  const actions = makeActions({
    openTarget(target, query) {
      launches.push({ target, query });
      return true;
    },
    async previewObject(object) {
      previews.push(object.uri);
    },
  });

  await actions.openObject({
    uri: "localhost://Library/movie.mp4",
    name: "movie.mp4",
    mime: "video/mp4",
  });

  assert.deepEqual(launches, []);
  assert.deepEqual(previews, ["localhost://Library/movie.mp4"]);
});

test("Library fails closed on malformed protected video mint_id", async () => {
  const launches = [];
  const previews = [];
  const statuses = [];
  const actions = makeActions({
    openTarget(target, query) {
      launches.push({ target, query });
      return true;
    },
    async previewObject(object) {
      previews.push(object.uri);
    },
    setStatus(status) {
      statuses.push(status);
    },
  });

  await actions.openObject({
    uri: "localhost://Library/movie.mp4",
    name: "movie.mp4",
    mime: "video/mp4",
    metadata: {
      protected_content: {
        schema: "elastos.library.protected-content-identity/v1",
        mint_id: "ABC123",
      },
    },
  });

  assert.deepEqual(launches, []);
  assert.deepEqual(previews, []);
  assert.deepEqual(statuses, ["Protected video is unavailable."]);
});
