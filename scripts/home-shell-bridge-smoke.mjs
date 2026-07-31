#!/usr/bin/env node

const moduleVersion = "home-20260725a";
const requests = [];
const injectedProviderCalls = [];
let extraWindowOpenCount = 0;
const windowListeners = new Map();
const localStorageValues = new Map([
  ["elastos.home.active-shell-hint", "home-cli"],
]);

function addWindowEventListener(type, callback) {
  if (!windowListeners.has(type)) {
    windowListeners.set(type, []);
  }
  windowListeners.get(type).push(callback);
}

function removeWindowEventListener(type, callback) {
  const listeners = windowListeners.get(type) || [];
  windowListeners.set(type, listeners.filter((listener) => listener !== callback));
}

function dispatchWindowEvent(event) {
  for (const listener of windowListeners.get(event?.type) || []) {
    listener(event);
  }
  return true;
}

class FakeClassList {
  constructor() {
    this.values = new Set();
  }

  add(...tokens) {
    for (const token of tokens) this.values.add(token);
  }

  remove(...tokens) {
    for (const token of tokens) this.values.delete(token);
  }

  contains(token) {
    return this.values.has(token);
  }

  toggle(token, force) {
    const next = force === undefined ? !this.values.has(token) : Boolean(force);
    if (next) this.values.add(token);
    else this.values.delete(token);
    return next;
  }
}

class FakeStyle {
  constructor() {
    this.values = new Map();
  }

  removeProperty(name) {
    this.values.delete(name);
    delete this[name];
  }

  setProperty(name, value) {
    this.values.set(name, String(value));
    this[name] = String(value);
  }
}

class FakeElement {
  constructor(selector = "", withTemplateContent = true) {
    this.selector = selector;
    this.children = [];
    this.dataset = {};
    this.style = new FakeStyle();
    this.hidden = false;
    this.disabled = false;
    this.inert = false;
    this.removed = false;
    this.textContent = "";
    this.title = "";
    this.value = "";
    this.src = "";
    this.attributes = new Map();
    this.listeners = new Map();
    this.queries = new Map();
    this.classList = new FakeClassList();
    this.content = withTemplateContent
      ? {
          firstElementChild: new FakeElement(`:template-child`, false),
          cloneNode: () => new FakeElement(`:template-fragment`, false),
        }
      : { firstElementChild: null };
  }

  addEventListener(type, callback) {
    if (!this.listeners.has(type)) {
      this.listeners.set(type, []);
    }
    this.listeners.get(type).push(callback);
  }

  appendChild(child) {
    child.parentElement = this;
    this.children.push(child);
    return child;
  }

  insertBefore(child, before) {
    child.parentElement = this;
    const index = this.children.indexOf(before);
    if (index >= 0) this.children.splice(index, 0, child);
    else this.children.push(child);
    return child;
  }

  cloneNode() {
    return new FakeElement(`${this.selector}:clone`);
  }

  closest() {
    return null;
  }

  focus() {}

  getAttribute(name) {
    return this.attributes.get(name) || "";
  }

  getBoundingClientRect() {
    return { left: 0, top: 0, width: 1024, height: 768, right: 1024, bottom: 768 };
  }

  querySelector(selector) {
    if (!this.queries.has(selector)) {
      this.queries.set(selector, new FakeElement(`${this.selector} ${selector}`));
    }
    return this.queries.get(selector);
  }

  querySelectorAll() {
    return [];
  }

  remove() {
    this.removed = true;
    if (this.parentElement) {
      this.parentElement.children = this.parentElement.children.filter((child) => child !== this);
      this.parentElement = null;
    }
  }

  removeAttribute(name) {
    this.attributes.delete(name);
    delete this[name];
  }

  replaceChildren(...children) {
    this.children = children;
  }

  setAttribute(name, value) {
    this.attributes.set(name, String(value));
    this[name] = String(value);
  }
}

const elementCache = new Map();
function elementForSelector(selector) {
  if (!elementCache.has(selector)) {
    elementCache.set(selector, new FakeElement(selector));
  }
  return elementCache.get(selector);
}

function jsonResponse(value) {
  return {
    ok: true,
    status: 200,
    statusText: "OK",
    json: async () => value,
    text: async () => JSON.stringify(value),
  };
}

function assert(condition, message, details = null) {
  if (!condition) {
    const error = new Error(message);
    error.details = details;
    throw error;
  }
}

const summary = {
  authority: { signed_in: true },
  active_shell: {
    active: "home-cli",
    candidates: [
      { name: "home-gui", title: "Home GUI", role: "shell", launchable: true, route: "/apps/home/" },
      { name: "home-cli", title: "Home CLI", role: "shell", launchable: true, route: "/apps/home-cli/" },
    ],
  },
  app: { id: "home", route: "/apps/home/" },
  appearance: {},
  browser_state: {
    principal_id: "principal:home-shell-bridge",
    layout: { desktop: {}, taskbar: [], desktopHidden: [], desktopIconsVisible: true },
    recent_targets: [],
    session: { windows: [] },
  },
  desktop_objects: { objects: [] },
  identity: {},
  notifications: {},
  people: {},
  runtime: { running: true },
  services: {},
  site: {},
  targets: [
    { target: "browser", title: "Browser", attach_kind: "iframe", role: "app", target_kind: "app" },
    { target: "inbox", title: "Inbox", attach_kind: "iframe", role: "app", target_kind: "app" },
    { target: "system", title: "System", attach_kind: "iframe", role: "app", target_kind: "app" },
    { target: "wallet", title: "Wallet", attach_kind: "iframe", role: "app", target_kind: "app" },
  ],
};
let activeShellName = "home-cli";
let passkeyCompleted = false;
const braveAddress = "0x2222222222222222222222222222222222222222";
const metamaskAddress = "0x1111111111111111111111111111111111111111";
const bitcoinAddress = "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l";
const payloadHash = `0x${"bb".repeat(32)}`;

