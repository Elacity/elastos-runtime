import assert from "node:assert/strict";
import test from "node:test";

import { createLibraryActions } from "./actions.js";
import { decimalIntegerToHexQuantity } from "./model.js";

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
    showProtectAndListDialog: async () => false,
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

test("Library converts positive decimal integers to canonical hex quantities", () => {
  assert.equal(decimalIntegerToHexQuantity("1"), "0x1");
  assert.equal(decimalIntegerToHexQuantity("15"), "0xf");
  assert.equal(decimalIntegerToHexQuantity("00016"), "0x10");
  assert.equal(
    decimalIntegerToHexQuantity("18446744073709551616"),
    "0x10000000000000000",
  );
  assert.equal(
    decimalIntegerToHexQuantity("115792089237316195423570985008687907853269984665640564039457584007913129639935"),
    "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
  );
  assert.throws(() => decimalIntegerToHexQuantity("0"), /1 to 2\^256-1|positive whole number/);
  assert.throws(() => decimalIntegerToHexQuantity("-1"), /positive whole number/);
  assert.throws(() => decimalIntegerToHexQuantity("1.5"), /positive whole number/);
  assert.throws(() => decimalIntegerToHexQuantity(""), /positive whole number/);
  assert.throws(
    () => decimalIntegerToHexQuantity("115792089237316195423570985008687907853269984665640564039457584007913129639936"),
    /1 to 2\^256-1/,
  );
});

test("Library protect and list sends the exact runtime custody publish shape", async () => {
  const requests = [];
  const statuses = [];
  const reloads = [];
  const actions = makeActions({
    async loadCurrentFolder() {
      reloads.push("reload");
    },
    async providerApi(op, payload) {
      requests.push({ op, payload });
      return { object: { uri: payload.uri } };
    },
    setStatus(status) {
      statuses.push(status);
    },
    async showProtectAndListDialog() {
      return {
        copies: "2",
        price: "1000000000000000000",
      };
    },
  });

  await actions.protectAndListObject({
    uri: "localhost://Users/test/Videos/movie.mp4",
    revision: "rev:movie",
    name: "movie.mp4",
    mime: "video/mp4",
    capabilities: ["publish"],
  });

  assert.deepEqual(requests, [
    {
      op: "publish",
      payload: {
        uri: "localhost://Users/test/Videos/movie.mp4",
        if_revision: "rev:movie",
        protection: {
          mode: "runtime_custody",
          copies: "0x2",
          price: "0xde0b6b3a7640000",
        },
      },
    },
  ]);
  assert.deepEqual(statuses, [
    "Protecting movie.mp4 as 2 copies at 1000000000000000000 base units...",
    "Protected and listed movie.mp4.",
  ]);
  assert.deepEqual(reloads, ["reload"]);
});

test("Library plain publish stays unchanged", async () => {
  const requests = [];
  const actions = makeActions({
    async providerApi(op, payload) {
      requests.push({ op, payload });
      return {};
    },
  });

  await actions.publishObject({
    uri: "localhost://Users/test/Documents/guide.md",
    revision: "rev:guide",
    name: "guide.md",
  });

  assert.deepEqual(requests, [
    {
      op: "publish",
      payload: {
        uri: "localhost://Users/test/Documents/guide.md",
        if_revision: "rev:guide",
      },
    },
  ]);
});

