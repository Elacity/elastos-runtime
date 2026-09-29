import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import test from "node:test";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const browser = read("capsules/browser/browser/browser.js");
const service = read("scripts/browser-selkies-control-service.mjs");
function fn(source, name) {
  const start = source.search(new RegExp(`(?:async )?function ${name}\\(`));
  assert(start >= 0, name);
  return source.slice(start, source.indexOf("\n}", start) + 2);
}
function client() {
  const calls = [];
  const context = vm.createContext({
    libraryPickerRequest: { requestId: "file-chooser:1", pageId: "page-1", consumed: false },
    libraryPickerDocumentNonce: "document-1", lastLibraryFilePickerRequestId: "file-chooser:1",
    currentPage: { page_id: "page-1" }, LIBRARY_FILE_PICKER_MAX_BYTES: 16 * 1024 * 1024,
    showStatus() {}, Uint8Array, btoa,
    sendBrowserInput: async (event) => { calls.push(event); return { accepted: true, file_upload: { request_id: event.request_id } }; },
  });
  for (const name of ["fileNameFromLibraryPayload", "base64FromBlob", "handleLibraryFilePickerSelection"]) vm.runInContext(fn(browser, name), context);
  const payload = { requestId: "file-chooser:1", documentNonce: "document-1", pickerId: "picker-1", deliveryId: "delivery-1", blob: new Blob(["data"]), fileName: "file.txt" };
  return { context, payload, calls };
}
test("Browser accepts its current request once and forwards chooser identity", async () => {
  const f = client();
  assert.equal(await f.context.handleLibraryFilePickerSelection(f.payload), true);
  assert.equal(f.calls[0].request_id, "file-chooser:1");
  assert.equal(await f.context.handleLibraryFilePickerSelection(f.payload), false);
  assert.equal(f.calls.length, 1);
});
for (const key of ["requestId", "documentNonce"]) {
  test(`Browser rejects stale ${key} before upload`, async () => {
    const f = client();
    assert.equal(await f.context.handleLibraryFilePickerSelection({ ...f.payload, [key]: "old" }), false);
    assert.equal(f.calls.length, 0);
  });
}
for (const transition of ["page", "chooser"]) {
  test(`Browser rechecks ${transition} after asynchronous byte preparation`, async () => {
    const f = client();
    let resolve;
    f.payload.blob = { size: 4, arrayBuffer: () => new Promise((done) => { resolve = done; }) };
    const pending = f.context.handleLibraryFilePickerSelection(f.payload);
    if (transition === "page") f.context.currentPage = { page_id: "page-2" };
    else f.context.libraryPickerRequest = { requestId: "file-chooser:2", pageId: "page-1" };
    resolve(new Uint8Array([1]).buffer);
    assert.equal(await pending, false);
    assert.equal(f.calls.length, 0);
  });
}
test("missing or failed Runtime acceptance never closes the picker", async () => {
  for (const response of [undefined, { accepted: true }, { accepted: false, file_upload: { request_id: "file-chooser:1" } }]) {
    const f = client();
    f.context.sendBrowserInput = async () => response;
    assert.equal(await f.context.handleLibraryFilePickerSelection(f.payload), false);
  }
});

test("a reused chooser serial on a new page opens a distinct request", () => {
  const f = client();
  let opened = 0;
  f.context.openLibraryFilePicker = () => { opened += 1; };
  vm.runInContext(fn(browser, "handleFileChooserFromStatus"), f.context);
  f.context.currentPage = { page_id: "page-2" };
  f.context.handleFileChooserFromStatus({ file_chooser: { pending: true, request_id: "file-chooser:1" } });
  assert.equal(opened, 1);
  assert.equal(f.context.libraryPickerRequest.pageId, "page-2");
});

test("late upload response does not update a replacement page", async () => {
  const f = client();
  let respond;
  let projections = 0;
  Object.assign(f.context, {
    currentDisplayMode: "fixture", fetchJson: async () => new Promise((resolve) => { respond = resolve; }),
    recoverMissingRuntimePage: () => false,
    handleFileChooserFromStatus: () => { projections += 1; }, syncViewFromResponse: () => { projections += 1; },
  });
  vm.runInContext(fn(browser, "sendBrowserInput"), f.context);
  const pending = f.context.sendBrowserInput({ type: "file_upload", request_id: "file-chooser:1" });
  f.context.currentPage = { page_id: "page-2" };
  respond({ accepted: true, actual_url: "https://old.example/", file_upload: { request_id: "file-chooser:1" } });
  await assert.rejects(pending, /file request changed/);
  assert.equal(projections, 0);
  assert.deepEqual(f.context.currentPage, { page_id: "page-2" });
});