function evmProvider(label, address) {
  let chainId = "0x14";
  return {
    async request(request) {
      injectedProviderCalls.push({ label, ...request });
      switch (request?.method) {
        case "eth_requestAccounts":
        case "eth_accounts":
          return [address];
        case "eth_chainId":
          return chainId;
        case "personal_sign":
        case "eth_signTypedData_v4":
          return `0x${"ab".repeat(65)}`;
        case "eth_sendTransaction":
          return `0x${"cd".repeat(32)}`;
        case "wallet_switchEthereumChain":
          chainId = request.params?.[0]?.chainId || chainId;
          return null;
        case "wallet_addEthereumChain":
          chainId = request.params?.[0]?.chainId || chainId;
          return null;
        default:
          throw new Error(`unexpected ${label} provider method: ${request?.method || "missing"}`);
      }
    },
  };
}

const braveProvider = evmProvider("brave", braveAddress);
const metamaskProvider = evmProvider("metamask", metamaskAddress);
const unisatProvider = {
  async requestAccounts() {
    injectedProviderCalls.push({ label: "unisat", method: "requestAccounts" });
    return [bitcoinAddress];
  },
  async getAccounts() {
    injectedProviderCalls.push({ label: "unisat", method: "getAccounts" });
    return [bitcoinAddress];
  },
  async getNetwork() {
    injectedProviderCalls.push({ label: "unisat", method: "getNetwork" });
    return "livenet";
  },
  async signMessage(message, signatureType) {
    injectedProviderCalls.push({ label: "unisat", method: "signMessage", message, signatureType });
    return "mock-bip322-signature";
  },
};

function currentSummary() {
  return {
    ...summary,
    active_shell: {
      ...summary.active_shell,
      active: activeShellName,
    },
  };
}

globalThis.HTMLElement = FakeElement;
globalThis.document = {
  activeElement: null,
  body: elementForSelector("body"),
  documentElement: elementForSelector("html"),
  addEventListener() {},
  createElement: (tag) => new FakeElement(tag),
  querySelector: elementForSelector,
  querySelectorAll: () => [],
};
Object.defineProperty(globalThis, "navigator", {
  configurable: true,
  value: {
    credentials: {
      get: async () => ({
        id: "credential-id",
        rawId: new Uint8Array([1]).buffer,
        type: "public-key",
        response: {
          authenticatorData: new Uint8Array([2]).buffer,
          clientDataJSON: new Uint8Array([3]).buffer,
          signature: new Uint8Array([4]).buffer,
          userHandle: null,
        },
      }),
    },
  },
});
globalThis.window = {
  PublicKeyCredential: function PublicKeyCredential() {},
  unisat: unisatProvider,
  atob: globalThis.atob,
  btoa: globalThis.btoa,
  crypto: {
    randomUUID: () => "home-shell-bridge-smoke",
    getRandomValues(bytes) {
      bytes.forEach((_value, index) => {
        bytes[index] = 160 + index;
      });
      return bytes;
    },
  },
  location: { href: "http://localhost:61180/apps/home/", origin: "http://localhost:61180" },
  localStorage: {
    getItem: (key) => localStorageValues.get(key) || null,
    removeItem: (key) => localStorageValues.delete(key),
    setItem: (key, value) => localStorageValues.set(key, String(value)),
  },
  performance: { now: () => Date.now() },
  innerWidth: 1280,
  addEventListener: addWindowEventListener,
  removeEventListener: removeWindowEventListener,
  dispatchEvent: dispatchWindowEvent,
  clearInterval() {},
  clearTimeout() {},
  setInterval: () => 0,
  setTimeout: () => 0,
  open() {
    extraWindowOpenCount += 1;
    throw new Error("Home wallet connector must not open another window");
  },
};

elementForSelector("#active-shell-root").hidden = true;
elementForSelector("#active-shell-frame").hidden = true;
elementForSelector("#shell-host-recovery").hidden = true;
elementForSelector("#shell-host-recovery-detail").hidden = true;
elementForSelector("#desktop-context-menu").hidden = true;
elementForSelector("#home-notification-toast").hidden = true;
elementForSelector("#home-unlock").hidden = true;
elementForSelector("#launcher").hidden = true;

