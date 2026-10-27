#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

class FakeElement {
  constructor() {
    this.classList = { add() {}, remove() {}, toggle() {} };
    this.dataset = {};
    this.disabled = false;
    this.hidden = false;
    this.innerHTML = "";
    this.textContent = "";
    this.value = "";
    this.placeholder = "";
    this.listeners = new Map();
  }

  addEventListener(type, callback) {
    this.listeners.set(type, callback);
  }

  querySelector() {
    return null;
  }

  querySelectorAll() {
    return [];
  }

  closest() {
    return this;
  }
}

function response(status, payload) {
  return {
    ok: status >= 200 && status < 300,
    status,
    async text() {
      return JSON.stringify(payload);
    },
  };
}

function discoverySummary({
  configured = false,
  enabled = false,
  status = configured ? (enabled ? "visible" : "off") : "unconfigured",
  statusMessage = configured
    ? (enabled
      ? "Discovery is on. Visibility lasts up to ten minutes, and both people must be visible at the same time."
      : "Discovery is off.")
    : "Discovery is unavailable.",
  discoveredPeers = [],
  pendingRequestCount = 0,
  remainingSeconds = null,
  remoteVisibilityMayRemainUntil = null,
  remoteVisibilityRemainingSeconds = null,
} = {}) {
  return {
    schema: "elastos.people.discovery/v1",
    configured,
    enabled,
    status,
    status_message: statusMessage,
    expires_at: remainingSeconds === null ? null : 1_000 + remainingSeconds,
    remaining_seconds: remainingSeconds,
    remote_visibility_may_remain_until: remoteVisibilityMayRemainUntil,
    remote_visibility_remaining_seconds: remoteVisibilityRemainingSeconds,
    discovered_count: discoveredPeers.length,
    discovered_peers: discoveredPeers,
    request_count: pendingRequestCount,
  };
}

function summary(
  displayName = "Runtime Name",
  discovery = discoverySummary(),
  identity = {},
  readinessStatus = "setup_required",
) {
  const projectedIdentity = {
    ...identity,
    profile_readiness: {
      schema: "elastos.profile.readiness/v1",
      status: readinessStatus,
    },
  };
  const contact = {
    contact_id: "contact:remote",
    profile_card: { display_name: "Stale Profile" },
    relationship: "connected",
    can_message: true,
    conversation_id: "direct:opaque:remote",
    route: "/apps/legacy-route-must-not-be-used/",
  };
  if (displayName !== undefined) {
    contact.display_name = displayName;
  }
  const contacts = [contact, {
    contact_id: "",
    display_name: "Missing contact id",
  }];
  if (displayName !== null) {
    contacts.push({
      contact_id: "contact:no-selector",
      display_name: "No selector",
      can_message: true,
      conversation_id: { invalid: true },
      route: "/apps/legacy-route-must-not-be-used/",
    });
  }
  return response(200, {
    identity: projectedIdentity,
    people: {
      contacts,
    },
    discovery,
  });
}

function discoveryResponse(options) {
  return response(200, discoverySummary(options));
}

function setupEnvironment(name, replies) {
  const nodes = new Map([
    ["locked-shell", new FakeElement()],
    ["people-shell", new FakeElement()],
    ["people-status", new FakeElement()],
    ["profile-form", new FakeElement()],
    ["profile-name", new FakeElement()],
    ["profile-title", new FakeElement()],
    ["profile-description", new FakeElement()],
    ["profile-submit", new FakeElement()],
    ["people-list", new FakeElement()],
    ["discovery-list", new FakeElement()],
    ["discovery-requests-list", new FakeElement()],
    ["discovery-status", new FakeElement()],
    ["discovery-toggle", new FakeElement()],
    ["discovery-refresh", new FakeElement()],
    ["people-count", new FakeElement()],
    ["discovery-visible-count", new FakeElement()],
    ["discovery-requests-count", new FakeElement()],
    ["people-page-title", new FakeElement()],
  ]);
  for (const id of ["profile-id", "profile-id-value", "profile-id-copy", "add-contact-form", "add-contact-id", "add-contact-submit", "add-contact-result", "add-contact-inbox"]) nodes.set(id, new FakeElement());
  const copies = [];
  globalThis.createClipboardFixture = (options) => ({
    start() { assert.equal(options.targetId, "people"); },
    async writeText(text, options) { copies.push({ text, ...options }); },
  });
  const listeners = new Map();
  const documentListeners = new Map();
  const timers = new Map();
  const messages = [];
  const calls = [];
  let timerId = 0;
  let clearCount = 0;
  const parentFrame = {
    postMessage(message, origin) {
      messages.push({ message, origin });
    },
  };

  globalThis.Element = FakeElement;
  globalThis.HTMLButtonElement = FakeElement;
  globalThis.document = {
    activeElement: null,
    getElementById(id) {
      return nodes.get(id) || null;
    },
    querySelectorAll() {
      return [];
    },
    querySelector(selector) {
      return selector === ".people-page-title" ? nodes.get("people-page-title") : null;
    },
    addEventListener(type, callback) {
      documentListeners.set(type, callback);
    },
  };
  globalThis.window = {
    location: {
      search: "?home_origin=http%3A%2F%2Flocalhost%3A61180",
      hash: "#home_token=people-test-token",
      origin: "http://localhost:61180",
    },
    confirm() {
      throw new Error("Native confirmation is blocked by the app sandbox");
    },
    top: parentFrame,
    parent: parentFrame,
    addEventListener(type, callback) {
      listeners.set(type, callback);
    },
    setInterval(callback, delay) {
      timerId += 1;
      timers.set(timerId, { callback, delay });
      return timerId;
    },
    clearInterval(id) {
      clearCount += 1;
      timers.delete(id);
    },
    postMessage(message) {
      messages.push(message);
    },
  };
  globalThis.fetch = async (path, options = {}) => {
    calls.push({ path, options });
    const reply = replies.shift();
    assert(reply, `${name}: unexpected fetch for ${path}`);
    return typeof reply === "function" ? reply(path) : reply;
  };

  return {
    copies,
    calls,
    listeners,
    documentListeners,
    nodes,
    replies,
    timers,
    clearCount: () => clearCount,
    messages,
    parentFrame,
  };
}

