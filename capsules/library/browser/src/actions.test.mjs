import assert from "node:assert/strict";
import test from "node:test";

import { createLibraryActions } from "./actions.js";
import { decimalIntegerToHexQuantity } from "./model.js";

function makeActions(overrides = {}) {
  return createLibraryActions({
    clearSelection() {},
    closeSelf() {},
    confirmDestructive: async () => true,
    confirmPublish: async () => true,
    currentFolderReadOnly: () => false,
    deliverToTarget: () => false,
    downloadObjectRaw: async () => ({ blob: new Blob([]), filename: "item.bin" }),
    loadCurrentFolder: async () => {},
    loadRoots: async () => {},
    navigate: async () => {},
    offerDownload: async () => false,
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
    showPublishedLink() {},
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
    "localhost://Users/smoke/Documents/Guide.md",
    "object URI",
    "resource.uri",
  );

  assert.deepEqual(identifiers, [
    "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
  ]);
  assert.deepEqual(uris, [
    "localhost://Users/smoke/Documents/Guide.md",
  ]);
  assert.deepEqual(statuses, [
    "Copied content CID.",
    "Copied object URI.",
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
    capabilities: ["publish", "protect"],
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

test("Library opens a published file from its local copy", async () => {
  const launches = [];
  const actions = makeActions({
    openTarget(target, query) {
      launches.push({ target, query });
      return true;
    },
  });

  await actions.openObject({
    uri: "localhost://Users/test/Documents/guide.md",
    name: "guide.md",
    mime: "text/markdown",
    published: true,
    published_cid: "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
    viewers: [{ id: "documents", label: "Documents" }],
  });

  assert.deepEqual(launches, [
    {
      target: "documents",
      query: {
        objectUri: "localhost://Users/test/Documents/guide.md",
        uri: "localhost://Users/test/Documents/guide.md",
        name: "guide.md",
        mime: "text/markdown",
      },
    },
  ]);
});

test("Library offers a PDF with no viewer as a download, never as a preview or Documents", async () => {
  const offers = [];
  const previews = [];
  const launches = [];
  const downloads = [];
  const statuses = [];
  const clicks = [];
  const previousDocument = globalThis.document;
  globalThis.document = {
    body: { appendChild() {} },
    createElement: () => ({ click() { clicks.push(this.download); }, remove() {} }),
  };
  try {
    const pdf = {
      uri: "localhost://Users/test/Documents/letter.pdf",
      name: "letter.pdf",
      mime: "application/pdf",
      capabilities: ["open", "read", "download"],
    };
    for (const accept of [false, true]) {
      const actions = makeActions({
        async downloadObjectRaw(payload) {
          downloads.push(payload.uri);
          return { blob: new Blob(["%PDF-1.4"]), filename: "letter.pdf" };
        },
        async offerDownload(object) {
          offers.push(object.uri);
          return accept;
        },
        openTarget(target) {
          launches.push(target);
          return true;
        },
        async previewObject(object) {
          previews.push(object.uri);
        },
        setStatus(status) {
          statuses.push(status);
        },
      });
      await actions.openObject(pdf);
    }
  } finally {
    globalThis.document = previousDocument;
  }
  assert.deepEqual(offers, [
    "localhost://Users/test/Documents/letter.pdf",
    "localhost://Users/test/Documents/letter.pdf",
  ]);
  assert.deepEqual(previews, []);
  assert.deepEqual(launches, []);
  assert.deepEqual(downloads, ["localhost://Users/test/Documents/letter.pdf"]);
  assert.deepEqual(clicks, ["letter.pdf"]);
  assert.deepEqual(statuses, ["Downloaded letter.pdf."]);
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
    {
      uri: "localhost://Users/test/Documents/protection-not-released.mp4",
      kind: "file",
      name: "protection-not-released.mp4",
      mime: "video/mp4",
      capabilities: ["publish"],
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
      capabilities: ["publish", "protect"],
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
    capabilities: ["publish", "protect"],
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

const DOCS = "localhost://Users/test/Documents";

function file(name, extra = {}) {
  return {
    uri: `${DOCS}/${name}`,
    name,
    kind: "file",
    revision: `rev:${name}`,
    capabilities: ["publish", "unpublish", "download"],
    ...extra,
  };
}

function publishHarness({ confirm = true, failures = {}, clipboardFails = false, selection = [] } = {}) {
  const requests = [];
  const statuses = [];
  const confirmations = [];
  const linkDialogs = [];
  const clipboard = [];
  const actions = makeActions({
    async confirmPublish(request) {
      confirmations.push(request);
      return confirm;
    },
    async providerApi(op, payload) {
      requests.push({ op, payload });
      const failure = failures[payload.uri];
      if (failure) {
        const { code, reason } = typeof failure === "string" ? { code: failure } : failure;
        throw Object.assign(new Error(`provider said ${code}`), { code, reason });
      }
      return { uri: "elastos://bafkreipublished" };
    },
    // A folder reload writes its own status, as Library's does.
    async loadCurrentFolder() {
      statuses.push("3 items");
    },
    selectedObjects: () => selection,
    setStatus(status) {
      statuses.push(status);
    },
    showPublishedLink(request) {
      linkDialogs.push(request);
    },
    async writeResourceUri(value) {
      if (clipboardFails) throw new Error("Clipboard write was denied.");
      clipboard.push(value);
    },
  });
  return { actions, clipboard, confirmations, linkDialogs, requests, statuses };
}

test("Library Publish asks once and sends nothing when cancelled", async () => {
  const one = publishHarness({ confirm: false });
  await one.actions.publishObject(file("Guide.md"));
  assert.equal(one.confirmations.length, 1);
  assert.equal(one.confirmations[0].title, "Publish publicly?");
  assert.deepEqual(one.requests, []);

  const many = publishHarness({ confirm: false, selection: [file("A.md"), file("B.md")] });
  await many.actions.publishSelectedObjects();
  assert.equal(many.confirmations.length, 1);
  assert.deepEqual(many.requests, []);
});

test("Library Publish sends the plain publish shape after one confirmation", async () => {
  const harness = publishHarness();
  await harness.actions.publishObject(file("Guide.md"));
  assert.deepEqual(harness.requests, [
    { op: "publish", payload: { uri: `${DOCS}/Guide.md`, if_revision: "rev:Guide.md" } },
  ]);
  assert.equal(harness.statuses.at(-1), "Published Guide.md.");
});

test("Library Publish Selected lists eligible files and leaves folders and published files out", async () => {
  const folder = { uri: `${DOCS}/Projects`, name: "Projects", kind: "directory", capabilities: ["publish"] };
  const published = file("Shared.md", { published: true, published_cid: "bafkreishared" });
  const harness = publishHarness({ selection: [file("A.md"), folder, published, file("B.md")] });
  await harness.actions.publishSelectedObjects();
  assert.equal(harness.confirmations.length, 1);
  assert.deepEqual(harness.confirmations[0].files.map((object) => object.name), ["A.md", "B.md"]);
  assert.deepEqual(harness.confirmations[0].excluded.map((object) => object.name), ["Projects", "Shared.md"]);
  assert.deepEqual(harness.requests.map(({ payload }) => payload.uri), [`${DOCS}/A.md`, `${DOCS}/B.md`]);
  assert.equal(harness.statuses.at(-1), "Published 2 files.");
});

test("Library Publish Selected with only folders asks nothing and says why", async () => {
  const folder = { uri: `${DOCS}/Projects`, name: "Projects", kind: "directory" };
  const harness = publishHarness({ selection: [folder] });
  await harness.actions.publishSelectedObjects();
  assert.deepEqual(harness.confirmations, []);
  assert.deepEqual(harness.requests, []);
  assert.equal(harness.statuses.at(-1), "Select files to publish. Folders can't be published.");
});

test("Library Publish Selected reports partial success in plain words", async () => {
  const harness = publishHarness({
    selection: [file("A.md"), file("Empty.md"), file("C.md")],
    failures: { [`${DOCS}/Empty.md`]: "empty_file" },
  });
  await harness.actions.publishSelectedObjects();
  assert.equal(harness.requests.length, 3);
  assert.equal(harness.statuses.at(-1), "Published 2 of 3 files. Empty.md: Empty files can't be published.",
    "the result must stay visible after the folder reloads");

  const two = publishHarness({
    selection: [file("A.md"), file("Big.bin"), file("Old.md")],
    failures: { [`${DOCS}/Big.bin`]: "too_large", [`${DOCS}/Old.md`]: "stale_revision" },
  });
  await two.actions.publishSelectedObjects();
  assert.equal(
    two.statuses.at(-1),
    "Published 1 of 3 files. Big.bin: This file is too large to publish. Files up to 8 MiB can be published. Old.md: This file changed. Refresh Library and try again.",
  );
});

test("Library Publish maps each error code to plain words", async () => {
  const expected = {
    empty_file: "Empty files can't be published.",
    too_large: "This file is too large to publish. Files up to 8 MiB can be published.",
    stale_revision: "This file changed. Refresh Library and try again.",
    unsupported: "This file can't be published from here.",
    unavailable: "Publishing isn't available right now. Try again.",
    changed_since_publish: "This file changed after it was published. Unpublish it first, then publish it again.",
    library_error: "Guide.md could not be published. Try again.",
  };
  for (const [code, text] of Object.entries(expected)) {
    const harness = publishHarness({ failures: { [`${DOCS}/Guide.md`]: code } });
    await harness.actions.publishObject(file("Guide.md"));
    assert.equal(harness.statuses.at(-1), text, code);
  }
  for (const [reason, text] of [
    ["folder", "Folders can't be published."],
    ["protected", "Protected files can't be published."],
    ["symlink", "This file can't be published from here."],
  ]) {
    const harness = publishHarness({ failures: { [`${DOCS}/Guide.md`]: { code: "unsupported", reason } } });
    await harness.actions.publishObject(file("Guide.md"));
    assert.equal(harness.statuses.at(-1), text, reason);
  }
  const ok = publishHarness();
  await ok.actions.publishObject(file("Guide.md"));
  assert.equal(ok.statuses.at(-1), "Published Guide.md.", "success must stay visible after the folder reloads");
});

test("Library Copy Link copies the recorded publication, never the current file CID", async () => {
  const harness = publishHarness();
  await harness.actions.copyPublishedLink(file("Guide.md", {
    published: true,
    published_cid: "bafkreirecorded",
    content_cid: "bafkreicurrentbytes",
  }));
  assert.deepEqual(harness.clipboard, ["elastos://bafkreirecorded"]);
  assert.deepEqual(harness.linkDialogs, []);
  assert.equal(harness.statuses.at(-1), "Copied the link to Guide.md.");
});

test("Library Copy Link shows the link as selectable text when the clipboard fails", async () => {
  const harness = publishHarness({ clipboardFails: true });
  await harness.actions.copyPublishedLink(file("Guide.md", { published: true, published_cid: "bafkreirecorded" }));
  assert.deepEqual(harness.linkDialogs, [{ name: "Guide.md", link: "elastos://bafkreirecorded" }]);
  assert.equal(harness.statuses.at(-1), "Copying didn't work. Select the link to copy it.");
});

test("Library gives Chat the file's address and revision, and Browser the file's bytes", async () => {
  const deliveries = [];
  const downloads = [];
  const harness = (returnTarget) => makeActions({
    closeSelf() {},
    async deliverToTarget(target, payload) {
      deliveries.push({ target, payload });
      return true;
    },
    async downloadObjectRaw({ uri }) {
      downloads.push(uri);
      return { blob: new Blob(["hello"], { type: "text/plain" }), filename: "Note.txt" };
    },
    state: { mode: "attach", returnTarget },
  });
  const note = file("Note.txt", { mime: "text/plain", size: 5 });
  await harness("chat-room").attachObject(note);
  const chat = deliveries.at(-1);
  assert.equal(chat.target, "chat-room");
  assert.equal(chat.payload.type, "chat-room:attach-library-item");
  assert.equal(chat.payload.objectUri, `${DOCS}/Note.txt`);
  assert.equal(chat.payload.revision, "rev:Note.txt");
  assert.equal(chat.payload.fileName, "Note.txt");
  assert.equal(chat.payload.sizeBytes, 5);

  const big = file("Big.bin", { size: 8 * 1024 * 1024 + 1 });
  await harness("chat-room").attachObject(big);
  assert.deepEqual(downloads, [`${DOCS}/Note.txt`], "Library must not read a file over Chat's limit");
  assert.equal(deliveries.at(-1).payload.blob, undefined);
  assert.equal(deliveries.at(-1).payload.sizeBytes, 8 * 1024 * 1024 + 1);

  await harness("browser").attachObject(note);
  const browser = deliveries.at(-1);
  assert.equal(browser.payload.type, "browser:file-picker-selection");
  assert.equal(await browser.payload.blob.text(), "hello");
  assert.equal(browser.payload.revision, undefined);
});
