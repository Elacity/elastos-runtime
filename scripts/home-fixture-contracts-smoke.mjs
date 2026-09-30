import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import vm from "node:vm";
import test from "node:test";
import { createRequire } from "node:module";
import { EventEmitter } from "node:events";
import { PassThrough, Writable } from "node:stream";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const require = createRequire(import.meta.url);
const hostedDriver = read("scripts/browser-hosted-product-display-smoke.sh")
  .split("node - <<'NODE'\n")[1].split("\nNODE")[0];
async function hostedAdapterFixture(mode) {
  const child = new EventEmitter();
  child.stdout = new PassThrough();
  child.exitCode = null;
  child.signalCode = null;
  const operations = [], errors = [];
  let cleanup, generation, finish;
  const done = new Promise((resolve) => { finish = resolve; });
  child.stdin = new Writable({ write(chunk, _encoding, callback) {
    const request = JSON.parse(String(chunk));
    operations.push(request.op);
    let response = { status: "ok" };
    if (request.op === "launch") {
      generation = request.lifecycle_generation;
      cleanup = { page_id: "fixture-page", generation,
        stream_id: request.stream_session.stream_id, principal_id: request.principal_id };
      response.data = { schema: "elastos.browser.engine.page/v1", page_id: cleanup.page_id,
        runtime_cleanup: cleanup, direct_network: false, network_mode: "runtime_net_only",
        display_session: { schema: "elastos.browser.display-session/v1", mode: "webrtc_remote_display",
          backend_class: "product_compositor", display_backend: "fixture", audio: true, video: true,
          direct_network: false, network_mode: "runtime_net_only" } };
    }
    if (request.op === "close_page") {
      assert.deepEqual(request.runtime_cleanup, cleanup);
      assert.equal(request.principal_id, cleanup.principal_id);
      response = mode === "success" ? { status: "ok", data: {
        schema: "elastos.browser.engine-cleanup-result/v2", ...cleanup, binding: cleanup, terminal: true,
        effects: Object.fromEntries(["page_absent", "child_absent", "vm_absent", "route_absent", "socket_absent"]
          .map((effect) => [effect, true])),
      } } : { status: "error", code: "fixture_close_failed" };
    }
    if (request.op === "shutdown" && mode === "close_and_shutdown_failure") {
      response = { status: "error", code: "fixture_shutdown_failed" };
    }
    child.stdout.write(JSON.stringify(response) + "\n");
    callback();
    if (request.op === "shutdown") setImmediate(() => {
      child.exitCode = 0;
      child.stdout.end();
      child.emit("close", 0, null);
      setImmediate(finish);
    });
  } });
  child.kill = () => { throw new Error("unexpected forced fixture kill"); };
  const processFixture = { env: { CONFIG_PATH: "fixture", ADAPTER_BIN: "fixture" }, exitCode: 0 };
  vm.runInNewContext(hostedDriver, {
    require: (id) => id === "node:fs" ? { readFileSync: () => JSON.stringify({ adapters: [] }) }
      : id === "node:child_process" ? { spawn: () => child } : require(id),
    process: processFixture, console: { log() {}, error: (error) => errors.push(String(error)) },
    setTimeout, clearTimeout,
  });
  await done;
  assert.deepEqual(operations, ["init", "launch", "close_page", "shutdown"]);
  assert.equal(processFixture.exitCode, mode === "success" ? 0 : 1);
  if (mode !== "success") assert.match(errors.at(-1), /adapter close_page failed/);
  return { generation, errors };
}
for (const mode of ["success", "close_failure", "close_and_shutdown_failure"]) {
  test(`hosted adapter ${mode} retains ordered shutdown and first failure`, { timeout: 2000 }, async () => {
    const result = await hostedAdapterFixture(mode);
    if (mode === "close_and_shutdown_failure") {
      assert(result.errors.some((error) => error.includes("adapter shutdown failed")));
    }
  });
}
test("hosted adapter generations differ between runs", { timeout: 2000 }, async () => {
  const first = await hostedAdapterFixture("success");
  const second = await hostedAdapterFixture("success");
  assert.notEqual(first.generation, second.generation);
});
function fn(source, name) {
  const start = source.search(new RegExp(`(?:async )?function ${name}\\(`));
  assert(start >= 0, name);
  return source.slice(start, source.indexOf("\n}", start) + 2);
}
const entropy = read("scripts/home-entropy-check.mjs");
function traversalFixture() {
  const entry = (name, directory = false) => ({ name, isDirectory: () => directory, isFile: () => !directory });
  const directories = new Map([
    ["/fixture", [entry("target", true), entry("target-build", true), entry("docs", true), entry("README.md")]],
    ["/fixture/docs", [entry("contract.md")]],
  ]);
  const files = new Map([["/fixture/README.md", "[contract](docs/contract.md)"], ["/fixture/docs/contract.md", "Current source contract."]]);
  const context = vm.createContext({
    repoRootPath: "/fixture", resolve, dirname,
    assert: (condition, message, details) => assert(condition, `${message}: ${JSON.stringify(details)}`),
    existsSync: (path) => directories.has(path) || files.has(path) || ["/fixture/target", "/fixture/target-build"].includes(path),
    readdirSync: (path) => {
      assert(directories.has(path), `must not traverse rebuildable output: ${path}`);
      return directories.get(path);
    },
    readFileSync: (path) => {
      if (!files.has(path)) throw new Error(`ENOENT: ${path}`);
      return files.get(path);
    },
  });
  for (const name of ["listMarkdownFiles", "listTextFiles", "listFilesRecursive", "isTestOrGeneratedPath", "assertMarkdownLocalLinksResolve"]) vm.runInContext(fn(entropy, name), context);
  return { context, files };
}
test("all entropy walks exclude the configured build output and retain source docs", () => {
  assert.match(read(".cargo/config.toml"), /^build-dir = "target-build"$/m);
  for (const name of ["listMarkdownFiles", "listTextFiles", "listFilesRecursive"]) {
    const { context } = traversalFixture();
    assert.deepEqual(Array.from(context[name]("/fixture")).sort(), ["/fixture/README.md", "/fixture/docs/contract.md"]);
  }
  const { context } = traversalFixture();
  assert(context.isTestOrGeneratedPath("/fixture/target-build/debug/generated.rs"));
  assert(!context.isTestOrGeneratedPath("/fixture/docs/contract.md"));
});
test("broken source links and source read failures still fail entropy", () => {
  const { context, files } = traversalFixture();
  context.assertMarkdownLocalLinksResolve();
  files.set("/fixture/docs/contract.md", "[missing](missing.md)");
  assert.throws(() => context.assertMarkdownLocalLinksResolve(), /Markdown local links must resolve/);
  files.delete("/fixture/docs/contract.md");
  assert.throws(() => context.assertMarkdownLocalLinksResolve(), /ENOENT/);
});