function backend(onCommand = () => {}) {
  const calls = [];
  const chooser = { request_id: "file-chooser:1", backend_node_id: 7 };
  const page = { debugger_url: "fixture", file_chooser: { pending: chooser } };
  const newer = { request_id: "file-chooser:2", backend_node_id: 9 };
  const context = vm.createContext({
    Buffer, MAX_BROWSER_FILE_UPLOAD_BYTES: 16 * 1024 * 1024,
    fs: { mkdtempSync: () => "fixture-only", writeFileSync() {} },
    path: { join: (...parts) => parts.join("/") }, os: { tmpdir: () => "fixture-only" },
    ensureBrowserFileChooserInterception: async () => {}, browserPageStateFromCdp: async () => ({}),
    withBrowserCdp: async (_page, _timeout, run) => run({ request: async (method, params) => {
      calls.push({ method, params });
      await onCommand(method, page, newer);
      if (method === "DOM.resolveNode") return { object: { objectId: "node-7" } };
      if (method === "Runtime.callFunctionOn") return { result: { value: { ok: true } } };
      return {};
    } }),
  });
  for (const name of ["sanitizeBrowserUploadFileName", "validateBrowserFileUploadEvent", "uploadFileIntoBrowserPage"]) vm.runInContext(fn(service, name), context);
  const event = { type: "file_upload", request_id: chooser.request_id, content_base64: "ZGF0YQ==" };
  return { context, page, newer, calls, event };
}
test("CDP upload returns the exact chooser receipt and clears only that chooser", async () => {
  const f = backend();
  const result = await f.context.uploadFileIntoBrowserPage(f.page, f.event, 1000);
  assert.equal(result.file_upload.request_id, "file-chooser:1");
  assert.equal(f.page.file_chooser.pending, null);
  assert.equal(f.calls.find((call) => call.method === "DOM.setFileInputFiles").params.backendNodeId, 7);
});
test("CDP rejects absent or old chooser identity before any command", async () => {
  for (const request_id of [undefined, "file-chooser:0"]) {
    const f = backend();
    await assert.rejects(f.context.uploadFileIntoBrowserPage(f.page, { ...f.event, request_id }, 1000));
    assert.equal(f.calls.length, 0);
  }
});
for (const method of ["DOM.enable", "DOM.setFileInputFiles", "DOM.resolveNode", "Runtime.callFunctionOn"]) {
  test(`chooser replacement during ${method} preserves the new request`, async () => {
    const f = backend((command, page, newer) => { if (command === method) page.file_chooser.pending = newer; });
    await assert.rejects(f.context.uploadFileIntoBrowserPage(f.page, f.event, 1000), /no longer available/);
    assert.equal(f.page.file_chooser.pending, f.newer);
    if (method === "DOM.enable") assert(!f.calls.some((call) => call.method === "DOM.setFileInputFiles"));
    if (method !== "Runtime.callFunctionOn") assert(!f.calls.some((call) => call.method === "Runtime.callFunctionOn"));
  });
}
test("simultaneous uploads cannot both consume the same chooser", async () => {
  let resume;
  const f = backend((method) => method === "Page.enable" ? new Promise((resolve) => { resume = resolve; }) : undefined);
  const first = f.context.uploadFileIntoBrowserPage(f.page, f.event, 1000);
  await assert.rejects(f.context.uploadFileIntoBrowserPage(f.page, f.event, 1000));
  resume();
  await first;
  assert.equal(f.calls.filter((call) => call.method === "DOM.setFileInputFiles").length, 1);
});

test("an uncertain failed upload cannot replay an effect on the same chooser", async () => {
  const f = backend((method) => { if (method === "DOM.setFileInputFiles") throw new Error("fixture transport failure"); });
  await assert.rejects(f.context.uploadFileIntoBrowserPage(f.page, f.event, 1000), /transport failure/);
  await assert.rejects(f.context.uploadFileIntoBrowserPage(f.page, f.event, 1000));
  assert.equal(f.calls.filter((call) => call.method === "DOM.setFileInputFiles").length, 1);
});

assert(read("elastos/crates/elastos-server/src/api/gateway_browser.rs").includes("pub(super) event: serde_json::Value"), "existing Runtime event forwarding retains the chooser field");
