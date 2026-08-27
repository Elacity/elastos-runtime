#!/usr/bin/env node

const moduleVersion = "home-20260805a";
let assertions = 0;

function assert(condition, message, details = null) {
  assertions += 1;
  if (!condition) {
    const error = new Error(message);
    error.details = details;
    throw error;
  }
}

const hostContext = await import(
  `../capsules/home/browser/home-browser-context.js?v=${moduleVersion}`
);
const expectedContext = "browser:000102030405060708090a0b0c0d0e0f";
const replacementContext = "browser:101112131415161718191a1b1c1d1e1f";
const stored = new Map();
let cryptoCalls = 0;
const cryptoSource = {
  getRandomValues(bytes) {
    cryptoCalls += 1;
    bytes.forEach((_value, index) => {
      bytes[index] = index;
    });
    return bytes;
  },
};
const storage = {
  getItem: (key) => stored.get(key) || null,
  setItem: (key, value) => stored.set(key, String(value)),
};

assert(
  hostContext.createHomeBrowserContextId(cryptoSource) === expectedContext,
  "host context creation must use exactly 128 bits of browser crypto",
);
assert(
  hostContext.isHomeBrowserContextId(expectedContext),
  "generated host context must satisfy the exact bounded shape",
);
for (const invalid of [
  "",
  "browser:",
  "browser:00010203",
  `browser:${"0".repeat(31)}`,
  `browser:${"0".repeat(33)}`,
  `browser:${"g".repeat(32)}`,
  ` browser:${"0".repeat(32)}`,
  `browser:${"0".repeat(32)} `,
  null,
]) {
  assert(
    !hostContext.isHomeBrowserContextId(invalid),
    "host context validator accepted a malformed or unbounded value",
    invalid,
  );
}

stored.set(hostContext.HOME_BROWSER_CONTEXT_STORAGE_KEY, "browser:legacy");
assert(
  hostContext.loadOrCreateHomeBrowserContextId(storage, cryptoSource) === expectedContext,
  "host must replace a stale-shape profile correlation with browser crypto",
);
const cryptoCallsAfterCreate = cryptoCalls;
assert(
  hostContext.loadOrCreateHomeBrowserContextId(storage, {
    getRandomValues() {
      throw new Error("stored context should not regenerate");
    },
  }) === expectedContext,
  "same top-level browser profile must retain its stored correlation",
);
assert(
  cryptoCalls === cryptoCallsAfterCreate,
  "stored host correlation unexpectedly consumed fresh randomness",
);
assert(
  hostContext.loadOrCreateHomeBrowserContextId(
    {
      getItem: () => null,
      setItem() {},
    },
    cryptoSource,
  ) === "",
  "host must not hand off a correlation that was not durably stored",
);
assert(
  hostContext.loadOrCreateHomeBrowserContextId(null, cryptoSource) === "",
  "host must fail closed when browser-profile storage is unavailable",
);
assert(
  hostContext.loadOrCreateHomeBrowserContextId(storage, null) === expectedContext,
  "valid durable host correlation must not require regeneration",
);

let guiStorageReads = 0;
const requests = [];
globalThis.document = {
  querySelector: () => null,
};
globalThis.window = {
  get localStorage() {
    guiStorageReads += 1;
    throw new Error("opaque Home GUI must not access localStorage");
  },
};
globalThis.fetch = async (url, init = {}) => {
  requests.push({
    url: String(url),
    body: init.body ? JSON.parse(init.body) : null,
  });
  return {
    ok: true,
    status: 200,
    statusText: "OK",
    json: async () => ({ ok: true }),
    text: async () => "",
  };
};

const guiCore = await import(
  `../capsules/home-gui/browser/shell-core.js?v=${moduleVersion}`
);
assert(guiStorageReads === 0, "Home GUI attempted to own browser-profile storage");
assert(
  guiCore.shellState.browserContextId === "",
  "new opaque Home GUI frame must begin without a browser correlation",
);
guiCore.shellState.homeBrowserState = {
  principalId: "principal:context-smoke",
  layout: null,
  session: {
    browser_context_id: expectedContext,
    root_shell: "home-gui",
    windows: [{ target: "browser" }],
  },
  recentTargets: [],
};
const requestCountBeforeContext = requests.length;
assert(
  guiCore.loadShellSessionState() === null,
  "Home GUI restored a session before accepting the host correlation",
);
assert(
  guiCore.saveShellSessionState({ root_shell: "home-gui", windows: [] }) === false,
  "Home GUI persisted a session before accepting the host correlation",
);
assert(
  requests.length === requestCountBeforeContext,
  "context-free Home GUI session persistence reached Runtime",
  requests,
);
assert(
  guiCore.shellState.homeBrowserState.session.browser_context_id === expectedContext &&
    guiCore.shellState.homeBrowserState.session.windows[0].target === "browser",
  "context-free Home GUI session persistence mutated protected state in memory",
  guiCore.shellState.homeBrowserState.session,
);
assert(
  !guiCore.acceptHomeBrowserContextId("browser:legacy"),
  "Home GUI accepted a stale-shape host correlation",
);
assert(
  guiCore.acceptHomeBrowserContextId(expectedContext),
  "Home GUI rejected the exact host correlation",
);
assert(
  guiCore.loadShellSessionState()?.windows?.[0]?.target === "browser",
  "Home GUI did not restore the session bound to its accepted host correlation",
);
assert(
  !guiCore.acceptHomeBrowserContextId(replacementContext),
  "Home GUI allowed its correlation to change after the frame was bound",
);
guiCore.shellState.homeBrowserState.session = {
  browser_context_id: replacementContext,
  root_shell: "home-gui",
  windows: [{ target: "system" }],
};
assert(
  guiCore.loadShellSessionState() === null,
  "another top-level host correlation restored this GUI session",
);
guiCore.setHomeGuiLaunchToken("context-smoke-token");
assert(
  guiCore.saveShellSessionState({
    root_shell: "home-gui",
    windows: [{ target: "browser" }],
  }),
  "Home GUI refused a session after accepting the host correlation",
);
assert(
  requests.at(-1)?.body?.session?.browser_context_id === expectedContext,
  "Home GUI did not bind the saved session to the accepted host correlation",
  requests,
);
assert(guiStorageReads === 0, "Home GUI accessed localStorage after host binding");
assert(assertions > 0, "Home browser-context smoke did not execute assertions");

console.log(
  `[home-browser-context] PASS assertions=${assertions} context=${expectedContext}`,
);
