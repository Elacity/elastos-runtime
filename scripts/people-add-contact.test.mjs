import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../capsules/people/browser/people.js", import.meta.url), "utf8");
function functions(names, bindings = {}, code = source) {
  const context = vm.createContext(bindings);
  for (const name of names) {
    const start = code.search(new RegExp(`(?:async )?function ${name}\\(`));
    assert(start >= 0, `${name} exists`);
    const next = code.indexOf("\n}", start) + 2;
    vm.runInContext(code.slice(start, next), context);
  }
  return context;
}

for (const [status, message, expected] of [
  [400, "That isn't a DID. A DID starts with did:key:z.", "That isn't a DID. A DID starts with did:key:z."],
  [409, "That's your own DID.", "That's your own DID."],
  [500, "database secret", "Could not send the request. Try again."],
  [401, "launch token expired", "Could not send the request. Try again."],
  [undefined, "Failed to fetch", "Could not send the request. Try again."],
]) {
  test(`contact request error ${status ?? "network"} uses public copy`, async () => {
    const context = functions(["readText", "publicError", "sendContactRequest"], {
      localProfileId: "did:key:zLocal", sendingContact: false,
      addContactInput: { value: "did:key:zRemote" }, addContactSubmit: {},
      addContactInbox: {}, addContactResult: {}, prefillContactFromLaunch() {},
      fetchJson: async () => { throw Object.assign(new Error(message), { status }); },
    });
    await context.sendContactRequest();
    assert.equal(context.addContactResult.textContent, expected);
    assert.equal(context.addContactInput.value, "did:key:zRemote");
    assert.equal(context.addContactSubmit.disabled, false);
  });
}

test("Profile save keeps DID diagnostics private", () => {
  const context = functions(["readText", "profileSaveFailureMessage"], {
    profileForm: { dataset: { profileState: "saved" } },
  });
  assert.equal(context.profileSaveFailureMessage(new Error("Invalid did:key:zPrivate")), "Could not save your Profile. Try again.");
});

test("unknown DID outcomes start with Request sent", () => {
  const context = functions(["readText", "contactRequestMessage"]);
  for (const status of ["requested", "already_requested"]) {
    assert.match(context.contactRequestMessage({ status }), /^Request sent\. Name not known yet\./);
  }
});

test("unknown contact rows include each DID suffix", () => {
  const context = functions(["readText", "contactMarkup"], {
    relationshipLabel: () => "Request sent", reachabilityLabel: () => "",
    escapeHtml: value => value, personCard: value => value,
    removingContactId: "", pendingRemoveId: "",
  });
  for (const suffix of ["a1b2c3", "d4e5f6"]) {
    const row = context.contactMarkup({ contact_id: suffix, display_name: "Name not known yet",
      remote_profile_did: `did:key:z6Mk${suffix}`, relationship: "requested" });
    assert.equal(row.name, `Name not known yet (…${suffix})`);
  }
  assert.equal(context.contactMarkup({ contact_id: "known", display_name: "Alma", remote_profile_did: "did:key:z6Mka1b2c3" }).name, "Alma");
});

test("People launch prefills and focuses a bare DID without sending", async () => {
  const did = "did:key:z6Mka1b2c3";
  for (const value of [did, "did:key:z", "did:key:z0OIl", `${did}/path`, `${did}?q=1`, `${did}#key`, ` ${did}`, `${did} `, `${did}\n`, `hello ${did}`, "https://example.com"]) {
    let focused = false;
    let posts = 0;
    const context = functions(["boot", "readText", "renderProfileId", "prefillContactFromLaunch"], {
      launchParams: new URLSearchParams({ did: value }),
      localProfileId: "did:key:zLocal", sendingContact: false, homeToken: "test-token",
      bindNavigation() {}, bindActions() {}, activateSection() {},
      lockedShell: { classList: { add() {}, remove() {} } }, peopleShell: { classList: { add() {}, remove() {} } },
      homeClipboard: { start() {} }, refreshPeople: async () => context.renderProfileId("did:key:zLocal"),
      profileIdValue: null, profileIdSection: null, addContactForm: {},
      addContactInput: { value: "", focus() { focused = true; }, scrollIntoView() {} },
      sendContactRequest() { posts++; },
    });
    await context.boot();
    assert.equal(context.addContactInput.value, value === did ? did : "");
    assert.equal(focused, value === did);
    assert.equal(posts, 0);
  }
});

