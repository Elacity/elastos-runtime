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

const {
  addAccountPasskey,
  bindHomeUnlock,
  offerPasskeyAddAfterSignIn,
  recoveryImportIncompleteMessage,
  showHomeUnlock,
} = await import("./shell-auth.js");

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

test("recovery says it worked when only the Home's recovery record is missing", () => {
  const imported = (wallet, people, audit) => ({
    schema: "elastos.full-recovery-bundle.import.response/v2",
    wallet_restore: { status: wallet },
    people_identity_restore: { status: people },
    runtime_audit: { status: audit },
  });
  assert.equal(recoveryImportIncompleteMessage(imported("complete", "restored", "complete")), "");
  const auditOnly = recoveryImportIncompleteMessage(imported("complete", "restored", "incomplete"));
  assert.match(auditOnly, /^Your account, Profile and Wallet were recovered\./);
  assert.doesNotMatch(auditOnly, /could not finish|failed|needs completion/i);
  assert.match(auditOnly, /Choose the same kit again to finish it\.$/);
  for (const response of [
    imported("incomplete", "restored", "complete"),
    imported("complete", "incomplete", "incomplete"),
  ]) {
    assert.equal(recoveryImportIncompleteMessage(response),
      "The root was recovered. Profile or Wallet recovery still needs completion. Choose the same kit again to retry safely.");
  }
  assert.match(recoveryImportIncompleteMessage(imported("complete", "absent", "complete")),
    /has no signed Profile or name/);
});

test("signing in by account name lists that account's passkeys and offers Add passkey after", async () => {
  const flows = [];
  await showHomeUnlock((response, flow) => { flows.push({ response, flow }); });
  const card = element(".home-unlock-card");
  const account = element("#home-unlock-account");
  const status = element("#home-unlock-status");
  assert.equal(account.hidden, true, "the account name is asked only in the fallback");
  element("#home-unlock-named").click();
  assert.equal(element(".home-unlock-face").hidden, true);
  assert.equal(card.hidden, false);
  assert.equal(account.hidden, false);
  assert.equal(element("#home-unlock-account-label").hidden, false);
  assert.equal(element("#home-unlock-title").textContent, "Sign in with your account name");
  assert.equal(element("#home-unlock-primary").textContent, "Continue");
  assert.equal(element("#home-unlock-secondary").textContent, "Back to sign in");

  requests.length = 0;
  account.value = "   ";
  element("#home-unlock-primary").click();
  await settle();
  assert.equal(status.textContent, "Enter your account name as this Home shows it.");
  assert.equal(requests.length, 0, "an empty name never reaches Home");

  let requested = null;
  routes["/api/auth/passkey/authenticate/named/begin"] = () => ({
    body: { ceremony_id: "passkey:authenticate-named:1", options: { publicKey: {
      challenge: "AA", allowCredentials: [{ type: "public-key", id: "AQ" }],
    } } },
  });
  routes["/api/auth/passkey/authenticate/complete"] = () => ({ body: { home_token: "signed" } });
  credentials = { get: async (request) => { requested = request; return fakeCredential(); } };
  account.value = "  Alice   Example ";
  element("#home-unlock-primary").click();
  await settle();
  const begin = requests.find((request) => request.url === "/api/auth/passkey/authenticate/named/begin");
  assert.deepEqual(begin.body, {
    schema: "elastos.auth.passkey.authenticate.named.begin.request/v1",
    account_name: "Alice Example",
  });
  assert.deepEqual([...new Uint8Array(requested.publicKey.allowCredentials[0].id)], [1]);
  const complete = requests.find((request) => request.url === "/api/auth/passkey/authenticate/complete");
  assert.equal(complete.body.ceremony_id, "passkey:authenticate-named:1");
  assert.deepEqual(flows.at(-1).flow, { signInMethod: "account_name" },
    "Home is told to offer Add passkey once");
  delete routes["/api/auth/passkey/authenticate/named/begin"];
});

