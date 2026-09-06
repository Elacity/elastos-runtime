#!/usr/bin/env node
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const source = readFileSync(new URL("../capsules/home/browser/shell-auth.js", import.meta.url), "utf8")
  .replace(/^import\s*\{[\s\S]*?\}\s*from\s*"[^"]+";\n/, "")
  .replace(/export /g, "");
let verified = false;
let consumed = false;
let creates = 0;
const completions = [];
const grant = { principal_id: "owner", session_id: "one-session", home_token: "one-grant" };

function reloadHome() {
  const elements = new Map();
  const element = (id) => {
    if (!elements.has(id)) elements.set(id, {
      value: "", hidden: false, dataset: {}, textContent: "", disabled: false,
      style: { removeProperty() {}, setProperty() {} },
      classList: { add() {}, remove() {} }, setAttribute() {}, addEventListener() {},
    });
    return elements.get(id);
  };
  let opened = null;
  const context = vm.createContext({
    document: { querySelector: element, body: element("body") },
    window: { PublicKeyCredential: {}, location: { protocol: "https:" }, atob, btoa,
      clearTimeout() {}, setTimeout() {}, setInterval() { return 1; }, clearInterval() {} },
    navigator: { credentials: { async create() {
      creates += 1;
      return { id: "credential", rawId: new Uint8Array([1]).buffer, type: "public-key",
        response: { clientDataJSON: new Uint8Array([2]).buffer, attestationObject: new Uint8Array([3]).buffer } };
    } } },
    Uint8Array, ArrayBuffer, atob, btoa,
    setHomeAuthorityToken() {}, clearHomeAuthorityToken() {},
    async fetchJson(path, options = {}) {
      if (path.endsWith("/status")) return { registered: verified, owner_setup_pending: verified && !consumed, guest_registration_enabled: false };
      if (path.endsWith("/begin")) return { schema: "elastos.auth.passkey.register.begin/v1", ceremony_id: "owner-ceremony", options: verified ? null : {
        publicKey: { challenge: "AQ", user: { id: "AQ" }, excludeCredentials: [] },
      } };
      assert.ok(path.endsWith("/complete"));
      const request = JSON.parse(options.body);
      completions.push(request);
      if (!verified) { verified = true; throw new Error("response lost after verification"); }
      assert.deepEqual(request, { ceremony_id: "owner-ceremony" });
      consumed = true;
      return grant;
    },
  });
  vm.runInContext(source + "\nglobalThis.fixture = { showHomeUnlock, runPasskeyCreate };", context);
  return { element, api: context.fixture, opened: () => opened, onOpen: (result) => { opened = result; } };
}

let home = reloadHome();
await home.api.showHomeUnlock(home.onOpen);
home.element("#home-unlock-name").value = "Owner";
await assert.rejects(home.api.runPasskeyCreate(), /response lost/);
assert.equal(completions[0].display_name, "Owner");
assert.equal(creates, 1);
assert.equal(home.opened(), null);

// New page context: the original attestation is gone; the HttpOnly claimant
// cookie is browser-owned and accompanies the same-origin requests.
home = reloadHome();
await home.api.showHomeUnlock(home.onOpen);
assert.equal(home.element("#home-unlock-primary").textContent, "Resume Home setup");
await home.api.runPasskeyCreate();
assert.equal(creates, 1, "recovery must not create another passkey");
assert.equal(completions.length, 2);
assert.equal(Object.hasOwn(completions[1], "display_name"), false);
assert.deepEqual(home.opened(), grant);
console.log("PASS Home owner enrollment response-loss/reload recovery");