async function loadPeople() {
  const source = readFileSync(new URL("../capsules/people/browser/people.js", import.meta.url), "utf8")
    .replace(/import \{ createHomeClipboardClient \} from [^;]+;/,
      "const createHomeClipboardClient = globalThis.createClipboardFixture;");
  await import(`data:text/javascript;base64,${Buffer.from(source).toString("base64")}`);
}

async function settle() {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 0));
  }
}

function assertRequestAuthority(environment) {
  for (const { path, options } of environment.calls) {
    assert.equal(options.headers.get("x-elastos-home-token"), "people-test-token", `${path}: token`);
    assert.equal(path.startsWith("/api/apps/people/presence"), false);
  }
}

function assertNoDiscoveryFallbacks(markup, label) {
  assert.doesNotMatch(markup, /\bPerson\b/, `${label}: should not render Person fallback`);
  assert.doesNotMatch(markup, /\bElastOS Home\b/, `${label}: should not render ElastOS Home fallback`);
  assert.doesNotMatch(markup, /\bElastOS user\b/, `${label}: should not render ElastOS user fallback`);
  assert.doesNotMatch(markup, /\bDevice [0-9a-f]{8}\b/, `${label}: should not render device-label fallback`);
}

function assertNoRawIdentityLeak(text, label) {
  assert.doesNotMatch(text, /did:key:/, `${label}: leaked a DID`);
  assert.doesNotMatch(text, /\bdevice\b/i, `${label}: leaked a device identity`);
  assert.doesNotMatch(text, /\bendpoint\b/i, `${label}: leaked an endpoint identity`);
  assert.doesNotMatch(text, /\broute\b/i, `${label}: leaked a route detail`);
  assert.doesNotMatch(text, /\bprovider\b/i, `${label}: leaked a provider detail`);
}

async function triggerAction(environment, action, dataset = {}) {
  const button = new FakeElement();
  button.dataset = { action, ...dataset };
  environment.documentListeners.get("click")({ target: button });
  await settle();
}

async function triggerProfileSubmit(environment) {
  environment.nodes.get("profile-form").listeners.get("submit")({ preventDefault() {} });
  await settle();
}

async function triggerProfileInput(environment, value) {
  const input = environment.nodes.get("profile-name");
  input.value = value;
  input.listeners.get("input")?.({ target: input });
  await settle();
}

async function triggerMenuCommand(environment, cmd, {
  origin = "null",
  source = environment.parentFrame,
} = {}) {
  const listener = environment.listeners.get("message");
  assert(listener, "menu command listener missing");
  listener({
    origin,
    source,
    data: { type: "elastos:menu-command", cmd },
  });
  await settle();
}