const lifecycle = read("scripts/home-browser-restored-lifecycle-headless-smoke.mjs");
test("restored-lifecycle summary satisfies the actual Home appearance parser", () => {
  const context = vm.createContext({ principalId: "fixture", browserContextId: "fixture-context", browserInstanceId: "fixture-browser" });
  vm.runInContext(fn(lifecycle, "homeSummary"), context);
  vm.runInContext(fn(read("capsules/home-gui/browser/home-gui.js"), "homeGuiUiPreferencesFromSummary"), context);
  const summary = context.homeSummary();
  assert.equal(context.homeGuiUiPreferencesFromSummary(summary).theme, "dark");
  assert.throws(() => context.homeGuiUiPreferencesFromSummary({ ...summary, appearance: {} }), /rejected/);
  assert.equal(summary.authority.session_id, "fixture-session");
  for (const field of ["notifications", "desktop_objects", "capsule_catalog", "capsule_interfaces"]) assert(!Array.isArray(summary[field]), field);
  assert.match(lifecycle, /ELASTOS_BROWSER_EXECUTABLE/);
  assert.match(lifecycle, /elastos.people.discovery\/v1/);
});
test("bootstrap wait fails before polling after a captured page error", async () => {
  const context = vm.createContext({
    Date, pageErrors: ["fixture bootstrap exception"], state: { errors: [] },
    assert: (condition, message, details) => assert(condition, `${message}: ${JSON.stringify(details)}`),
    setTimeout: () => { throw new Error("must fail before retry delay"); },
  });
  vm.runInContext(fn(lifecycle, "waitFor"), context);
  await assert.rejects(context.waitFor(() => false, 15_000, "Home bootstrap"), /fixture bootstrap exception/);
});
test("presence fixture retains Home token admission and the Runtime response shape", async () => {
  const responses = [];
  const context = vm.createContext({
    homeAuthorityToken: "fixture-home-token",
    assert: (condition, message) => assert(condition, message),
    json: (_res, status, body) => responses.push({ status, body }),
    readBody: async (req) => req.body,
  });
  for (const name of ["requireToken", "handleApi"]) vm.runInContext(fn(lifecycle, name), context);
  const url = new URL("https://fixture.test/api/apps/home/collaboration/presence");
  const req = { method: "POST", headers: { "x-elastos-home-token": "fixture-home-token" }, body: {} };
  assert.equal(await context.handleApi(req, {}, url), true);
  assert.equal(responses[0].status, 200);
  assert.equal(responses[0].body.schema, "elastos.people.discovery/v1");
  assert.equal(responses[0].body.configured, false);
  await context.handleApi({ ...req, headers: {} }, {}, url);
  assert.equal(responses[1].status, 401);
  await assert.rejects(context.handleApi({ ...req, body: { principal_id: "other" } }, {}, url), /empty admitted request/);
});
test("System fixture retains four tests and bootstrap assertions without temporary tracing", () => {
  const source = read("scripts/system-window-policy-browser-smoke.mjs");
  assert.equal((source.match(/await test\(/g) || []).length, 4);
  assert(!/SYSTEM_WINDOW_TRACE|systemWindowDiagnostic|systemWindowTrace/.test(source));
  assert(source.includes("System bootstrap must render verified recovery state"));
  assert(source.includes("fixture finishes without page errors"));
});