test("a wrong account name and a missing passkey read the same", async () => {
  await showHomeUnlock(() => {});
  element("#home-unlock-named").click();
  const status = element("#home-unlock-status");
  element("#home-unlock-account").value = "Nobody";
  routes["/api/auth/passkey/authenticate/named/begin"] = () => ({
    body: { ceremony_id: "passkey:authenticate-named:2", options: { publicKey: {
      challenge: "AA", allowCredentials: [{ type: "public-key", id: "Ag" }],
    } } },
  });
  credentials = { get: async () => { throw Object.assign(new Error("none"), { name: "NotAllowedError" }); } };
  element("#home-unlock-primary").click();
  await settle();
  assert.equal(status.textContent,
    "No passkey for that account name was used. Check the name, then try again with your security key or device.");
  assert.equal(status.dataset.tone, "muted");
  routes["/api/auth/passkey/authenticate/named/begin"] = () => ({ status: 422, body: "Enter your account name as this Home shows it." });
  element("#home-unlock-primary").click();
  await settle();
  assert.equal(status.textContent, "Enter your account name as this Home shows it.");
  delete routes["/api/auth/passkey/authenticate/named/begin"];
  element("#home-unlock-secondary").click();
  assert.equal(element(".home-unlock-face").hidden, false, "Back to sign in returns to the lock face");
  assert.equal(element("#home-unlock-account").hidden, true);
});

test("after an account-name sign-in a CLI-shell person stays on the CLI shell", async () => {
  const named = { signInMethod: "account_name" };
  const opened = [];
  const openSystem = async (query) => { opened.push(query); };
  routes["/api/auth/passkeys"] = () => ({ body: { passkeys: [
    { current: false, role: "guest" }, { current: true, role: "admin" },
  ] } });
  requests.length = 0;
  assert.equal(await offerPasskeyAddAfterSignIn({
    flow: named, homeToken: "signed", desktopActive: false, openSystem,
  }), false);
  assert.deepEqual(requests, [], "no shell switch, no System, no lookup on the CLI shell");
  assert.deepEqual(opened, []);

  // An ordinary passkey sign-in never offers it.
  assert.equal(await offerPasskeyAddAfterSignIn({
    flow: null, homeToken: "signed", desktopActive: true, openSystem,
  }), false);
  assert.deepEqual(opened, []);

  // An admin already on the Desktop gets the offer once, with no shell change.
  assert.equal(await offerPasskeyAddAfterSignIn({
    flow: named, homeToken: "signed", desktopActive: true, openSystem,
  }), true);
  assert.deepEqual(opened, [{ settings: "account", passkey: "add" }]);
  assert.deepEqual(requests.map((request) => request.url), ["/api/auth/passkeys"]);

  // A guest gets no System navigation.
  routes["/api/auth/passkeys"] = () => ({ body: { passkeys: [{ current: true, role: "guest" }] } });
  assert.equal(await offerPasskeyAddAfterSignIn({
    flow: named, homeToken: "signed", desktopActive: true, openSystem,
  }), false);
  assert.equal(opened.length, 1);

  // Nothing in the offer can fail the sign-in.
  routes["/api/auth/passkeys"] = () => ({ body: { passkeys: [{ current: true, role: "admin" }] } });
  const quiet = console.error;
  console.error = () => {};
  try {
    assert.equal(await offerPasskeyAddAfterSignIn({
      flow: named, homeToken: "signed", desktopActive: true,
      openSystem: async () => { throw new Error("System could not open the requested settings"); },
    }), false);
    delete routes["/api/auth/passkeys"];
    assert.equal(await offerPasskeyAddAfterSignIn({
      flow: named, homeToken: "signed", desktopActive: true, openSystem,
    }), false);
  } finally {
    console.error = quiet;
  }
  assert.ok(!requests.some((request) => request.url === "/api/apps/home/active-shell"));
  // The host wires this offer without any shell change of its own.
  const { readFileSync } = await import("node:fs");
  const host = readFileSync(new URL("./home-shell-host.js", import.meta.url), "utf8");
  const start = host.indexOf("async function showHostAuthGate(");
  const offer = host.indexOf("offerPasskeyAddAfterSignIn({", start);
  const end = host.indexOf("\n}\n", offer);
  assert.ok(start >= 0 && offer > start && end > offer);
  const branch = host.slice(host.lastIndexOf("} else {", offer), end);
  assert.doesNotMatch(branch, /activateDesktopShell|active-shell/);
});