globalThis.fetch = async (url, init = {}) => {
  const body = init.body ? JSON.parse(init.body) : null;
  requests.push({
    body,
    headers: init.headers || {},
    method: init.method || "GET",
    url: String(url),
  });
  if (url === "/api/apps/home/summary") {
    if (requests.filter((request) => request.url === "/api/apps/home/summary").length === 1) {
      assert(
        document.documentElement.dataset.homeShellBoot === "alternate",
        "alternate shell boot hint was not applied before the summary request",
        document.documentElement.dataset,
      );
      assert(
        document.body.dataset.homeShell === "alternate" && document.body.dataset.homeGui === "dormant",
        "alternate shell did not claim the first paint before the summary request",
        document.body.dataset,
      );
      assert(
        elementForSelector("#active-shell-root").hidden === false &&
          elementForSelector("#active-shell-root").dataset.target === "home-cli",
        "alternate shell root was not visible before the summary request",
        elementForSelector("#active-shell-root").dataset,
      );
      assert(
        elementForSelector("#active-shell-frame").hidden === true &&
          !elementForSelector("#active-shell-frame").getAttribute("src"),
        "alternate shell boot hint loaded a frame before Runtime launch authority",
        elementForSelector("#active-shell-frame").dataset,
      );
    }
    return jsonResponse(currentSummary());
  }
  if (url === "/api/auth/sessions/refresh") {
    return jsonResponse({ home_token: "host-token" });
  }
  if (url === "/api/auth/passkey-step-up/begin") {
    return jsonResponse({
      schema: "elastos.auth.passkey-step-up.begin.result/v1",
      ceremony_id: "ceremony-id",
      options: { publicKey: { challenge: "AQ", allowCredentials: [] } },
    });
  }
  if (url === "/api/auth/passkey-step-up/complete") {
    passkeyCompleted = true;
    return jsonResponse({
      schema: "elastos.auth.passkey-step-up.complete.result/v1",
      step_up_token: "system-step-up-token",
    });
  }
  if (url === "/api/auth/passkey-step-up/cancel") {
    return jsonResponse({
      schema: "elastos.auth.passkey-step-up.cancel.result/v1",
      ceremony_id: body?.ceremony_id,
      status: "cancelled",
    });
  }
  if (url === "/api/apps/home/active-shell") {
    assert(body?.active === "home-gui", "root shell app-open must switch back to home-gui", body);
    assert(
      init.headers?.["x-elastos-home-token"] === "root-token",
      "home-gui switchback did not use the shell launch token",
      init.headers,
    );
    activeShellName = "home-gui";
    return jsonResponse({ active: "home-gui" });
  }
  if (url === "/api/apps/home/launch") {
    assert(
      init.headers?.["x-elastos-home-token"] === "host-token",
      "Home launch did not use explicit host-held authority",
      init.headers,
    );
    assert(!Object.hasOwn(body || {}, "authority"), "Home launch carried removed intent authority", body);
    if (body?.target === "home-cli") {
      assert(body?.query?.shell_mode === "root", "alternate shell must launch in root mode", body);
      return jsonResponse({
        attach_kind: "iframe",
        route: "/apps/home-cli/?shell_mode=root&home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=root-token",
        target: "home-cli",
      });
    }
    if (body?.target === "home-gui") {
      assert(body?.query?.shell_mode === "root", "GUI shell must launch in root mode", body);
      return jsonResponse({
        attach_kind: "iframe",
        route: "/apps/home-gui/?shell_mode=root&home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=gui-token",
        target: "home-gui",
      });
    }
    if (body?.target === "browser") {
      return jsonResponse({
        attach_kind: "iframe",
        route: "/apps/browser/?home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=browser-token",
        target: "browser",
        title: "Browser",
      });
    }
    if (body?.target === "system") {
      return jsonResponse({
        attach_kind: "iframe",
        route: "/apps/system/?home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=system-token",
        target: "system",
        title: "System",
      });
    }
    if (body?.target === "wallet") {
      return jsonResponse({
        attach_kind: "iframe",
        route: "/apps/wallet/?home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=wallet-token",
        target: "wallet",
        title: "Wallet",
      });
    }
    if (body?.target === "wallet-metamask") {
      return jsonResponse({
        attach_kind: "iframe",
        route: "/apps/wallet-metamask/?home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=metamask-token",
        target: "wallet-metamask",
        title: "MetaMask",
      });
    }
    if (body?.target === "wallet-unisat") {
      return jsonResponse({
        attach_kind: "iframe",
        route: "/apps/wallet-unisat/?home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=unisat-token",
        target: "wallet-unisat",
        title: "UniSat",
      });
    }
    throw new Error(`unexpected launch target: ${body?.target || "missing"}`);
  }
  if (url === "/api/apps/home/wallet-connector/evm/link/challenge") {
    assert(body?.connector_id === "wallet-metamask", "EVM bridge changed connector", body);
    assert(
      body?.connector_token === "metamask-token",
      "EVM bridge did not carry the connector launch token",
      body,
    );
    return jsonResponse({
      schema: "elastos.home.wallet-connector.effect.result/v1",
      action: "evm_link_challenge",
      connector_id: "wallet-metamask",
      challenge: {
        schema: "elastos.auth.challenge/v1",
        challenge_id: "challenge:evm",
        message: "ElastOS EVM link challenge\nRuntime-issued",
        expires_at: 4_000_000_000,
        resources: ["elastos://wallet/link"],
      },
    });
  }
  if (url === "/api/apps/home/wallet-connector/evm/link/verify") {
    return jsonResponse({
      schema: "elastos.home.wallet-connector.effect.result/v1",
      action: "evm_link_complete",
      connector_id: "wallet-metamask",
      status: "linked",
    });
  }
  if (url === "/api/apps/home/wallet-connector/bitcoin/link/challenge") {
    assert(
      body?.connector_token === "unisat-token",
      "Bitcoin bridge did not carry the connector launch token",
      body,
    );
    return jsonResponse({
      schema: "elastos.home.wallet-connector.effect.result/v1",
      action: "bitcoin_link_challenge",
      connector_id: "wallet-unisat",
      challenge: {
        schema: "elastos.wallet.bitcoin_challenge/v1",
        challenge_id: "challenge:bitcoin",
        message: "ElastOS Bitcoin link challenge\nRuntime-issued",
        expires_at: 4_000_000_000,
        network: "bitcoin",
        address: bitcoinAddress,
        resources: ["elastos://wallet/link"],
        proof_type: "bip322_simple",
      },
    });
  }
  if (url === "/api/apps/home/wallet-connector/bitcoin/link/verify") {
    return jsonResponse({
      schema: "elastos.home.wallet-connector.effect.result/v1",
      action: "bitcoin_link_complete",
      connector_id: "wallet-unisat",
      status: "linked",
    });
  }
  if (
    String(url).startsWith("/api/apps/home/wallet-connector/approvals/")
    && String(url).endsWith("/handoff")
  ) {
    const requestId = decodeURIComponent(String(url).split("/approvals/")[1].split("/")[0]);
    const connectorId = body?.connector_id;
    const common = {
      schema: "elastos.wallet.webconnect_handoff/v1",
      request_id: requestId,
      payload_hash: payloadHash,
      signer: connectorId === "wallet-unisat" ? bitcoinAddress : metamaskAddress,
    };
    let handoff;
    if (requestId === "approval:transaction") {
      handoff = {
        ...common,
        intent: "transaction_intent",
        status: "awaiting_wallet_transaction",
        transaction: {
          from: metamaskAddress,
          to: "0x3333333333333333333333333333333333333333",
          value: "0x1",
          data: "0x",
          gas: "0x5208",
          gasPrice: "0x3b9aca00",
          nonce: "0x1",
          chainId: "0x14",
        },
      };
    } else {
      handoff = {
        ...common,
        intent: requestId === "approval:typed"
          ? "browser_typed_data_sign"
          : requestId === "approval:bitcoin"
            ? "bitcoin_bip322_proof"
            : "publish_envelope",
        message: requestId === "approval:typed"
          ? '{"domain":{"name":"ElastOS"},"message":{"request":"typed"}}'
          : `ElastOS approval\n${requestId}`,
        signature_type: requestId === "approval:bitcoin" ? "bip322_simple" : "personal_sign",
        status: "awaiting_wallet_signature",
      };
    }
    return jsonResponse({
      schema: "elastos.home.wallet-connector.effect.result/v1",
      action: "approval_handoff",
      connector_id: connectorId,
      request_id: requestId,
      handoff,
      evm_chains: [{
        chainId: "0x14",
        chainName: "Elastos Smart Chain",
        nativeCurrency: { name: "ELA", symbol: "ELA", decimals: 18 },
        rpcUrls: ["https://api.elastos.io/esc"],
      }],
    });
  }
  if (
    String(url).startsWith("/api/apps/home/wallet-connector/approvals/")
    && String(url).endsWith("/complete")
  ) {
    const requestId = decodeURIComponent(String(url).split("/approvals/")[1].split("/")[0]);
    return jsonResponse({
      schema: "elastos.home.wallet-connector.effect.result/v1",
      action: "approval_complete",
      connector_id: body?.connector_id,
      request_id: requestId,
      status: "completed",
    });
  }
  return jsonResponse({ ok: true });
};

