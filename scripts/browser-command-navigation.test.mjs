import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = fs.readFileSync(new URL("./browser-selkies-control-service.mjs", import.meta.url), "utf8");
const commandSource = source.slice(
  source.indexOf("async function applyBrowserCommand("),
  source.indexOf("\nfunction isRecoverableCommandNavigationFailure("),
);

function commandHarness({ navigationError = null } = {}) {
  let pendingDomEvent = false;
  let elapsedMs = 0;
  const calls = [];
  const state = { url: "https://example.test/next", title: "Next" };
  const cdp = {
    async request(method) {
      calls.push(method);
      if (method === "Page.getNavigationHistory") {
        return { currentIndex: 1, entries: [{ id: 1 }, { id: 2 }, { id: 3 }] };
      }
      if (["Page.reload", "Page.navigateToHistoryEntry"].includes(method)) pendingDomEvent = true;
      return {};
    },
    async waitForEvent(method, timeoutMs) {
      calls.push(method);
      if (pendingDomEvent) {
        pendingDomEvent = false;
        return {};
      }
      elapsedMs += timeoutMs;
      throw new Error("event timed out");
    },
  };
  const context = vm.createContext({
    validateBrowserNavigationUrl: (url) => new URL(url).href,
    withBrowserCdp: async (_page, _timeout, action) => action(cdp),
    ensureBrowserFileChooserInterception: async () => {},
    installWalletBridge: async () => null,
    normalizeWalletBridge: (wallet) => wallet,
    navigateInitialBrowserPage: async () => {
      if (navigationError) throw navigationError;
      // The navigation observer consumes the document event before returning.
      pendingDomEvent = true;
      await cdp.waitForEvent("Page.domContentEventFired", 15000);
      return {};
    },
    assertBrowserNavigationSucceeded: () => {},
    projectAndLogRuntimeProxyOnlineState: async () => {},
    refreshBrowserPageState: async () => state,
    assertBrowserStateDidNotLandOnErrorPage: () => {},
    isRecoverableCommandNavigationFailure: () => false,
  });
  vm.runInContext(`${commandSource}\nthis.runCommand = applyBrowserCommand;`, context);
  const page = { debugger_url: "ws://example.test/devtools/page/1" };
  return {
    page, calls, state,
    elapsedMs: () => elapsedMs,
    run: (command) => context.runCommand(
      { browserControl: { timeoutMs: 20000 }, runtimeFetchProxyUrl: "http://runtime.test" },
      page, { command, url: state.url }, 20000,
    ),
  };
}

test("completed navigation returns page state without an extra event timeout", async () => {
  const harness = commandHarness();
  assert.equal(await harness.run("navigate"), harness.state);
  assert.equal(harness.elapsedMs(), 0, "a settled page must return without waiting for another DOM event");
  assert.equal(harness.page.navigationInProgress, false);
});

for (const command of ["reload", "back", "forward"]) {
  test(`${command} still consumes its own navigation event`, async () => {
    const harness = commandHarness();
    assert.equal(await harness.run(command), harness.state);
    assert.equal(harness.elapsedMs(), 0);
    assert.equal(harness.calls.filter((method) => method === "Page.domContentEventFired").length, 1);
  });
}

test("terminal navigation failure preserves the error and clears in-progress state", async () => {
  const error = new Error("destination policy denied");
  const harness = commandHarness({ navigationError: error });
  await assert.rejects(harness.run("navigate"), (actual) => actual === error);
  assert.equal(harness.elapsedMs(), 0);
  assert.equal(harness.page.navigationInProgress, false);
});
