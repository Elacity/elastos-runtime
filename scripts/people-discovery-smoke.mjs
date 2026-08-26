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
      return false;
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
  if (name === "configured") {
    const initialDiscovery = discoverySummary({
      configured: true,
      enabled: true,
      discoveredPeers: [{
        advertisement_id: "ad-1",
        display_name: "ESPFix",
        handle: "espfix",
        peer_id: "must-not-render",
        connect_ticket: "must-not-render",
        topic: "must-not-render",
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
    await import("../capsules/people/browser/people.js");
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
    assert.equal(environment.nodes.get("profile-name").value, "");
    assert.equal(environment.nodes.get("profile-name").placeholder, "Suggested Profile Name");
    assert.equal(environment.nodes.get("profile-title").textContent, "Create your Profile");
    assert.equal(environment.nodes.get("profile-submit").textContent, "Create Profile");
    assert.equal(
      environment.nodes.get("profile-description").textContent,
      "Your Profile is your signed identity for People and Chat.",
    );
    assert.doesNotMatch(environment.nodes.get("people-list").innerHTML, /Person/);
    assert.doesNotMatch(environment.nodes.get("people-list").innerHTML, /Stale Profile/);
    assert.doesNotMatch(environment.nodes.get("people-list").innerHTML, /Missing contact id/);
    assert.doesNotMatch(environment.nodes.get("people-list").innerHTML, /person-card/);
    assert.match(environment.nodes.get("people-list").innerHTML, /Accepted contacts appear here/);
    assert.match(environment.nodes.get("discovery-list").innerHTML, /ESPFix/);
    assert.match(environment.nodes.get("discovery-list").innerHTML, /Add contact/);
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

    environment.nodes.get("profile-name").value = "Typing My Name";
    globalThis.document.activeElement = environment.nodes.get("profile-name");
    await triggerAction(environment, "discovery-refresh");
    assert.equal(environment.calls.filter(({ path }) => path === "/api/apps/people/discovery/refresh").length, 1);
    assert.equal(environment.calls.filter(({ path }) => path === "/api/apps/people/summary").length, 2);
    assert.equal(environment.timers.size, 0, "manual Refresh must only wake Runtime sync");
    assert.equal(environment.nodes.get("profile-name").value, "Typing My Name");
    assert.equal(environment.nodes.get("profile-name").placeholder, "Suggested Profile Name");
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
    assert.equal(environment.calls.filter(({ path }) => path === "/api/apps/people/summary").length, 6);
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
    await import("../capsules/people/browser/people.js");
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
    await import("../capsules/people/browser/people.js");
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
    await import("../capsules/people/browser/people.js");
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
    await import("../capsules/people/browser/people.js");
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

  if (name === "recovery_required") {
    const environment = setupEnvironment(name, [
      summary(null, discoverySummary(), {
        profile_setup_display_name: "Suggested Name",
      }),
      response(409, {
        schema: "elastos.people.profile-protection-required/v1",
        status: "recovery_required",
        action_target: "system",
        message: "Open System, choose Security, and download Recovery. Then retry creating your Profile.",
      }),
      response(200, {}),
      summary("Runtime Name", discoverySummary(), {
        profile: { display_name: "Recovered Profile" },
      }, "ready"),
    ]);
    await import("../capsules/people/browser/people.js");
    await settle();
    environment.nodes.get("profile-name").value = "Recovered Profile";

    await triggerProfileSubmit(environment);
    assert.equal(environment.nodes.get("profile-form").dataset.profileState, "recovery_required");
    assert.equal(environment.nodes.get("profile-title").textContent, "Recovery required");
    assert.equal(environment.nodes.get("profile-submit").textContent, "Open System");
    assert.equal(environment.nodes.get("profile-name").disabled, true);
    assert.match(environment.nodes.get("profile-description").textContent, /choose Security/);

    await triggerProfileSubmit(environment);
    assert.deepEqual(environment.messages.at(-1), {
      message: {
        type: "home:open-target",
        target: "system",
        query: {},
        homeToken: "people-test-token",
      },
      origin: "http://localhost:61180",
    });
    assert.equal(environment.nodes.get("profile-form").dataset.profileState, "retry");
    assert.equal(environment.nodes.get("profile-submit").textContent, "Retry Create Profile");
    assert.equal(environment.nodes.get("profile-name").disabled, false);

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
  await import("../capsules/people/browser/people.js");
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
  assert.match(peopleUi, /function isValidContact/);
  assert.match(peopleUi, /Add contact/);
  assert.match(peopleUi, /data-conversation-id/);
  assert.doesNotMatch(peopleUi, /data-contact-route|contact\?\.route|new URL\(route/);
  assert.match(peopleHtml, /id="profile-title">Create your Profile/);
  assert.match(peopleHtml, /id="profile-description">Your Profile is your signed identity for People and Chat\./);
  assert.match(peopleHtml, /id="profile-submit" type="submit">Create Profile/);
  for (const childScenario of [
    "configured",
    "unavailable",
    "recovery_required",
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
  const inboxUi = readFileSync("capsules/inbox/browser/index.html", "utf8");
  assert.match(inboxUi, /contact-accept-request:/);
  assert.match(inboxUi, /createActionButton\("Accept", actionId, "primary"\)/);
  assert.match(inboxUi, /createActionButton\("Decline", "contact-decline-request:/);
  assert.match(inboxUi, /entry\.kind !== "contact_request"/);
  console.log("people-discovery-smoke: PASS");
} else {
  await runScenario(scenario);
}