const hostCore = await import(`../capsules/home/browser/shell-core.js?v=${moduleVersion}`);

await import(`../capsules/home/browser/home-shell-host.js?v=${moduleVersion}`);
for (let attempt = 0; attempt < 20 && !elementForSelector("#active-shell-frame").dataset.route; attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

const body = document.body;
const activeShellRoot = elementForSelector("#active-shell-root");
const activeShellFrame = elementForSelector("#active-shell-frame");
const shellHostRecovery = elementForSelector("#shell-host-recovery");
const launchRequest = requests.find((request) => request.url === "/api/apps/home/launch");

assert(body.dataset.homeShell === "alternate", "alternate shell did not take root mode", body.dataset);
assert(body.dataset.homeGui === "dormant", "Home GUI was not marked dormant", body.dataset);
assert(
  document.documentElement.dataset.homeShellHint === "alternate",
  "alternate shell boot hint was not applied before summary",
  document.documentElement.dataset,
);
assert(
  localStorageValues.get("elastos.home.active-shell-hint") === "home-cli",
  "alternate shell hint was not refreshed from Runtime summary",
  Object.fromEntries(localStorageValues),
);
assert(activeShellRoot.hidden === false, "active shell root stayed hidden");
assert(activeShellRoot.dataset.target === "home-cli", "active shell root target drifted", activeShellRoot.dataset);
assert(activeShellFrame.hidden === false, "active shell frame stayed hidden");
assert(activeShellFrame.dataset.route.includes("/apps/home-cli/"), "active shell frame did not load Home CLI", activeShellFrame.dataset);
assert(activeShellFrame.dataset.route.includes("#home_token=root-token"), "active shell launch token was not carried in the frame fragment", activeShellFrame.dataset);
assert(!activeShellFrame.dataset.route.includes("?home_token="), "active shell authority leaked into the query string", activeShellFrame.dataset);
assert(shellHostRecovery.hidden === true, "host recovery panel showed during healthy shell launch");
assert(launchRequest, "alternate shell launch request was not made", requests);
assert(!requests.some((request) => request.url === "/api/apps/home/active-shell"), "bridge smoke should not switch shell using ambient state", requests);

const shellMessages = [];
const shellFrameWindow = {
  postMessage(payload, origin) {
    shellMessages.push({ origin, payload });
  },
};
activeShellFrame.contentWindow = shellFrameWindow;
const launchRequestsBeforeInvalidMessages = requests.filter(
  (request) => request.url === "/api/apps/home/launch",
).length;
for (const data of [
  {
    origin: "http://evil.invalid",
    type: "home:open-target",
    target: "browser",
    homeToken: "root-token",
  },
  {
    origin: "null",
    type: "home:open-target",
    target: "browser",
    homeToken: "wrong-token",
  },
  {
    origin: "null",
    type: "home:open-target",
    target: "home",
    homeToken: "root-token",
  },
  {
    origin: "null",
    type: "home:sign-out",
    homeToken: "wrong-token",
  },
]) {
  for (const listener of windowListeners.get("message") || []) {
    listener({
      origin: data.origin,
      source: shellFrameWindow,
      data,
    });
  }
}
for (let attempt = 0; attempt < 5; attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
assert(
  requests.filter((request) => request.url === "/api/apps/home/launch").length ===
    launchRequestsBeforeInvalidMessages,
  "unauthorized shell messages launched a target",
  requests,
);
assert(
  !requests.some((request) => request.url === "/api/apps/home/active-shell"),
  "unauthorized shell messages switched active shell",
  requests,
);
assert(
  !requests.some((request) => request.url === "/api/auth/sessions/sign-out"),
  "unauthorized shell message signed out Home",
  requests,
);
for (const listener of windowListeners.get("message") || []) {
  listener({
    origin: "null",
    source: shellFrameWindow,
    data: {
      type: "home:launch-target",
      requestId: "cli-launch-browser",
      target: "browser",
      query: {},
      homeToken: "root-token",
    },
  });
}
for (
  let attempt = 0;
  attempt < 20 && !shellMessages.some((message) => message.payload?.requestId === "cli-launch-browser");
  attempt += 1
) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
assert(
  shellMessages.some((message) =>
    message.origin === "*" &&
    message.payload?.type === "home:shell-response" &&
    message.payload?.requestId === "cli-launch-browser" &&
    message.payload?.result?.route?.includes("#home_token=browser-token")
  ),
  "Home CLI did not receive the same Runtime-scoped launch result as Home GUI",
  shellMessages,
);
assert(
  !requests.some((request) => request.url === "/api/apps/home/active-shell"),
  "Home CLI launch request switched shells implicitly",
  requests,
);
for (const listener of windowListeners.get("message") || []) {
  listener({
    origin: "null",
    source: shellFrameWindow,
    data: {
      type: "home:open-target",
      target: "browser",
      homeToken: "root-token",
    },
  });
}
await new Promise((resolve) => setTimeout(resolve, 0));
assert(
  !requests.some((request) => request.url === "/api/apps/home/active-shell"),
  "ordinary CLI app intent switched Home GUI implicitly",
  requests,
);
for (const listener of windowListeners.get("message") || []) {
  listener({
    origin: "null",
    source: shellFrameWindow,
    data: {
      type: "home:switch-shell-and-open-target",
      requestId: "explicit-gui-browser",
      target: "browser",
      homeToken: "root-token",
    },
  });
}
for (let attempt = 0; attempt < 20 && activeShellFrame.dataset.route.includes("home-cli"); attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
assert(body.dataset.homeShell === "desktop", "explicit graphical action did not switch to desktop shell mode", body.dataset);
assert(body.dataset.homeGui === "mounted", "explicit graphical action did not mount Home GUI", body.dataset);
assert(activeShellRoot.hidden === false, "explicit graphical action hid the active shell root");
assert(activeShellRoot.dataset.target === "home-gui", "explicit graphical action did not select isolated Home GUI", activeShellRoot.dataset);
assert(activeShellFrame.dataset.route.includes("/apps/home-gui/"), "explicit graphical action did not launch isolated Home GUI", activeShellFrame.dataset);
assert(activeShellFrame.dataset.route.includes("#home_token=gui-token"), "isolated Home GUI did not receive fragment authority", activeShellFrame.dataset);
assert(
  hostCore.shellState.activeShellRootTarget === "home-gui",
  "explicit graphical action did not replace the alternate root shell",
  hostCore.shellState,
);
assert(
  requests.some((request) => request.url === "/api/apps/home/active-shell"),
  "explicit graphical action did not switch active shell with the shell token",
  requests,
);
assert(
  shellMessages.some((message) => message.payload?.type === "home:gui-command" && message.payload?.target === "browser"),
  "explicit graphical action did not hand Browser intent to isolated Home GUI",
  shellMessages,
);

function sendChildMessage(origin, source, data) {
  for (const listener of windowListeners.get("message") || []) {
    listener({ origin, source, data });
  }
}

function announceProvider(rdns, uuid, provider) {
  dispatchWindowEvent({
    type: "eip6963:announceProvider",
    detail: {
      info: { rdns, uuid, name: rdns, icon: "data:image/svg+xml,<svg/>" },
      provider,
    },
  });
}

async function waitForReply(replies, requestId) {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    const reply = replies.find((entry) => entry.payload?.requestId === requestId);
    if (reply) {
      return reply;
    }
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
  throw new Error(`timed out waiting for wallet bridge reply ${requestId}`);
}

sendChildMessage("null", shellFrameWindow, {
  type: "home:launch-target",
  requestId: "launch-wallet",
  target: "wallet",
  query: {},
  homeToken: "gui-token",
});
for (
  let attempt = 0;
  attempt < 50 && !shellMessages.some((message) => message.payload?.requestId === "launch-wallet");
  attempt += 1
) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
assert(
  shellMessages.some((message) =>
    message.payload?.requestId === "launch-wallet" &&
    message.payload?.result?.route?.includes("#home_token=wallet-token")
  ),
  "Home GUI could not launch the visible Wallet capsule",
  shellMessages,
);
const walletFrameWindow = { postMessage() {} };
sendChildMessage("null", walletFrameWindow, {
  type: "home:app-ready",
  homeToken: "wallet-token",
});

const connectorLaunchesBeforeDeniedRequests = requests.filter(
  (request) =>
    request.url === "/api/apps/home/launch" &&
    ["wallet-metamask", "wallet-unisat"].includes(request.body?.target),
).length;
sendChildMessage("null", shellFrameWindow, {
  type: "home:launch-target",
  requestId: "direct-gui-metamask",
  target: "wallet-metamask",
  query: {},
  homeToken: "gui-token",
});
sendChildMessage("http://evil.invalid", walletFrameWindow, {
  type: "home:open-target",
  target: "wallet-metamask",
  homeToken: "wallet-token",
});
sendChildMessage("null", walletFrameWindow, {
  type: "home:open-target",
  target: "wallet-walletconnect",
  homeToken: "wallet-token",
});
sendChildMessage("null", walletFrameWindow, {
  type: "home:open-target",
  target: "wallet-metamask",
  homeToken: "wallet-token",
  query: {},
});
await new Promise((resolve) => setTimeout(resolve, 0));
assert(
  requests.filter(
    (request) =>
      request.url === "/api/apps/home/launch" &&
      ["wallet-metamask", "wallet-unisat"].includes(request.body?.target),
  ).length === connectorLaunchesBeforeDeniedRequests,
  "forged, substituted, or direct Home GUI connector intent reached Runtime launch",
  requests,
);
assert(
  shellMessages.some((message) =>
    message.payload?.type === "home:shell-response" &&
    message.payload?.requestId === "direct-gui-metamask" &&
    message.payload?.error === "Home denied the shell launch"
  ),
  "Home GUI obtained hidden connector launch authority",
  shellMessages,
);

async function launchConnectorFromWallet(target, token) {
  const launchesBefore = requests.filter(
    (request) => request.url === "/api/apps/home/launch" && request.body?.target === target,
  ).length;
  const attachmentsBefore = shellMessages.filter((message) =>
    message.payload?.type === "home:gui-command" &&
    message.payload?.command === "attach-authorized-target" &&
    message.payload?.descriptor?.target === target
  ).length;
  const showDesktopCommandsBefore = shellMessages.filter((message) =>
    message.payload?.type === "home:gui-command" &&
    message.payload?.command === "show-desktop"
  ).length;
  sendChildMessage("null", walletFrameWindow, {
    type: "home:open-target",
    target,
    homeToken: "wallet-token",
  });
  let attachment = null;
  for (let attempt = 0; attempt < 50 && !attachment; attempt += 1) {
    attachment = shellMessages.find((message) =>
      message.payload?.type === "home:gui-command" &&
      message.payload?.command === "attach-authorized-target" &&
      message.payload?.descriptor?.target === target
    );
    if (!attachment) {
      await new Promise((resolve) => setTimeout(resolve, 0));
    }
  }
  assert(
    attachment?.origin === "*" &&
      attachment.payload?.descriptor?.schema ===
        "elastos.home.authorized-target-attachment/v1" &&
      attachment.payload?.descriptor?.route?.includes(`#home_token=${token}`),
    `Home did not produce the bounded ${target} attachment descriptor`,
    shellMessages,
  );
  assert(
    requests.filter(
      (request) => request.url === "/api/apps/home/launch" && request.body?.target === target,
    ).length === launchesBefore + 1,
    `Wallet ${target} intent did not make exactly one Runtime launch`,
    requests,
  );
  assert(
    shellMessages.filter((message) =>
      message.payload?.type === "home:gui-command" &&
      message.payload?.command === "attach-authorized-target" &&
      message.payload?.descriptor?.target === target
    ).length === attachmentsBefore + 1,
    `Wallet ${target} intent did not attach exactly one connector window`,
    shellMessages,
  );
  assert(
    shellMessages.filter((message) =>
      message.payload?.type === "home:gui-command" &&
      message.payload?.command === "show-desktop"
    ).length === showDesktopCommandsBefore,
    `Wallet ${target} intent hid the existing Home GUI windows`,
    shellMessages,
  );
  assert(
    !shellMessages.some((message) =>
      message.payload?.type === "home:shell-response" &&
      message.payload?.requestId === `launch-${target}`
    ),
    `Wallet ${target} launch was laundered through the generic Home GUI request path`,
    shellMessages,
  );
  const replies = [];
  const source = {
    postMessage(payload, origin) {
      replies.push({ payload, origin });
    },
  };
  sendChildMessage("null", source, { type: "home:app-ready", homeToken: token });
  return { source, replies };
}

const metamaskFrame = await launchConnectorFromWallet(
  "wallet-metamask",
  "metamask-token",
);
const unisatFrame = await launchConnectorFromWallet(
  "wallet-unisat",
  "unisat-token",
);

sendChildMessage("null", { postMessage() {} }, {
  type: "home:open-target",
  target: "wallet-metamask",
  homeToken: "wallet-token",
});
await new Promise((resolve) => setTimeout(resolve, 0));
assert(
  requests.filter(
    (request) =>
      request.url === "/api/apps/home/launch" &&
      request.body?.target === "wallet-metamask",
  ).length === 1,
  "a non-Wallet source replayed Wallet connector authority",
  requests,
);

function sendWalletEffect(frame, connectorId, connectorToken, requestId, action, extras = {}) {
  sendChildMessage(extras.origin || "null", extras.source || frame.source, {
    type: "home:wallet-connector-effect",
    schema: "elastos.home.wallet-connector-effect/v1",
    requestId,
    connectorId,
    connectorToken,
    action,
    ...(extras.fields || {}),
  });
}

announceProvider("com.brave.wallet", "brave-provider", braveProvider);
sendWalletEffect(
  metamaskFrame,
  "wallet-metamask",
  "metamask-token",
  "wallet-link-brave",
  { kind: "link" },
);
const braveReply = await waitForReply(metamaskFrame.replies, "wallet-link-brave");
assert(
  braveReply.origin === "*"
    && braveReply.payload?.result?.status === "linked"
    && injectedProviderCalls.some((call) =>
      call.label === "brave" && call.method === "personal_sign"
    ),
  "Home did not use the exact Brave fallback when MetaMask was absent",
  { braveReply, injectedProviderCalls },
);

announceProvider("io.metamask", "metamask-provider", metamaskProvider);
sendWalletEffect(
  metamaskFrame,
  "wallet-metamask",
  "metamask-token",
  "wallet-link-metamask",
  { kind: "link" },
);
const metamaskReply = await waitForReply(metamaskFrame.replies, "wallet-link-metamask");
assert(
  metamaskReply.payload?.result?.status === "linked"
    && injectedProviderCalls.some((call) =>
      call.label === "metamask" && call.method === "personal_sign"
    ),
  "Home did not prefer the exact EIP-6963 MetaMask provider",
  { metamaskReply, injectedProviderCalls },
);

sendWalletEffect(
  unisatFrame,
  "wallet-unisat",
  "unisat-token",
  "wallet-link-unisat",
  { kind: "link" },
);
const unisatLinkReply = await waitForReply(unisatFrame.replies, "wallet-link-unisat");
assert(
  unisatLinkReply.payload?.result?.status === "linked"
    && injectedProviderCalls.some((call) =>
      call.label === "unisat"
      && call.method === "signMessage"
      && call.signatureType === "bip322-simple"
    ),
  "Home did not complete the Runtime-issued UniSat linking challenge",
  { unisatLinkReply, injectedProviderCalls },
);

for (const [requestId, approvalRequestId, expectedMethod, expectedEffect] of [
  ["wallet-approve-personal", "approval:personal", "personal_sign", "signature"],
  ["wallet-approve-typed", "approval:typed", "eth_signTypedData_v4", "signature"],
  ["wallet-approve-transaction", "approval:transaction", "eth_sendTransaction", "transaction"],
]) {
  const callsBefore = injectedProviderCalls.length;
  sendWalletEffect(
    metamaskFrame,
    "wallet-metamask",
    "metamask-token",
    requestId,
    { kind: "approve", approvalRequestId },
  );
  const reply = await waitForReply(metamaskFrame.replies, requestId);
  assert(
    reply.payload?.result?.status === "completed"
      && reply.payload?.result?.effect === expectedEffect
      && injectedProviderCalls.slice(callsBefore).some((call) =>
        call.label === "metamask" && call.method === expectedMethod
      ),
    `Home did not execute the Runtime-implied ${expectedMethod} effect`,
    { reply, injectedProviderCalls: injectedProviderCalls.slice(callsBefore) },
  );
}

sendWalletEffect(
  unisatFrame,
  "wallet-unisat",
  "unisat-token",
  "wallet-approve-bitcoin",
  { kind: "approve", approvalRequestId: "approval:bitcoin" },
);
const bitcoinApprovalReply = await waitForReply(
  unisatFrame.replies,
  "wallet-approve-bitcoin",
);
assert(
  bitcoinApprovalReply.payload?.result?.status === "completed"
    && bitcoinApprovalReply.payload?.result?.effect === "signature",
  "Home did not complete the Runtime-implied UniSat approval effect",
  bitcoinApprovalReply,
);

const runtimeEffectsBeforeInvalid = requests.filter((request) =>
  request.url.includes("/api/apps/home/wallet-connector/")
).length;
const providerCallsBeforeInvalid = injectedProviderCalls.length;
const invalidMessages = [
  {
    requestId: "wallet-wrong-origin",
    extras: { origin: "https://evil.invalid" },
  },
  {
    requestId: "wallet-wrong-source",
    extras: { source: { postMessage() {} } },
  },
  {
    requestId: "wallet-wrong-token",
    token: "wrong-token",
  },
  {
    requestId: "wallet-nonexact-token",
    token: "metamask-token ",
  },
  {
    requestId: "wallet-wrong-connector",
    connectorId: "wallet-unisat",
  },
  {
    requestId: "wallet-nonexact-connector",
    connectorId: "wallet-metamask ",
  },
  {
    requestId: "bad request id",
  },
  {
    requestId: " wallet-trimmed-request",
  },
  {
    requestId: "wallet-nonexact-approval-id",
    action: {
      kind: "approve",
      approvalRequestId: " approval:personal",
    },
  },
  {
    requestId: "wallet-extra-field",
    extras: { fields: { method: "eth_sendTransaction" } },
  },
  {
    requestId: "wallet-arbitrary-method",
    action: {
      kind: "approve",
      approvalRequestId: "approval:personal",
      method: "eth_sendTransaction",
    },
  },
  {
    requestId: "wallet-arbitrary-message",
    action: {
      kind: "approve",
      approvalRequestId: "approval:personal",
      message: "attacker controlled",
    },
  },
  {
    requestId: "wallet-arbitrary-transaction",
    action: {
      kind: "approve",
      approvalRequestId: "approval:personal",
      transaction: { to: "attacker" },
    },
  },
  {
    requestId: "wallet-arbitrary-authority",
    extras: {
      fields: {
        principal: "attacker",
        session: "attacker",
        proofBinding: "attacker",
        grant: "attacker",
      },
    },
  },
];
for (const invalid of invalidMessages) {
  sendWalletEffect(
    metamaskFrame,
    invalid.connectorId || "wallet-metamask",
    invalid.token || "metamask-token",
    invalid.requestId,
    invalid.action || { kind: "link" },
    invalid.extras,
  );
}
await new Promise((resolve) => setTimeout(resolve, 0));
assert(
  requests.filter((request) =>
    request.url.includes("/api/apps/home/wallet-connector/")
  ).length === runtimeEffectsBeforeInvalid
    && injectedProviderCalls.length === providerCallsBeforeInvalid,
  "invalid Home wallet messages reached Runtime or an injected provider",
  { requests, injectedProviderCalls },
);

const replayRequestId = "wallet-replay-proof";
sendWalletEffect(
  metamaskFrame,
  "wallet-metamask",
  "metamask-token",
  replayRequestId,
  { kind: "approve", approvalRequestId: "approval:replay" },
);
await waitForReply(metamaskFrame.replies, replayRequestId);
const runtimeEffectsAfterFirstReplay = requests.filter((request) =>
  request.url.includes("/api/apps/home/wallet-connector/")
).length;
const providerCallsAfterFirstReplay = injectedProviderCalls.length;
sendWalletEffect(
  metamaskFrame,
  "wallet-metamask",
  "metamask-token",
  replayRequestId,
  { kind: "approve", approvalRequestId: "approval:replay" },
);
await new Promise((resolve) => setTimeout(resolve, 0));
assert(
  requests.filter((request) =>
    request.url.includes("/api/apps/home/wallet-connector/")
  ).length === runtimeEffectsAfterFirstReplay
    && injectedProviderCalls.length === providerCallsAfterFirstReplay,
  "replayed Home wallet request reached Runtime or an injected provider",
  { requests, injectedProviderCalls },
);

const runtimeEffectsBeforeSingleFlight = requests.filter((request) =>
  request.url.includes("/api/apps/home/wallet-connector/")
).length;
sendWalletEffect(
  metamaskFrame,
  "wallet-metamask",
  "metamask-token",
  "wallet-single-flight-first",
  { kind: "approve", approvalRequestId: "approval:single-flight" },
);
sendWalletEffect(
  metamaskFrame,
  "wallet-metamask",
  "metamask-token",
  "wallet-single-flight-second",
  { kind: "approve", approvalRequestId: "approval:single-flight" },
);
await waitForReply(metamaskFrame.replies, "wallet-single-flight-first");
await waitForReply(metamaskFrame.replies, "wallet-single-flight-second");
assert(
  requests.filter((request) =>
    request.url.includes("/api/apps/home/wallet-connector/")
  ).length === runtimeEffectsBeforeSingleFlight + 2,
  "Home wallet bridge did not enforce one in-flight approval lifecycle",
  requests,
);

announceProvider("com.brave.wallet", "metamask-conflicting-identity", metamaskProvider);
const runtimeEffectsBeforeAmbiguity = requests.filter((request) =>
  request.url.includes("/api/apps/home/wallet-connector/")
).length;
const providerCallsBeforeAmbiguity = injectedProviderCalls.length;
sendWalletEffect(
  metamaskFrame,
  "wallet-metamask",
  "metamask-token",
  "wallet-conflicting-provider",
  { kind: "link" },
);
const ambiguousReply = await waitForReply(
  metamaskFrame.replies,
  "wallet-conflicting-provider",
);
assert(
  String(ambiguousReply.payload?.error || "").includes("conflict")
    && requests.filter((request) =>
      request.url.includes("/api/apps/home/wallet-connector/")
    ).length === runtimeEffectsBeforeAmbiguity
    && injectedProviderCalls.length === providerCallsBeforeAmbiguity,
  "conflicting EIP-6963 identities did not fail before Runtime and provider effects",
  { ambiguousReply, requests, injectedProviderCalls },
);
assert(extraWindowOpenCount === 0, "wallet connector bridge opened an extra browser window");

const shellMessageCountBeforeReady = shellMessages.length;
sendChildMessage("http://localhost:61180", shellFrameWindow, {
  type: "home:shell-ready",
  homeToken: "gui-token",
});
sendChildMessage("null", shellFrameWindow, {
  type: "home:shell-ready",
  homeToken: "wrong-token",
});
sendChildMessage("null", { postMessage() {} }, {
  type: "home:shell-ready",
  homeToken: "gui-token",
});
assert(
  !shellMessages.slice(shellMessageCountBeforeReady).some(
    (message) => message.payload?.type === "home:shell-context",
  ),
  "Home handed a browser correlation to an unaccepted shell-ready sender",
  shellMessages.slice(shellMessageCountBeforeReady),
);
sendChildMessage("null", shellFrameWindow, {
  type: "home:shell-ready",
  homeToken: "gui-token",
});
const acceptedReadyMessages = shellMessages.slice(shellMessageCountBeforeReady);
const shellContextMessage = acceptedReadyMessages.find(
  (message) => message.payload?.type === "home:shell-context",
);
const shellSummaryMessage = acceptedReadyMessages.find(
  (message) => message.payload?.type === "home:shell-summary",
);
assert(
  shellContextMessage?.origin === "*" &&
    /^browser:[0-9a-f]{32}$/.test(shellContextMessage.payload.browserContextId),
  "accepted Home GUI ready did not receive the exact bounded host correlation",
  acceptedReadyMessages,
);
assert(
  acceptedReadyMessages.indexOf(shellContextMessage) <
    acceptedReadyMessages.indexOf(shellSummaryMessage),
  "Home must bind the GUI correlation before sending the restorable summary",
  acceptedReadyMessages,
);
assert(
  Object.keys(shellContextMessage.payload).sort().join(",") ===
    "browserContextId,type",
  "Home correlation handoff carried authority or Runtime summary facts",
  shellContextMessage,
);
assert(
  localStorageValues.get("elastos.home.browser-context-id") ===
    shellContextMessage.payload.browserContextId,
  "Home did not durably retain its browser-profile correlation",
  Object.fromEntries(localStorageValues),
);
const firstBrowserContextId = shellContextMessage.payload.browserContextId;
sendChildMessage("null", shellFrameWindow, {
  type: "home:shell-ready",
  homeToken: "gui-token",
});
assert(
  shellMessages
    .filter((message) => message.payload?.type === "home:shell-context")
    .every((message) => message.payload.browserContextId === firstBrowserContextId),
  "same Home browser profile regenerated its correlation",
  shellMessages.filter((message) => message.payload?.type === "home:shell-context"),
);
sendChildMessage("null", shellFrameWindow, {
  type: "home:launch-target",
  requestId: "launch-browser",
  target: "browser",
  query: {},
  homeToken: "gui-token",
});
for (let attempt = 0; attempt < 20 && !shellMessages.some((message) => message.payload?.requestId === "launch-browser"); attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
assert(
  requests.some((request) => request.url === "/api/apps/home/launch" && request.body?.target === "browser"),
  "isolated Home GUI could not ask Home to launch Browser",
  requests,
);
assert(
  shellMessages.some((message) =>
    message.origin === "*" &&
    message.payload?.type === "home:shell-response" &&
    message.payload?.requestId === "launch-browser" &&
    message.payload?.result?.route?.includes("#home_token=browser-token")
  ),
  "Home did not return the isolated Browser route to Home GUI",
  shellMessages,
);

const browserFrameWindow = { postMessage() {} };
sendChildMessage("null", browserFrameWindow, {
  type: "home:app-ready",
  homeToken: "browser-token",
});
const connectorLaunchesBeforeBrowserRequest = requests.filter(
  (request) =>
    request.url === "/api/apps/home/launch" &&
    ["wallet-metamask", "wallet-unisat"].includes(request.body?.target),
).length;
sendChildMessage("null", browserFrameWindow, {
  type: "home:open-target",
  target: "wallet-metamask",
  homeToken: "browser-token",
});
await new Promise((resolve) => setTimeout(resolve, 0));
assert(
  requests.filter(
    (request) =>
      request.url === "/api/apps/home/launch" &&
      ["wallet-metamask", "wallet-unisat"].includes(request.body?.target),
  ).length === connectorLaunchesBeforeBrowserRequest,
  "a verified non-Wallet capsule launched a hidden Wallet connector",
  requests,
);
const passkeyRequestsBeforeBrowser = requests.filter(
  (request) => request.url === "/api/auth/passkey-step-up/begin",
).length;
sendChildMessage("null", browserFrameWindow, {
  type: "elastos.home.passkey-step-up.request/v1",
  requestId: "browser-request",
  homeToken: "browser-token",
  operation: "browser.profile.delete",
  request: { profile: "default" },
});
await new Promise((resolve) => setTimeout(resolve, 0));
assert(
  requests.filter((request) => request.url === "/api/auth/passkey-step-up/begin").length ===
    passkeyRequestsBeforeBrowser,
  "Browser frame obtained host passkey authority",
  requests,
);

sendChildMessage("null", shellFrameWindow, {
  type: "home:launch-target",
  requestId: "launch-system",
  target: "system",
  query: {},
  homeToken: "gui-token",
});
for (let attempt = 0; attempt < 20 && !shellMessages.some((message) => message.payload?.requestId === "launch-system"); attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
assert(
  shellMessages.some((message) =>
    message.payload?.requestId === "launch-system" &&
    message.payload?.result?.route?.includes("#home_token=system-token")
  ),
  "Home did not return an isolated System launch",
  shellMessages,
);

let passkeyReply = null;
const systemFrameWindow = {
  postMessage(payload, origin) {
    passkeyReply = { origin, payload };
  },
};
sendChildMessage("null", systemFrameWindow, {
  type: "home:app-ready",
  homeToken: "system-token",
});
const stepUpBeginsBeforeClosedMessage = requests.filter(
  (request) => request.url === "/api/auth/passkey-step-up/begin",
).length;
sendChildMessage("null", systemFrameWindow, {
  type: "elastos.home.passkey-step-up.request/v1",
  requestId: "system-request-with-extra-field",
  homeToken: "system-token",
  operation: "auth.full-recovery-bundle.export",
  request: { label: "Recovery Kit" },
  authority: "legacy",
});
await new Promise((resolve) => setTimeout(resolve, 0));
assert(
  requests.filter((request) => request.url === "/api/auth/passkey-step-up/begin").length ===
    stepUpBeginsBeforeClosedMessage && !passkeyReply,
  "Home accepted a passkey step-up message with an extra field",
  requests,
);
sendChildMessage("null", systemFrameWindow, {
  type: "elastos.home.passkey-step-up.request/v1",
  requestId: "system-request",
  homeToken: "system-token",
  operation: "auth.full-recovery-bundle.export",
  request: { label: "Recovery Kit" },
});
for (let attempt = 0; attempt < 20 && !passkeyReply; attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
assert(
  passkeyReply?.payload?.stepUpToken === "system-step-up-token" &&
    passkeyReply?.payload?.type === "elastos.home.passkey-step-up.result/v1" &&
    passkeyReply?.payload?.requestId === "system-request" &&
    passkeyReply?.origin === "*",
  "validated System frame did not receive capsule-scoped passkey proof",
  passkeyReply,
);
const systemStepUpBegin = requests.find(
  (request) => request.url === "/api/auth/passkey-step-up/begin",
);
assert(
  systemStepUpBegin?.body?.schema === "elastos.auth.passkey-step-up.begin.request/v1" &&
    systemStepUpBegin?.body?.app_token === "system-token" &&
    systemStepUpBegin?.body?.operation === "auth.full-recovery-bundle.export" &&
    systemStepUpBegin?.body?.request?.label === "Recovery Kit",
  "Home did not bind the System step-up ceremony to the original launch and exact request",
  systemStepUpBegin,
);

console.log("[home-shell-bridge] PASS");
