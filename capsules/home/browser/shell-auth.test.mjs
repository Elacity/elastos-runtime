import assert from "node:assert/strict";
import test from "node:test";

// The real shell-auth module on a minimal page: lock face, card and buttons.
class FakeElement {
  constructor() {
    this.hidden = false;
    this.disabled = false;
    this.textContent = "";
    this.value = "";
    this.title = "";
    this.dataset = {};
    this.attributes = new Map();
    this.listeners = new Map();
    this.style = { setProperty() {}, removeProperty() {} };
    this.classList = { add() {}, remove() {}, toggle() {}, contains: () => false };
  }

  setAttribute(name, value) {
    this.attributes.set(name, String(value));
  }

  addEventListener(type, callback) {
    this.listeners.set(type, callback);
  }

  click() {
    this.listeners.get("click")?.({ target: this });
  }

  focus() {}
}

const elements = new Map();
const element = (selector) => {
  if (!elements.has(selector)) elements.set(selector, new FakeElement());
  return elements.get(selector);
};
globalThis.document = {
  body: { dataset: {} },
  querySelector: element,
  getElementById: (id) => element(`#${id}`),
};
globalThis.location = new URL("http://localhost:61180/home/");
globalThis.window = {
  PublicKeyCredential: function PublicKeyCredential() {},
  location: globalThis.location,
  sessionStorage: { getItem: () => null, setItem() {}, removeItem() {} },
  matchMedia: () => ({ matches: false }),
  setTimeout,
  clearTimeout,
  setInterval: () => 1,
  clearInterval() {},
  addEventListener() {},
  atob,
  btoa,
};

let credentials = {};
Object.defineProperty(globalThis, "navigator", {
  configurable: true,
  value: { credentials: { get: (...args) => credentials.get(...args), create: (...args) => credentials.create(...args) } },
});

let routes = {};
const requests = [];
globalThis.fetch = async (url, init = {}) => {
  requests.push({ url, body: init.body ? JSON.parse(init.body) : null });
  const route = routes[url];
  if (!route) throw new TypeError("Failed to fetch");
  const { status = 200, body = {} } = await route();
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: status === 403 ? "Forbidden" : "",
    text: async () => (typeof body === "string" ? body : JSON.stringify(body)),
    json: async () => body,
  };
};

const fakeCredential = () => ({
  id: "AQ",
  rawId: new Uint8Array([1]).buffer,
  type: "public-key",
  response: {
    clientDataJSON: new Uint8Array([2]).buffer,
    authenticatorData: new Uint8Array([3]).buffer,
    signature: new Uint8Array([4]).buffer,
    attestationObject: new Uint8Array([5]).buffer,
    userHandle: null,
  },
});

const settle = async () => {
  for (let i = 0; i < 20; i += 1) await new Promise((resolve) => setImmediate(resolve));
};

const { addAccountPasskey, bindHomeUnlock, showHomeUnlock } = await import("./shell-auth.js");

routes["/api/auth/passkey/status"] = () => ({ body: { registered: true, guest_registration_enabled: false } });
await showHomeUnlock(() => {});
bindHomeUnlock();
const faceStatus = element("#home-unlock-face-status");

async function signInOnLockFace() {
  assert.equal(element(".home-unlock-face").hidden, false, "the lock face is the sign-in surface");
  assert.equal(element(".home-unlock-card").hidden, true);
  element("#home-unlock-person").click();
  await settle();
  assert.equal(faceStatus.hidden, false, "the failure is visible on the lock face");
  assert.doesNotMatch(faceStatus.textContent, /\d{3}|request failed|enrollment|credential|Error/i);
  return faceStatus.textContent;
}

const begin = () => ({ body: { ceremony_id: "c1", options: { publicKey: { challenge: "AA" } } } });