test("Library protect and list ignores ineligible objects", async () => {
  const requests = [];
  const dialogCalls = [];
  const actions = makeActions({
    async providerApi(op, payload) {
      requests.push({ op, payload });
      return {};
    },
    async showProtectAndListDialog(object) {
      dialogCalls.push(object?.uri || "");
      return { copies: "1", price: "1" };
    },
  });

  const cases = [
    {
      uri: "localhost://Users/test/Documents/folder",
      kind: "directory",
      name: "folder",
      mime: "inode/directory",
      capabilities: ["publish"],
    },
    {
      uri: "localhost://Users/test/Documents/image.png",
      kind: "file",
      name: "image.png",
      mime: "image/png",
      capabilities: ["publish"],
    },
    {
      uri: "localhost://WebSpaces/Cloud/movie.mp4",
      kind: "file",
      name: "movie.mp4",
      mime: "video/mp4",
      capabilities: ["publish"],
    },
    {
      uri: "localhost://Users/test/.Trash/movie.mp4",
      kind: "file",
      name: "movie.mp4",
      mime: "video/mp4",
      capabilities: ["publish"],
    },
    {
      uri: "localhost://Users/test/Documents/locked.mp4",
      kind: "file",
      name: "locked.mp4",
      mime: "video/mp4",
      capabilities: ["publish"],
      metadata: { readonly: true },
    },
    {
      uri: "localhost://Users/test/Documents/published.mp4",
      kind: "file",
      name: "published.mp4",
      mime: "video/mp4",
      capabilities: ["publish"],
      published: true,
    },
    {
      uri: "localhost://Users/test/Documents/protected.mp4",
      kind: "file",
      name: "protected.mp4",
      mime: "video/mp4",
      capabilities: ["publish"],
      metadata: { protected_content: { schema: "elastos.library.protected-content-identity/v1" } },
    },
    {
      uri: "localhost://Users/test/Documents/missing-capability.mp4",
      kind: "file",
      name: "missing-capability.mp4",
      mime: "video/mp4",
    },
  ];

  for (const object of cases) {
    await actions.protectAndListObject(object);
  }

  assert.deepEqual(dialogCalls, []);
  assert.deepEqual(requests, []);
});

test("Library renders pending, denied, unavailable, and failed protection states", async () => {
  const cases = [
    ["Runtime custody creator mint is pending exact Wallet or Chain settlement", "Protection for movie.mp4 is pending approval or confirmation."],
    ["Runtime custody creator mint was denied", "Protection for movie.mp4 was denied."],
    ["Runtime custody composition is unavailable", "Protection for movie.mp4 is unavailable."],
    ["unexpected failure", "Protection for movie.mp4 failed."],
  ];

  for (const [message, expected] of cases) {
    const statuses = [];
    const reloads = [];
    const actions = makeActions({
      async loadCurrentFolder() {
        reloads.push("reload");
      },
      async providerApi() {
        throw new Error(message);
      },
      setStatus(status) {
        statuses.push(status);
      },
      async showProtectAndListDialog() {
        return { copies: "1", price: "5" };
      },
    });

    await actions.protectAndListObject({
      uri: "localhost://Users/test/Videos/movie.mp4",
      revision: "rev:movie",
      name: "movie.mp4",
      mime: "video/mp4",
      capabilities: ["publish"],
    });

    assert.equal(statuses.at(-1), expected);
    assert.deepEqual(reloads, []);
  }
});

test("Library coalesces repeated protect and list clicks for one object", async () => {
  const requests = [];
  const statuses = [];
  const reloads = [];
  let releaseRequest;
  const requestPending = new Promise((resolve) => {
    releaseRequest = resolve;
  });
  let dialogCalls = 0;
  const actions = makeActions({
    async loadCurrentFolder() {
      reloads.push("reload");
    },
    async providerApi(op, payload) {
      requests.push({ op, payload });
      return requestPending;
    },
    setStatus(status) {
      statuses.push(status);
    },
    async showProtectAndListDialog() {
      dialogCalls += 1;
      return { copies: "1", price: "5" };
    },
  });
  const object = {
    uri: "localhost://Users/test/Videos/movie.mp4",
    revision: "rev:movie",
    name: "movie.mp4",
    mime: "video/mp4",
    capabilities: ["publish"],
  };

  const first = actions.protectAndListObject(object);
  await Promise.resolve();
  const second = actions.protectAndListObject(object);
  await Promise.resolve();

  assert.equal(dialogCalls, 1);
  assert.equal(requests.length, 1);
  assert(statuses.includes("Protection for movie.mp4 is already in progress."));

  releaseRequest({ object: { uri: object.uri } });
  await Promise.all([first, second]);
  assert.equal(requests.length, 1);
  assert.deepEqual(reloads, ["reload"]);
  assert.equal(statuses.at(-1), "Protected and listed movie.mp4.");
});