test("Home passes a DID to the existing People window", async () => {
  const query = { did: "did:key:z6Mka1b2c3" };
  const entry = { id: "people-window" };
  const navigations = [];
  const windows = readFileSync(new URL("../capsules/home-gui/browser/shell-windows.js", import.meta.url), "utf8");
  const context = functions(["launchBrowserTargetWindow"], {
    isSingleWindowTarget: () => true, topBrowserWindowEntryForTarget: () => entry,
    shellState: { windows: new Map([[entry.id, entry]]) }, focusWindow() {},
    SYSTEM_APP_ID: "system", postPeopleNavigate: (entry, query) => navigations.push({ entry, query }),
  }, windows);
  await context.launchBrowserTargetWindow("people", { query });
  assert.deepEqual(navigations, [{ entry, query }]);
});

test("People accepts DID navigation only from its Home GUI and keeps drafts on refresh", () => {
  const parent = {};
  let focused = 0;
  const did = "did:key:z6Mkd4e5f6";
  const context = functions(["readText", "renderProfileId", "prefillContactFromLaunch", "receivePeopleNavigation"], {
    window: { parent }, homeToken: "people-token", launchParams: new URLSearchParams(),
    localProfileId: "", sendingContact: false, profileIdValue: null, profileIdSection: null, addContactForm: {},
    activateSection() {}, addContactInput: { value: "draft", focus() { focused++; }, scrollIntoView() {} },
  });
  const data = { type: "elastos.people.navigate/v1", homeToken: "people-token", query: { did } };
  const event = { source: parent, origin: "null", data };
  for (const invalid of [
    { ...event, source: {} }, { ...event, origin: "https://foreign.example" },
    { ...event, data: { ...data, homeToken: "wrong" } },
    { ...event, data: { ...data, query: { did, send: true } } },
    { ...event, data: { ...data, query: { did: `${did}#key` } } },
  ]) {
    context.receivePeopleNavigation(invalid);
    assert.equal(context.addContactInput.value, "draft");
    assert.equal(focused, 0);
  }
  context.receivePeopleNavigation(event);
  assert.equal(context.addContactInput.value, "draft", "wait for Profile creation");
  context.renderProfileId("did:key:zLocal");
  assert.equal(context.addContactInput.value, did);
  assert.equal(focused, 1);
  context.addContactInput.value = "edited draft";
  context.renderProfileId("did:key:zLocal");
  assert.equal(context.addContactInput.value, "edited draft");
  assert.equal(focused, 1);
});

test("Home addresses People navigation to the current frame authority", () => {
  const sent = [];
  const frame = { dataset: { route: "/apps/people/#home_token=current" },
    contentWindow: { postMessage: (...args) => sent.push(args) } };
  const entry = { node: { querySelector: () => frame } };
  const windows = readFileSync(new URL("../capsules/home-gui/browser/shell-windows.js", import.meta.url), "utf8");
  const context = functions(["postPeopleNavigate", "browserLaunchAuthority"], {
    URL, URLSearchParams, window: { location: { href: "https://home.example/" } },
  }, windows);
  context.postPeopleNavigate(entry, { did: "did:key:z6Mka1b2c3" });
  assert.deepEqual(JSON.parse(JSON.stringify(sent)), [[{ type: "elastos.people.navigate/v1",
    homeToken: "current", query: { did: "did:key:z6Mka1b2c3" } }, "*"]]);
  frame.dataset.route = "/apps/people/";
  context.postPeopleNavigate(entry, { did: "did:key:z6Mkd4e5f6" });
  assert.equal(sent.length, 1);
});

test("a DID opened during a send remains ready for a separate request", async () => {
  const parent = {};
  const original = "did:key:z6Mka1b2c3";
  const next = "did:key:z6Mkd4e5f6";
  let complete;
  const posts = [];
  const context = functions(["readText", "publicError", "contactRequestMessage", "sendContactRequest",
    "prefillContactFromLaunch", "receivePeopleNavigation"], {
    window: { parent }, homeToken: "people-token", launchParams: new URLSearchParams(),
    localProfileId: "did:key:zLocal", sendingContact: false, activateSection() {},
    addContactInput: { value: original, focus() {}, scrollIntoView() {} },
    addContactSubmit: {}, addContactInbox: {}, addContactResult: {},
    fetchJson: (_, options) => { posts.push(JSON.parse(options.body)); return new Promise(resolve => { complete = resolve; }); },
    refreshPeople: async () => {},
  });
  const sending = context.sendContactRequest();
  context.receivePeopleNavigation({ source: parent, origin: "null", data: {
    type: "elastos.people.navigate/v1", homeToken: "people-token", query: { did: next },
  } });
  complete({ status: "requested", display_name: "Alma" });
  await sending;
  assert.equal(context.addContactInput.value, next);
  assert.deepEqual(posts, [{ profile_id: original }]);
});