test("a passkey Home rejects shows one plain reason and one next step on the lock face", async () => {
  routes["/api/auth/passkey/authenticate/begin"] = begin;
  routes["/api/auth/passkey/authenticate/complete"] = () => ({ status: 403, body: "owner enrollment unavailable" });
  credentials = { get: async () => fakeCredential() };
  assert.equal(
    await signInOnLockFace(),
    "Home did not accept this passkey. Use the passkey you created for this Home.",
  );
  assert.equal(faceStatus.dataset.tone, "error");
});

test("a cancelled passkey prompt says so and how to retry", async () => {
  routes["/api/auth/passkey/authenticate/begin"] = begin;
  credentials = { get: async () => null };
  assert.equal(await signInOnLockFace(), "No passkey was used. Select your account to try again.");
  assert.equal(faceStatus.dataset.tone, "muted");
});

test("a busy or unreachable Home is named, not shown as a status code", async () => {
  routes["/api/auth/passkey/authenticate/begin"] = () => ({ status: 429, body: "Passkey sign-in is busy." });
  assert.equal(await signInOnLockFace(), "Sign-in is busy right now. Wait a minute, then try again.");
  delete routes["/api/auth/passkey/authenticate/begin"];
  assert.equal(
    await signInOnLockFace(),
    "Home is not responding. Check that ElastOS is running, then try again.",
  );
});

test("adding a passkey takes a fresh approval first and carries the System launch", async () => {
  requests.length = 0;
  const calls = [];
  routes["/api/auth/passkey-step-up/begin"] = () => ({
    body: {
      schema: "elastos.auth.passkey-step-up.begin.result/v1",
      ceremony_id: "step-up-1",
      options: { publicKey: { challenge: "AA" } },
    },
  });
  routes["/api/auth/passkey-step-up/complete"] = () => ({
    body: { schema: "elastos.auth.passkey-step-up.complete.result/v1", step_up_token: "fresh-approval" },
  });
  routes["/api/auth/passkey/add/begin"] = () => ({
    body: {
      schema: "elastos.auth.passkey.add.begin/v1",
      ceremony_id: "add-1",
      options: { publicKey: { challenge: "AA", user: { id: "AQ", name: "x", displayName: "x" } } },
    },
  });
  routes["/api/auth/passkey/add/complete"] = () => ({
    body: { schema: "elastos.auth.passkey.add.result/v1", proof_binding_id: "proof:passkey:new", role: "admin" },
  });
  credentials = {
    get: async () => { calls.push("approve"); return fakeCredential(); },
    create: async () => { calls.push("create"); return fakeCredential(); },
  };
  assert.deepEqual(await addAccountPasskey("system-launch"), { role: "admin" });
  assert.deepEqual(calls, ["approve", "create"]);
  const byUrl = Object.fromEntries(requests.map((request) => [request.url, request.body]));
  assert.equal(byUrl["/api/auth/passkey-step-up/begin"].operation, "auth.passkey.add");
  assert.equal(byUrl["/api/auth/passkey-step-up/begin"].app_token, "system-launch");
  assert.equal(byUrl["/api/auth/passkey/add/begin"].step_up_token, "fresh-approval");
  assert.equal(byUrl["/api/auth/passkey/add/begin"].app_token, "system-launch");
  assert.equal(byUrl["/api/auth/passkey/add/complete"].ceremony_id, "add-1");
  assert.equal(byUrl["/api/auth/passkey/add/complete"].app_token, "system-launch");
});

test("a refused or cancelled passkey add is one plain sentence", async () => {
  credentials = {
    get: async () => fakeCredential(),
    create: async () => null,
  };
  await assert.rejects(addAccountPasskey("system-launch"), {
    message: "No new passkey was created. Select Add passkey to try again.",
  });
  routes["/api/auth/passkey/add/begin"] = () => ({ status: 403, body: "passkey step-up token has already been used" });
  await assert.rejects(addAccountPasskey("system-launch"), {
    message: "Home did not add the passkey. Select Add passkey to try again.",
  });
});
