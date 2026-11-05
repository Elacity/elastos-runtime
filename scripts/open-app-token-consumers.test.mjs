import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const read = path => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
function fn(source, name) {
  const plain = source.indexOf(`function ${name}(`);
  assert.ok(plain >= 0, name);
  const start = source.slice(plain - 6, plain) === "async " ? plain - 6 : plain;
  return source.slice(start, source.indexOf("\n}", plain) + 2);
}
function fixture() {
  const requests = [], clips = [];
  let options;
  const window = { location: { hash: "#home_token=old", search: "?home_origin=https://home.fixture" }, top: {},
    setTimeout, clearTimeout };
  const context = vm.createContext({ window, URLSearchParams, Headers, AbortController,
    document: { getElementById: () => ({}), querySelector: () => ({}) },
    createHomeNavigationClient(value) { options = value; return {}; },
    createHomeClipboardClient(value) {
      const clip = { token: value.homeToken, retired: false, start() {}, setHomeToken(token) { this.token = token; }, teardown() { this.retired = true; } };
      clips.push(clip); return clip;
    },
    fetch: async (path, options) => {
      requests.push({ path, token: new Headers(options.headers).get("x-elastos-home-token"), body: options.body });
      return { ok: true, status: 200, headers: new Headers({ "content-type": "application/json" }),
        text: async () => "{}", json: async () => ({}) };
    },
    sharedFileName: value => value || "", readText: value => value || "",
    DOCUMENTS_REQUEST_DEADLINE_MS: 1000,
  });
  return { context, requests, clips, renew() { assert.ok(options?.onHomeToken); options.onHomeToken("fresh"); } };
}

test("People requests and Clipboard use the replacement token", async () => {
  const f = fixture(), source = read("capsules/people/browser/people.js");
  vm.runInContext(source.slice(source.indexOf("const lockedShell"), source.indexOf("let localProfileId")), f.context);
  vm.runInContext(fn(source, "fetchJson"), f.context);
  await f.context.fetchJson("/people"); f.renew(); await f.context.fetchJson("/people");
  assert.deepEqual(f.requests.map(r => r.token), ["old", "fresh"]);
  assert.equal(f.clips.length, 1); assert.equal(f.clips[0].retired, false); assert.equal(f.clips[0].token, "fresh");
});

test("Documents keeps its draft and sends the next save with the replacement token", async () => {
  const f = fixture(), source = read("capsules/documents/browser/documents.js");
  vm.runInContext(source.slice(source.indexOf("const state"), source.indexOf("const DOCUMENTS_WINDOW_CLOSE_REQUEST_TYPE")), f.context);
  vm.runInContext(fn(source, "documentsApi"), f.context);
  vm.runInContext('state.dirty = true; state.current = { text: "Keep my edits" };', f.context);
  await f.context.documentsApi("/save", { method: "POST", body: "first revision" });
  f.renew();
  await f.context.documentsApi("/save", { method: "POST", body: "saved revision" });
  assert.deepEqual(f.requests.map(r => r.token), ["old", "fresh"]);
  assert.equal(f.requests[1].body, "saved revision");
  assert.equal(vm.runInContext("state.dirty && state.current.text", f.context), "Keep my edits");
});

test("Assistant requests and token listeners receive the replacement", async () => {
  const f = fixture(), source = read("capsules/assistant/browser/harness-host.js");
  vm.runInContext(source.slice(source.indexOf("let homeToken"), source.indexOf("/* ---- shell-core")).replaceAll("export ", ""), f.context);
  vm.runInContext(fn(source, "fetchJson"), f.context);
  f.context.homeOrigin = () => "https://home.fixture";
  vm.runInContext(fn(source, "getHomeGuiLaunchToken"), f.context);
  const app = read("capsules/assistant/browser/home-agent.js");
  vm.runInContext(app.slice(app.indexOf("const clipboardOptions"), app.indexOf("/* A copy Home")), f.context);
  let observed; f.context.onHomeTokenChanged(token => { observed = token; });
  await f.context.fetchJson("/assistant"); f.renew(); await f.context.fetchJson("/assistant");
  assert.deepEqual(f.requests.map(r => r.token), ["old", "fresh"]);
  assert.equal(observed, "fresh");
  assert.equal(f.clips[0].token, "fresh");
  assert.equal(f.clips[0].retired, false);
});

test("Library requests read the renewed state and replace Clipboard authority", async () => {
  const f = fixture(), source = read("capsules/library/browser/src/app.js");
  vm.runInContext('const state = { homeToken: "old", mode: "browse" }; const queryParams = new URLSearchParams(window.location.search);', f.context);
  vm.runInContext(read("capsules/library/browser/src/api.js").replaceAll("export ", ""), f.context);
  vm.runInContext(source.slice(source.indexOf('    const homeOrigin ='), source.indexOf('    const elements =')), f.context);
  await vm.runInContext('runtimeProviderApi("read", {})', f.context);
  f.renew();
  await vm.runInContext('runtimeProviderApi("write", { text: "saved" })', f.context);
  assert.deepEqual(f.requests.map(r => r.token), ["old", "fresh"]);
  assert.equal(f.clips.length, 1); assert.equal(f.clips[0].retired, false); assert.equal(f.clips[0].token, "fresh");
});

test("Only Home's exact proactive request reaches Browser's renewal adapter", () => {
  const source = read("capsules/browser/browser/browser.js");
  const start = source.indexOf('window.addEventListener("message", (event) => {');
  let receive, renewals = 0;
  const top = {};
  const context = vm.createContext({ window: { top, addEventListener(type, callback) { receive = callback; } },
    homeParentOrigin: "https://home.fixture", launchToken: "browser-old", requestHomeRelaunch() { renewals++; } });
  vm.runInContext(source.slice(start, source.indexOf("  if (isHomeBrowserWindowCloseRequest", start)) + "});", context);
  const data = { type: "elastos.home.authority.request/v1", homeToken: "browser-old" };
  receive({ data, source: {}, origin: "https://home.fixture" });
  receive({ data, source: top, origin: "https://evil.fixture" });
  receive({ data: { ...data, homeToken: "other" }, source: top, origin: "https://home.fixture" });
  assert.equal(renewals, 0);
  receive({ data, source: top, origin: "https://home.fixture" });
  assert.equal(renewals, 1);
});