async function runScenario(name) {
  if (name === "request_delivery_detail") {
    const detail = "Contact requests can't be delivered right now.";
    const contacts = [
      { contact_id: "contact:waiting", display_name: "Name not known yet", relationship: "requested", delivery_detail: detail },
      { contact_id: "contact:normal", display_name: "Alma", relationship: "requested" },
      { contact_id: "contact:connected", display_name: "Bea", relationship: "connected", delivery_detail: detail },
      { contact_id: "contact:escaped", display_name: "Cy", relationship: "requested", delivery_detail: '<script>alert("fixture")</script>' },
    ];
    const environment = setupEnvironment(name, [response(200, { people: { contacts } })]);
    await loadPeople();
    await settle();
    const markup = environment.nodes.get("people-list").innerHTML;
    const cards = markup.match(/<article[\s\S]*?<\/article>/g);
    assert.equal(cards.length, 4);
    assert.ok(cards[0].includes("Contact requests can&#039;t be delivered right now."), "Requested row lost its delivery detail");
    assert.ok(cards[1].includes("Request sent"));
    assert.ok(!cards[1].includes("delivered"), "Normal requested row invented a delivery failure");
    assert.ok(!cards[2].includes("delivered"), "Delivery detail belongs only to requested rows");
    assert.ok(cards[3].includes("&lt;script&gt;"), "Runtime detail must remain text");
    assert.ok(!cards[3].includes("<script>"));
    return;
  }
  if (name === "add_by_did") {
    const ready = () => response(200, {
      profile_id: "did:key:zLocalProfile",
      identity: { profile: { display_name: "Local" }, profile_readiness: { schema: "elastos.profile.readiness/v1", status: "ready" } },
      people: { contacts: [{ contact_id: "contact:unknown", display_name: "Name not known yet", relationship: "requested" }] },
      discovery: discoverySummary({ configured: true, enabled: false }),
    });
    const environment = setupEnvironment(name, [ready()]);
    await loadPeople();
    await settle();
    const node = (id) => environment.nodes.get(id);
    assert.equal(node("profile-id-value").textContent, "did:key:zLocalProfile");
    assert.equal(node("profile-id").hidden, false);
    assert.match(node("people-list").innerHTML, /Name not known yet/);
    await node("profile-id-copy").listeners.get("click")();
    await settle();
    assert.deepEqual(environment.copies, [{ text: "did:key:zLocalProfile", purpose: "profile.id" }]);
    const expected = {
      requested: "Request sent. Alma sees it in Inbox when their Home is online.",
      already_requested: "Request sent. Waiting for Alma to accept.",
      already_contact: "Alma is already a contact.",
      pending_from_them: "Alma already asked to connect. Open Inbox to answer.",
      own_profile: "That's your own DID.",
    };
    for (const [status, message] of Object.entries(expected)) {
      environment.replies.push(response(200, { status, display_name: "Alma" }), ready());
      node("add-contact-id").value = "  did:key:zRemoteProfile  ";
      node("add-contact-form").listeners.get("submit")({ preventDefault() {} });
      await settle();
      assert.equal(node("add-contact-result").textContent, message);
      assert.equal(node("add-contact-inbox").hidden, status !== "pending_from_them");
      assert.equal(node("add-contact-submit").disabled, false);
      assert.equal(node("add-contact-id").value, "");
      const request = environment.calls.at(-2);
      assert.equal(request.path, "/api/apps/people/contacts/request");
      assert.deepEqual(JSON.parse(request.options.body), { profile_id: "did:key:zRemoteProfile" });
    }
    environment.replies.push(response(200, { status: "requested", display_name: "Name not known yet" }), ready());
    node("add-contact-id").value = "did:key:zUnknown";
    node("add-contact-form").listeners.get("submit")({ preventDefault() {} });
    await settle();
    assert.equal(node("add-contact-result").textContent, "Name not known yet. Request sent. They'll see it in Inbox when their Home is online.");
    for (const [status, message] of [
      [400, "That isn't a DID. A DID starts with did:key:z."],
      [400, "That's this Home's device DID. Use the DID shown in People."],
      [409, "You have 16 requests waiting. Wait for answers before sending more."],
      [409, "Contact requests can't be delivered right now. Try again later."],
      [409, "This Home left Community. Rejoin in System → Security, under Community. Contacts, Direct messages and history remain available."],
    ]) {
      environment.replies.push({ ok: false, status, text: async () => message });
      node("add-contact-id").value = "bad input";
      node("add-contact-form").listeners.get("submit")({ preventDefault() {} });
      await settle();
      assert.equal(node("add-contact-result").textContent, message);
      assert.equal(node("add-contact-id").value, "bad input");
      assert.equal(node("add-contact-submit").disabled, false);
    }
    let finish;
    environment.replies.push(() => new Promise(resolve => { finish = resolve; }), ready());
    node("add-contact-id").value = "did:key:zRemote";
    node("add-contact-form").listeners.get("submit")({ preventDefault() {} });
    const count = environment.calls.length;
    node("add-contact-form").listeners.get("submit")({ preventDefault() {} });
    assert.equal(environment.calls.length, count, "Two submits queued duplicate requests");
    assert.equal(node("add-contact-submit").disabled, true);
    finish(response(200, { status: "requested", display_name: "Alma" }));
    await settle();
    assert.equal(node("add-contact-submit").disabled, false);
    assertRequestAuthority(environment);
    return;
  }
  if (name === "community_left") {
    const environment = setupEnvironment(name, [summary("Retained Contact", discoverySummary({
      configured: true, enabled: false, status: "community_left",
      statusMessage: "This Home left Community. Rejoin in System → Security, under Community. Contacts, Direct messages and history remain available.",
    }))]);
    await loadPeople();
    await settle();
    assert.equal(environment.nodes.get("discovery-toggle").disabled, true);
    assert.equal(environment.nodes.get("discovery-refresh").disabled, true);
    assert.match(environment.nodes.get("discovery-status").textContent, /Rejoin in System → Security, under Community\./);
    assert.match(environment.nodes.get("people-list").innerHTML, /Retained Contact/);
    assert.match(environment.nodes.get("people-list").innerHTML, /data-action="chat" data-conversation-id="direct:opaque:remote">Message/);
    assert.equal(environment.replies.length, 0);
    assertRequestAuthority(environment);
    return;
  }
  if (name === "remove_confirmation") {
    const connected = summary("Exact Contact");
    const removedBody = JSON.parse(await summary("Exact Contact").text());
    removedBody.people.contacts[0].relationship = "removed";
    removedBody.people.contacts[0].can_message = false;
    const environment = setupEnvironment(name, [connected]);
    await loadPeople();
    await settle();
    const markup = () => environment.nodes.get("people-list").innerHTML;
    const removals = () => environment.calls.filter(({ path }) => path === "/api/apps/people/contacts/remove");
    await triggerAction(environment, "remove", { contactId: "contact:foreign" });
    assert.doesNotMatch(markup(), /person-confirm/);
    await triggerAction(environment, "remove", { contactId: "contact:remote" });
    assert.match(markup(), /Remove Exact Contact from People\?/);
    assert.match(markup(), /data-action="cancel-remove" data-contact-id="contact:remote"/);
    assert.equal(removals().length, 0, "Opening confirmation must not remove the contact");
    await triggerAction(environment, "cancel-remove", { contactId: "contact:foreign" });
    assert.match(markup(), /person-confirm/);
    await triggerAction(environment, "cancel-remove", { contactId: "contact:remote" });
    assert.doesNotMatch(markup(), /person-confirm/);
    assert.equal(removals().length, 0, "Cancel must not send a removal POST");
    await triggerAction(environment, "confirm-remove", { contactId: "contact:remote" });
    assert.equal(removals().length, 0, "An unopened confirmation cannot remove a contact");
    await triggerAction(environment, "remove", { contactId: "contact:remote" });
    await triggerAction(environment, "confirm-remove", { contactId: "contact:foreign" });
    assert.equal(removals().length, 0, "Confirmation must keep its exact target");
    let release;
    environment.replies.push(() => new Promise((resolve) => { release = resolve; }));
    await triggerAction(environment, "confirm-remove", { contactId: "contact:remote" });
    assert.equal(removals().length, 1);
    assert.match(markup(), /data-action="confirm-remove" data-contact-id="contact:remote" disabled/);
    await triggerAction(environment, "confirm-remove", { contactId: "contact:remote" });
    await triggerAction(environment, "cancel-remove", { contactId: "contact:remote" });
    assert.equal(removals().length, 1, "A pending removal must dispatch once");
    release(response(401, { error: "unauthorized" }));
    await settle();
    assert.equal(environment.nodes.get("people-status").dataset.tone, "error");
    assert.equal(environment.nodes.get("people-status").textContent, "Could not complete that action.");
    assert.match(markup(), /data-action="confirm-remove" data-contact-id="contact:remote">Remove/);
    environment.replies.push(response(200, {}), response(200, removedBody));
    await triggerAction(environment, "confirm-remove", { contactId: "contact:remote" });
    assert.equal(removals().length, 2);
    assert.deepEqual(removals().map(({ options }) => JSON.parse(options.body)), [
      { contact_id: "contact:remote" }, { contact_id: "contact:remote" },
    ], "A retry must preserve the original target");
    assert.doesNotMatch(markup(), /data-action="(?:remove|confirm-remove|cancel-remove)"/);
    assert.match(markup(), /Removed/);
    environment.replies.push(summary("Exact Contact"));
    await triggerMenuCommand(environment, "refresh");
    await triggerAction(environment, "remove", { contactId: "contact:remote" });
    environment.replies.push(response(200, removedBody));
    await triggerMenuCommand(environment, "refresh");
    await triggerAction(environment, "confirm-remove", { contactId: "contact:remote" });
    assert.equal(removals().length, 2, "A stale removed target must not send another POST");
    assertRequestAuthority(environment);
    assert.equal(environment.replies.length, 0);
    return;
  }
  if (name === "configured") {
    const initialDiscovery = discoverySummary({
      configured: true,
      enabled: true,
      discoveredPeers: [{
        advertisement_id: "ad-1",
        display_name: "ESPFix",
        handle: "espfix",
        status: "visible",
        peer_id: "must-not-render",
        connect_ticket: "must-not-render",
        topic: "must-not-render",
      }, {
        advertisement_id: "ad-contact",
        display_name: "Already Added",
        status: "contact",
      }, {
        advertisement_id: "ad-requested",
        display_name: "Asked Already",
        status: "requested",
      }, {
        advertisement_id: "ad-pending",
        display_name: "Asking Me",
        status: "pending",
      }],
      pendingRequestCount: 1,
    });
    const refreshedDiscovery = discoverySummary({
      configured: true,
      enabled: true,
      discoveredPeers: [],
      statusMessage: "People who choose to be visible will appear here.",
    });
    const environment = setupEnvironment(name, [
      summary(null, initialDiscovery, {
        profile_setup_display_name: "Suggested Profile Name",
      }),
      discoveryResponse(refreshedDiscovery),
      summary("Mac", refreshedDiscovery, {
        profile: { display_name: "Mac Profile" },
        profile_setup_display_name: "Do not use this suggestion",
      }, "ready"),
      discoveryResponse(initialDiscovery),
      summary("Runtime Name", initialDiscovery, {
        profile: { display_name: "Mac Profile" },
      }, "ready"),
      discoveryResponse({
        configured: true,
        enabled: false,
        status: "off",
        statusMessage: "Discovery is off.",
        discoveredPeers: [],
      }),
      summary("Runtime Name", discoverySummary({
        configured: true,
        enabled: false,
        status: "off",
        statusMessage: "Discovery is off.",
        discoveredPeers: [],
      }), {
        profile: { display_name: "Mac Profile" },
      }, "ready"),
      discoveryResponse(initialDiscovery),
      summary("Runtime Name", initialDiscovery, {
        profile: { display_name: "Mac Profile" },
      }, "ready"),
    ]);
    await loadPeople();
    await settle();

    assert.deepEqual(environment.messages.slice(0, 2), [
      {
        message: {
          type: "home:app-ready",
          homeToken: "people-test-token",
        },
        origin: "http://localhost:61180",
      },
      {
        message: {
          type: "home:menu-manifest",
          homeToken: "people-test-token",
          menus: [
            {
              title: "File",
              items: [{ label: "Close Window", cmd: "__close-window" }],
            },
            {
              title: "View",
              items: [{ label: "Refresh", cmd: "refresh" }],
            },
          ],
        },
        origin: "http://localhost:61180",
      },
    ]);
    assert.equal(environment.calls.filter(({ path }) => path === "/api/apps/people/summary").length, 1);
    assert.equal(environment.calls.filter(({ path }) => path === "/api/apps/people/discovery/refresh").length, 0);
    assert.equal(environment.timers.size, 0, "People must not own Discovery transport polling");
    assert.equal(environment.nodes.get("profile-name").value, "Suggested Profile Name");
    assert.equal(environment.nodes.get("profile-name").placeholder, "Your name");
    assert.equal(environment.nodes.get("profile-title").textContent, "Create your Profile");
    assert.equal(environment.nodes.get("profile-submit").textContent, "Create Profile");
    assert.equal(
      environment.nodes.get("profile-description").textContent,
      "Confirm the name people will see in People and Chat. Discovery stays off until you enable it.",
    );
    assert.doesNotMatch(environment.nodes.get("people-list").innerHTML, /Person/);
    assert.doesNotMatch(environment.nodes.get("people-list").innerHTML, /Stale Profile/);
    assert.doesNotMatch(environment.nodes.get("people-list").innerHTML, /Missing contact id/);
    assert.doesNotMatch(environment.nodes.get("people-list").innerHTML, /person-card/);
    assert.match(environment.nodes.get("people-list").innerHTML, /Accepted contacts appear here/);
    assert.match(environment.nodes.get("discovery-list").innerHTML, /ESPFix/);
    // Only the person with no relationship yet is offered Add contact.
    const discoveryCards = environment.nodes.get("discovery-list").innerHTML.split("<article").slice(1);
    assert.equal(discoveryCards.length, 4);
    assert.deepEqual(
      discoveryCards.map((card) => [
        card.match(/<h4>([^<]*)<\/h4>/)[1],
        card.match(/<p class="person-details"><span>([^<]*)<\/span>/)[1],
        /data-action="discovery-request"/.test(card),
      ]),
      [
        ["ESPFix", "espfix", true],
        ["Already Added", "Contact", false],
        ["Asked Already", "Contact request sent", false],
        ["Asking Me", "Wants to add you as a contact", false],
      ],
    );
    assert.doesNotMatch(environment.nodes.get("discovery-list").innerHTML, />Request</);
    assert.match(environment.nodes.get("discovery-requests-list").innerHTML, /Open Inbox/);
    assert.match(environment.nodes.get("discovery-requests-list").innerHTML, /for your Profile are decided in Inbox/);
    assert.doesNotMatch(environment.nodes.get("discovery-requests-list").innerHTML, /Accept|Decline/);
    assert.doesNotMatch(environment.nodes.get("discovery-list").innerHTML, /must-not-render/);
    assertNoDiscoveryFallbacks(environment.nodes.get("discovery-list").innerHTML, "configured discovery list");
    assertNoDiscoveryFallbacks(
      environment.nodes.get("discovery-requests-list").innerHTML,
      "configured discovery requests",
    );
    assert.match(environment.nodes.get("discovery-status").textContent, /ten minutes/);
    assert.match(environment.nodes.get("discovery-status").textContent, /same time/);
    assertNoRawIdentityLeak(environment.nodes.get("discovery-status").textContent, "configured discovery status");

    await triggerProfileInput(environment, "Typing My Name");
    globalThis.document.activeElement = null;
    environment.replies.unshift(summary(null, initialDiscovery, {
      profile_setup_display_name: "Replacement Suggestion",
    }));
    await triggerMenuCommand(environment, "refresh");
    assert.equal(environment.nodes.get("profile-name").value, "Typing My Name");
    assert.equal(environment.nodes.get("profile-name").placeholder, "Your name");

    await triggerProfileInput(environment, "");
    globalThis.document.activeElement = null;
    environment.replies.unshift(summary(null, initialDiscovery, {
      profile_setup_display_name: "Another Suggestion",
    }));
    await triggerMenuCommand(environment, "refresh");
    assert.equal(environment.nodes.get("profile-name").value, "");
    assert.equal(environment.nodes.get("profile-name").placeholder, "Your name");

    await triggerProfileInput(environment, "Typing My Name");
    globalThis.document.activeElement = environment.nodes.get("profile-name");
    await triggerAction(environment, "discovery-refresh");
    assert.equal(environment.calls.filter(({ path }) => path === "/api/apps/people/discovery/refresh").length, 1);
    assert.equal(environment.calls.filter(({ path }) => path === "/api/apps/people/summary").length, 4);
    assert.equal(environment.timers.size, 0, "manual Refresh must only wake Runtime sync");
    assert.equal(environment.nodes.get("profile-name").value, "Typing My Name");
    assert.equal(environment.nodes.get("profile-name").placeholder, "Your name");
    assert.equal(environment.nodes.get("profile-title").textContent, "My Profile");
    assert.equal(environment.nodes.get("profile-submit").textContent, "Save");
    assert.match(environment.nodes.get("profile-description").textContent, /Shown to people/);
    assert.match(environment.nodes.get("people-list").innerHTML, /Mac/);
    assert.match(environment.nodes.get("people-list").innerHTML, />Message</);
    assert.match(environment.nodes.get("people-list").innerHTML, /data-conversation-id="direct:opaque:remote"/);
    assert.doesNotMatch(environment.nodes.get("people-list").innerHTML, /legacy-route-must-not-be-used/);
    assert.equal((environment.nodes.get("people-list").innerHTML.match(/>Message<\/button>/g) || []).length, 1);
    assert.match(environment.nodes.get("people-list").innerHTML, /No selector/);
    assert.doesNotMatch(environment.nodes.get("people-list").innerHTML, /Missing contact id/);
    assert.doesNotMatch(environment.nodes.get("people-list").innerHTML, /Stale Profile/);
    assert.match(environment.nodes.get("discovery-list").innerHTML, /No one visible right now/);
    assert.match(environment.nodes.get("discovery-requests-list").innerHTML, /No requests/);
    assertNoDiscoveryFallbacks(environment.nodes.get("discovery-list").innerHTML, "refreshed discovery list");
    assertNoDiscoveryFallbacks(
      environment.nodes.get("discovery-requests-list").innerHTML,
      "refreshed discovery requests",
    );

    const summaryCallsBeforeMenuRefresh = environment.calls.filter(
      ({ path }) => path === "/api/apps/people/summary",
    ).length;
    environment.replies.unshift(summary("Mac", refreshedDiscovery, {
      profile: { display_name: "Mac Profile" },
      profile_setup_display_name: "Do not use this suggestion",
    }, "ready"));
    await triggerMenuCommand(environment, "refresh");
    assert.equal(
      environment.calls.filter(({ path }) => path === "/api/apps/people/summary").length,
      summaryCallsBeforeMenuRefresh + 1,
    );
    const summaryCallsAfterTrustedMenuRefresh = environment.calls.filter(
      ({ path }) => path === "/api/apps/people/summary",
    ).length;
    await triggerMenuCommand(environment, "refresh", { origin: "http://localhost:61180" });
    await triggerMenuCommand(environment, "refresh", { source: {} });
    assert.equal(
      environment.calls.filter(({ path }) => path === "/api/apps/people/summary").length,
      summaryCallsAfterTrustedMenuRefresh,
    );

    globalThis.document.activeElement = null;
    await triggerAction(environment, "discovery-request", { advertisementId: "ad-1" });
    assert.equal(environment.calls.filter(({ path }) => path === "/api/apps/people/discovery/requests").length, 1);
    assert.equal(
      JSON.parse(environment.calls.find(({ path }) => path === "/api/apps/people/discovery/requests").options.body)
        .advertisement_id,
      "ad-1",
    );
    assert.equal(environment.nodes.get("profile-name").value, "Mac Profile");
    assert.equal(environment.nodes.get("profile-name").placeholder, "Your name");

    await triggerAction(environment, "open-inbox");
    assert.deepEqual(environment.messages.at(-1), {
      message: {
        type: "home:open-target",
        target: "inbox",
        query: {},
        homeToken: "people-test-token",
      },
      origin: "http://localhost:61180",
    });

    await triggerAction(environment, "chat", { conversationId: "direct:opaque:remote" });
    assert.deepEqual(environment.messages.at(-1), {
      message: {
        type: "home:open-target",
        target: "chat-room",
        query: { conversation_id: "direct:opaque:remote" },
        homeToken: "people-test-token",
      },
      origin: "http://localhost:61180",
    });

    await triggerAction(environment, "discovery-toggle", { enabled: "true" });
    const toggleCalls = environment.calls.filter(({ path }) => path === "/api/apps/people/discovery");
    assert.equal(toggleCalls.length, 1);
    assert.equal(JSON.parse(toggleCalls[0].options.body).enabled, false);
    assert.match(environment.nodes.get("discovery-status").textContent, /Discovery is off/);

    await triggerAction(environment, "discovery-toggle", { enabled: "false" });
    assert.equal(toggleCalls.length + 1, environment.calls.filter(({ path }) => path === "/api/apps/people/discovery").length);
    const secondToggle = environment.calls.filter(({ path }) => path === "/api/apps/people/discovery")[1];
    assert.equal(JSON.parse(secondToggle.options.body).enabled, true);
    assert.match(environment.nodes.get("discovery-list").innerHTML, /ESPFix/);
    assertNoDiscoveryFallbacks(environment.nodes.get("discovery-list").innerHTML, "re-enabled discovery list");

    assert.equal(environment.clearCount(), 0);
    assert.equal(environment.calls.filter(({ path }) => path === "/api/apps/people/discovery/refresh").length, 1);
    assert.equal(environment.calls.filter(({ path }) => path === "/api/apps/people/summary").length, 8);
    assert.equal(environment.replies.length, 0);
    assertRequestAuthority(environment);
    return;
  }

  if (name === "unavailable") {
    const environment = setupEnvironment(name, [
      summary(null, discoverySummary(), {
        profile_setup_display_name: "Must not become setup input",
      }, "unavailable"),
    ]);
    await loadPeople();
    await settle();
    assert.equal(environment.nodes.get("profile-name").value, "");
    assert.equal(environment.nodes.get("profile-name").placeholder, "Your name");
    assert.equal(environment.nodes.get("profile-title").textContent, "Profile unavailable");
    assert.equal(environment.nodes.get("profile-submit").textContent, "Recovery required");
    assert.equal(environment.nodes.get("profile-submit").disabled, true);
    assert.match(environment.nodes.get("profile-description").textContent, /System Recovery/);
    assert.equal(environment.calls.length, 1);
    assertRequestAuthority(environment);
    return;
  }

  if (name === "setup_suggestion_confirmed") {
    const environment = setupEnvironment(name, [
      summary(null, discoverySummary(), {
        profile_setup_display_name: "Suggested Profile Name",
      }),
      response(200, {}),
      summary("Runtime Name", discoverySummary(), {
        profile: { display_name: "Suggested Profile Name" },
      }, "ready"),
    ]);
    await loadPeople();
    await settle();
    assert.equal(environment.nodes.get("profile-name").value, "Suggested Profile Name");
    assert.equal(environment.nodes.get("profile-name").placeholder, "Your name");
    await triggerProfileSubmit(environment);
    const createCall = environment.calls.find(({ path }) => path === "/api/apps/people/profile");
    assert(createCall, "profile create call missing");
    assert.equal(
      JSON.parse(createCall.options.body).display_name,
      "Suggested Profile Name",
    );
    assert.equal(environment.nodes.get("profile-form").dataset.profileState, "saved");
    assert.equal(environment.nodes.get("profile-name").value, "Suggested Profile Name");
    assert.equal(environment.nodes.get("profile-submit").textContent, "Save");
    assertRequestAuthority(environment);
    return;
  }

  if (name === "discovery_states") {
    const environment = setupEnvironment(name, [
      summary("Runtime Name", discoverySummary({
        configured: true,
        enabled: false,
        status: "off",
        statusMessage: "Discovery is off.",
      }), {
        profile: { display_name: "Ready Profile" },
      }, "ready"),
      discoveryResponse({
        configured: true,
        enabled: false,
        status: "off_pending_expiry",
        statusMessage: "Discovery is off on this Home. It stopped advertising locally, but may remain visible for another 600 seconds.",
        remoteVisibilityMayRemainUntil: 700,
        remoteVisibilityRemainingSeconds: 600,
      }),
      summary("Runtime Name", discoverySummary({
        configured: true,
        enabled: false,
        status: "off_pending_expiry",
        statusMessage: "Discovery is off on this Home. It stopped advertising locally, but may remain visible for another 600 seconds.",
        remoteVisibilityMayRemainUntil: 700,
        remoteVisibilityRemainingSeconds: 600,
      }), {
        profile: { display_name: "Ready Profile" },
      }, "ready"),
      discoveryResponse({
        configured: true,
        enabled: false,
        status: "off",
        statusMessage: "Discovery is off.",
      }),
      summary("Runtime Name", discoverySummary({
        configured: true,
        enabled: false,
        status: "off",
        statusMessage: "Discovery is off.",
      }), {
        profile: { display_name: "Ready Profile" },
      }, "ready"),
      discoveryResponse({
        configured: true,
        enabled: false,
        status: "off",
        statusMessage: "Discovery is off.",
      }),
      summary("Runtime Name", discoverySummary({
        configured: true,
        enabled: false,
        status: "off",
        statusMessage: "Discovery is off.",
      }), {
        profile: { display_name: "Ready Profile" },
      }, "ready"),
    ]);
    await loadPeople();
    await settle();

    assert.equal(environment.nodes.get("discovery-status").textContent, "Discovery is off.");
    assert.match(
      environment.nodes.get("discovery-list").innerHTML,
      /Turn Discovery on when you want a ten-minute visibility window/,
    );

    await triggerAction(environment, "discovery-refresh");
    assert.match(environment.nodes.get("discovery-status").textContent, /another 600 seconds/);
    assert.match(environment.nodes.get("discovery-list").innerHTML, /Visibility is expiring/);

    await triggerAction(environment, "discovery-refresh");
    assert.equal(environment.nodes.get("discovery-status").textContent, "Discovery is off.");
    assert.match(
      environment.nodes.get("discovery-list").innerHTML,
      /Turn Discovery on when you want a ten-minute visibility window/,
    );

    await triggerAction(environment, "discovery-refresh");
    assert.equal(environment.nodes.get("discovery-status").textContent, "Discovery is off.");
    assert.match(
      environment.nodes.get("discovery-list").innerHTML,
      /Turn Discovery on when you want a ten-minute visibility window/,
    );
    assertNoRawIdentityLeak(environment.nodes.get("discovery-status").textContent, "off discovery status");
    return;
  }

  if (name === "discovery_reload_converges") {
    const environment = setupEnvironment(name, [
      summary("Runtime Name", discoverySummary({
        configured: true,
        enabled: false,
        status: "off",
        statusMessage: "Discovery is off.",
      }), {
        profile: { display_name: "Ready Profile" },
      }, "ready"),
    ]);
    await loadPeople();
    await settle();

    assert.equal(environment.nodes.get("discovery-status").textContent, "Discovery is off.");
    assert.match(
      environment.nodes.get("discovery-list").innerHTML,
      /Turn Discovery on when you want a ten-minute visibility window/,
    );
    assertNoRawIdentityLeak(environment.nodes.get("discovery-status").textContent, "reload off discovery status");
    return;
  }

  if (name === "profile_failure_sanitized") {
    const environment = setupEnvironment(name, [
      summary(null, discoverySummary(), {
        profile_setup_display_name: "Suggested Name",
      }),
      response(500, {
        message: "provider route failed for did:key:zExample endpoint device-1",
      }),
    ]);
    await loadPeople();
    await settle();
    environment.nodes.get("profile-name").value = "New Profile";
    await triggerProfileSubmit(environment);
    assert.equal(
      environment.nodes.get("people-status").textContent,
      "Could not create your Profile. Try again.",
    );
    assert.equal(environment.nodes.get("profile-form").dataset.profileState, "create");
    assert.equal(environment.nodes.get("profile-name").disabled, false);
    assert.equal(environment.nodes.get("profile-submit").disabled, false);
    assertNoRawIdentityLeak(environment.nodes.get("people-status").textContent, "profile failure");
    return;
  }

  if (name === "create_retry") {
    const environment = setupEnvironment(name, [
      summary(null, discoverySummary(), {
        profile_setup_display_name: "Suggested Name",
      }),
      response(503, { message: "Profile setup unavailable" }),
      response(200, {}),
      summary("Runtime Name", discoverySummary(), {
        profile: { display_name: "Recovered Profile" },
      }, "ready"),
    ]);
    await loadPeople();
    await settle();
    assert.equal(environment.calls.filter(({ path }) => path === "/api/apps/people/profile").length, 0,
      "suggestion is editable presentation until explicitly confirmed");
    environment.nodes.get("profile-name").value = "Recovered Profile";

    await triggerProfileSubmit(environment);
    assert.equal(environment.nodes.get("profile-form").dataset.profileState, "create");
    assert.equal(environment.nodes.get("profile-submit").textContent, "Create Profile");
    assert.equal(environment.nodes.get("profile-name").disabled, false);
    assert.equal(environment.nodes.get("profile-name").value, "Recovered Profile");
    assert(!environment.messages.some(({ message }) => message.type === "home:open-target"),
      "first-create errors remain in People without a recovery prerequisite");

    await triggerProfileSubmit(environment);
    assert.equal(environment.nodes.get("profile-form").dataset.profileState, "saved");
    assert.equal(environment.nodes.get("profile-title").textContent, "My Profile");
    assert.equal(environment.nodes.get("profile-name").value, "Recovered Profile");
    assert.equal(environment.replies.length, 0);
    assertRequestAuthority(environment);
    return;
  }

  assert.equal(name, "isolated");
  const environment = setupEnvironment(name, [
    summary(),
  ]);
  await loadPeople();
  await settle();
  assert.equal(environment.timers.size, 0);
  assert.match(environment.nodes.get("discovery-list").innerHTML, /Discovery unavailable/);
  assert.match(environment.nodes.get("discovery-requests-list").innerHTML, /Discovery requests are unavailable/);
  assert.equal(environment.clearCount(), 0);
  assert.equal(environment.replies.length, 0);
  assertRequestAuthority(environment);
}

const scenario = process.argv[2] || "";
if (!scenario) {
  const peopleUi = readFileSync(
    new URL("../capsules/people/browser/people.js", import.meta.url),
    "utf8",
  );
  const peopleHtml = readFileSync("capsules/people/browser/index.html", "utf8");
  assert.doesNotMatch(peopleUi, /hasVerifiedContactDisplayName/);
  assert.doesNotMatch(peopleUi, /displayName\(contact, "Person"\)/);
  assert.doesNotMatch(peopleUi, /\|\| readText\(identity\?\.profile_setup_display_name\)/);
  assert.doesNotMatch(peopleUi, /\blastDiscoveryStatus\b/);
  assert.doesNotMatch(peopleUi, /function normalizeDiscoveryStatus\(/);
  assert.doesNotMatch(peopleUi, /Visibility expired/);
  assert.doesNotMatch(peopleUi, /last visible window expired/);
  assert.doesNotMatch(peopleUi, /window\.confirm\(/);
  assert.match(peopleUi, /function isValidContact/);
  assert.match(peopleUi, /Add contact/);
  assert.match(peopleUi, /data-conversation-id/);
  assert.doesNotMatch(peopleUi, /data-contact-route|contact\?\.route|new URL\(route/);
  assert.match(peopleHtml, /id="profile-title">Create your Profile/);
  assert.match(peopleHtml, /id="profile-description">Your Profile is your signed identity for People and Chat\./);
  assert.match(peopleHtml, /id="profile-submit" type="submit">Create Profile/);
  for (const childScenario of [
    "add_by_did",
    "request_delivery_detail",
    "configured",
    "community_left",
    "remove_confirmation",
    "setup_suggestion_confirmed",
    "unavailable",
    "create_retry",
    "isolated",
    "discovery_states",
    "discovery_reload_converges",
    "profile_failure_sanitized",
  ]) {
    const child = spawnSync(
      process.execPath,
      [fileURLToPath(import.meta.url), childScenario],
      { cwd: process.cwd(), encoding: "utf8" },
    );
    assert.equal(child.status, 0, child.stderr || child.stdout);
  }
  // People's own open-target messages must pass the Home shell's source policy.
  const shellHost = readFileSync("capsules/home/browser/home-shell-host.js", "utf8");
  const peoplePolicy = shellHost.match(/^  people: new Set\(\[([^\]]*)\]\),$/m);
  assert.ok(peoplePolicy, "Home shell has no People open-target policy");
  const allowed = new Set([...peoplePolicy[1].matchAll(/"([^"]+)"/g)].map((match) => match[1]));
  const opened = [...peopleUi.matchAll(/type: "home:open-target",\s*target: "([^"]+)"/g)].map((match) => match[1]);
  assert.ok(opened.includes("inbox") && opened.includes("chat-room"), `People open targets: ${opened}`);
  for (const target of opened) {
    assert.ok(allowed.has(target), `Home shell refuses People's ${target} target`);
  }
  const inboxUi = readFileSync("capsules/inbox/browser/inbox.js", "utf8");
  assert.match(inboxUi, /contact-accept-request:/);
  assert.match(inboxUi, /createActionButton\("Accept", actionId, "primary"\)/);
  assert.match(inboxUi, /createActionButton\("Decline", "contact-decline-request:/);
  assert.match(inboxUi, /entry\.kind !== "contact_request"/);
  console.log("people-discovery-smoke: PASS");
} else {
  await runScenario(scenario);
}
