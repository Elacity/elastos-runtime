#!/usr/bin/env node
// Local two-runtime People/Chat acceptance.
//
// Drives two independent Home runtimes through the collaboration journey a
// pair of people would take: sign in with passkeys, create Profiles, opt in
// to bounded Discovery, send one contact request, accept it in Inbox, message
// both ways in the direct conversation, propagate a rename, remove and re-add
// the contact, and survive a restart of both runtimes — asserting along the
// way that normal UI never shows a raw DID.
//
// Each side signs in with a persisted virtual-authenticator credential
// created by scripts/home-passkey-virtual-auth-smoke.mjs run with
// HOME_VIRTUAL_AUTH_CLEANUP=0 and HOME_VIRTUAL_AUTH_PROFILE pointing at the
// same directory passed here.
//
//   ELASTOS_A_BASE_URL=<fixture-a-origin> \
//   ELASTOS_A_PROFILE=<fixture-a-browser-profile> \
//   ELASTOS_B_BASE_URL=<fixture-b-origin> \
//   ELASTOS_B_PROFILE=<fixture-b-browser-profile> \
//   ELASTOS_A_RESTART_CMD=<fixture-a-restart-command> \
//   ELASTOS_B_RESTART_CMD=<fixture-b-restart-command> \
//   ELASTOS_A_FIXTURE_MANIFEST=<fixture-a-manifest> \
//   ELASTOS_B_FIXTURE_MANIFEST=<fixture-b-manifest> \
//   node scripts/home-two-runtime-acceptance.mjs

import { execSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { createRequire } from "node:module";

import {
  assertRecoverySetupEvidence,
  assertDistinctProfileContactEvidence,
  assertDistinctRuntimeEvidence,
  assertExactDirectConversation,
  assertFreshFixturePrecondition,
  assertIdentityFrame,
  assertRestartTransition,
  createAcceptanceReport,
  finalizeAcceptanceReport,
  loadAcceptanceConfig,
  loadRestartReceipt,
  recordAcceptancePass,
} from "./home-two-runtime-acceptance-core.mjs";

const CONFIG = (() => {
  try {
    return loadAcceptanceConfig(process.env);
  } catch (error) {
    console.error("FAIL home-two-runtime-acceptance configuration");
    console.log(JSON.stringify({
      schema: "elastos.home.two-runtime-acceptance/v2",
      ok: false,
      results: [],
      error: String(error.message || error),
    }, null, 2));
    process.exit(1);
  }
})();
const SIDE_A = CONFIG.a;
const SIDE_B = CONFIG.b;
const RENAMED_A = `${SIDE_A.name} Renamed`;

const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");

function fail(message, details) {
  const error = new Error(message);
  if (details !== undefined) {
    error.details = details;
  }
  throw error;
}

function assertOk(condition, message, details) {
  if (!condition) {
    fail(message, details);
  }
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function poll(label, timeoutMs, stepMs, fn) {
  const deadline = Date.now() + timeoutMs;
  let last;
  while (Date.now() < deadline) {
    last = await fn();
    if (last?.done) {
      return last.value;
    }
    await delay(stepMs);
  }
  fail(`timed out: ${label}`, last?.value);
}

function credentialStorePath(profileDir) {
  return join(profileDir, "elastos-virtual-authenticator-credentials.json");
}

function readCredentialStore(profileDir) {
  const path = credentialStorePath(profileDir);
  if (!existsSync(path)) {
    return [];
  }
  const parsed = JSON.parse(readFileSync(path, "utf8"));
  assertOk(
    parsed?.schema === "elastos.home.virtual-authenticator-credentials/v1"
      && Array.isArray(parsed.credentials),
    `credential store is unsupported: ${path}`,
  );
  // WebAuthn clone detection demands a strictly increasing sign counter, and
  // a replayed snapshot would sit at or below the server's stored count.
  // Timer-based counters are valid authenticator behaviour, so resume from
  // wall-clock seconds — always ahead of any prior run.
  const timerCount = Math.floor(Date.now() / 1000) - 1_767_225_600;
  return parsed.credentials.map((credential) => ({
    ...credential,
    signCount: Math.max(Number(credential.signCount) || 0, timerCount),
  }));
}

async function persistCredentials(side) {
  const { credentials } = await side.cdp.send("WebAuthn.getCredentials", {
    authenticatorId: side.authenticatorId,
  });
  mkdirSync(side.profile, { recursive: true });
  writeFileSync(
    credentialStorePath(side.profile),
    `${JSON.stringify({
      schema: "elastos.home.virtual-authenticator-credentials/v1",
      generated_at: new Date().toISOString(),
      credentials,
    }, null, 2)}\n`,
    { mode: 0o600 },
  );
  chmodSync(credentialStorePath(side.profile), 0o600);
}

async function openSide(side) {
  const context = await chromium.launchPersistentContext(side.profile, {
    acceptDownloads: true,
    headless: true,
    ignoreHTTPSErrors: true,
    viewport: { width: 1440, height: 900 },
  });
  const page = context.pages()[0] || await context.newPage();
  page.on("dialog", (dialog) => {
    dialog.accept().catch(() => {});
  });
  const consoleTail = [];
  page.on("console", (message) => {
    consoleTail.push(`${message.type()}: ${message.text().slice(0, 220)}`);
    if (consoleTail.length > 40) {
      consoleTail.shift();
    }
  });
  const netTail = [];
  page.on("response", (response) => {
    try {
      if (!response.ok() && response.url().includes("/api/")) {
        const line = `${response.status()} ${new URL(response.url()).pathname}`;
        netTail.push(line);
        if (netTail.length > 30) {
          netTail.shift();
        }
        response.text().then((body) => {
          netTail.push(`   ^ ${body.slice(0, 160)}`);
        }).catch(() => {});
      }
    } catch {}
  });
  const cdp = await context.newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  const { authenticatorId } = await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: {
      protocol: "ctap2",
      transport: "internal",
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  });
  const stored = readCredentialStore(side.profile);
  for (const credential of stored) {
    await cdp.send("WebAuthn.addCredential", { authenticatorId, credential });
  }
  return {
    ...side,
    context,
    page,
    cdp,
    authenticatorId,
    consoleTail,
    netTail,
    hasStoredCredential: stored.length > 0,
  };
}

async function ensureAccount(side) {
  if (side.hasStoredCredential) {
    await signIn(side);
    return;
  }
  // First passkey on a fresh Home becomes the admin.
  await side.page.goto(`${side.base}/apps/home/`, { waitUntil: "domcontentloaded" });
  const name = side.page.locator("#home-unlock-name");
  await name.waitFor({ state: "visible", timeout: 30_000 });
  await name.fill(`${side.name} Admin`);
  const registered = side.page.waitForResponse((response) => (
    response.request().method() === "POST"
      && response.url().endsWith("/api/auth/passkey/register/complete")
  ), { timeout: 30_000 });
  registered.catch(() => {});
  await side.page.evaluate(() => {
    document.querySelector("#home-unlock-primary")?.click();
  });
  const completion = await registered;
  assertOk(completion.ok(), `${side.prefix}: passkey registration failed`, {
    status: completion.status(),
  });
  await persistCredentials(side);
  side.hasStoredCredential = true;
  await signIn(side);
}

async function signIn(side) {
  const homeUrl = `${side.base}/apps/home/`;
  await side.page.goto(homeUrl, { waitUntil: "domcontentloaded" });
  await poll(`${side.prefix}: signed Home`, 30_000, 500, async () => {
    const state = await side.page.evaluate(() => ({
      status: document.body?.dataset?.homeStatus || "",
      authority: document.body?.dataset?.homeAuthority || "",
      unlockVisible: document.querySelector("#home-unlock")?.hidden === false,
      unlockPrimary: document.querySelector("#home-unlock-primary")?.textContent?.trim() || "",
    })).catch(() => null);
    if (!state) {
      return { done: false };
    }
    if (state.authority === "signed" && state.status === "ready") {
      return { done: true, value: state };
    }
    if (state.unlockVisible && /passkey/i.test(state.unlockPrimary)) {
      await side.page.evaluate(() => {
        document.querySelector("#home-unlock-primary")?.click();
      }).catch(() => {});
    }
    return { done: false, value: state };
  });
}

function capsuleFrame(side, target) {
  const origin = new URL(side.base).origin;
  return side.page.frames().find((frame) => {
    try {
      const url = new URL(frame.url());
      return url.origin === origin && url.pathname.startsWith(`/apps/${target}/`);
    } catch {
      return false;
    }
  }) || null;
}

function recoveryDownloadPath(side) {
  return join(
    side.fixture.dataRoot,
    "acceptance-recovery",
    `${side.fixture.fixtureId}.json`,
  );
}

async function waitForFrame(side, target, timeoutMs = 30_000) {
  const frame = await poll(`${side.prefix}: ${target} frame`, timeoutMs, 200, async () => {
    const found = capsuleFrame(side, target);
    return found ? { done: true, value: found } : { done: false };
  });
  await frame.waitForFunction(() => Boolean(document.body), null, { timeout: 15_000 });
  return frame;
}

async function openAppWindow(side, target) {
  await signIn(side);
  const homeGuiFrame = await waitForFrame(side, "home-gui");
  await homeGuiFrame.locator("#launcher-toggle").click();
  const card = homeGuiFrame.locator(`#launcher-grid [data-target="${target}"]`).first();
  await card.waitFor({ state: "visible", timeout: 10_000 });
  // Summary refreshes rebuild the launcher grid, which detaches cards
  // between actionability checks; dispatch the click on the current node.
  await homeGuiFrame.evaluate((appTarget) => {
    document.querySelector(`#launcher-grid [data-target="${appTarget}"]`)?.click();
  }, target);
  const windowFrameEl = homeGuiFrame
    .locator(`section.window[data-target="${target}"] iframe.window-frame`)
    .last();
  await windowFrameEl.waitFor({ state: "visible", timeout: 20_000 });
  const handle = await windowFrameEl.elementHandle();
  const appFrame = handle ? await handle.contentFrame() : null;
  assertOk(appFrame, `${side.prefix}: desktop window for ${target} had no content frame`);
  await poll(`${side.prefix}: ${target} document`, 20_000, 100, async () => (
    { done: appFrame.url().includes(`/apps/${target}/`) }
  ));
  await appFrame.waitForFunction(() => Boolean(document.body), null, { timeout: 15_000 });
  return appFrame;
}

async function systemDeviceDid(side) {
  const frame = await openAppWindow(side, "system");
  const deviceDid = await frame.evaluate(async () => {
    const token = new URLSearchParams(window.location.hash.replace(/^#/, ""))
      .get("home_token") || "";
    if (!token) {
      return "";
    }
    const response = await fetch("/api/apps/system/summary", {
      credentials: "same-origin",
      headers: { "x-elastos-home-token": token },
    });
    if (!response.ok) {
      return "";
    }
    const summary = await response.json();
    return typeof summary?.identity?.device_did === "string"
      ? summary.identity.device_did.trim()
      : "";
  });
  assertOk(deviceDid, `${side.prefix}: authorized System summary has no device identity`);
  return deviceDid;
}

async function peopleSnapshot(frame) {
  return frame.evaluate(() => {
    const text = (node) => (node?.textContent || "").replace(/\s+/g, " ").trim();
    const cards = (root) => [...(root?.querySelectorAll(".person-card") || [])].map((card) => ({
      text: text(card),
      actions: [...card.querySelectorAll("[data-action]")].map((button) => ({
        action: button.dataset.action,
        advertisementId: button.dataset.advertisementId || "",
        contactId: button.dataset.contactId || "",
        conversationId: button.dataset.conversationId || "",
        disabled: button.disabled,
      })),
    }));
    return {
      profileTitle: text(document.querySelector("#profile-title")),
      profileValue: document.querySelector("#profile-name")?.value || "",
      status: text(document.querySelector("#people-status")),
      discoveryHidden: document.querySelector("#discovery")?.hidden !== false,
      discoveryStatus: text(document.querySelector("#discovery-status")),
      discoveryToggle: text(document.querySelector("#discovery-toggle")),
      contacts: cards(document.querySelector("#people-list")),
      discovered: cards(document.querySelector("#discovery-list")),
      requests: cards(document.querySelector("#discovery-requests-list")),
      bodyText: (document.body?.innerText || "").replace(/\s+/g, " ").trim(),
    };
  });
}

async function peopleReadiness(frame) {
  return frame.evaluate(async () => {
    const token = new URLSearchParams(window.location.hash.replace(/^#/, ""))
      .get("home_token") || "";
    if (!token) {
      return { status: "", schema: "" };
    }
    const response = await fetch("/api/apps/people/summary", {
      credentials: "same-origin",
      headers: { "x-elastos-home-token": token },
    });
    if (!response.ok) {
      return { status: "", schema: "" };
    }
    const summary = await response.json();
    const readiness = summary?.identity?.profile_readiness;
    return {
      schema: typeof readiness?.schema === "string" ? readiness.schema.trim() : "",
      status: typeof readiness?.status === "string" ? readiness.status.trim() : "",
    };
  });
}

async function completeRecoverySetup(side, peopleFrame) {
  const before = await peopleReadiness(peopleFrame);
  assertOk(
    before.schema === "elastos.profile.readiness/v1" && before.status === "setup_required",
    `${side.prefix}: fresh Home did not begin in setup_required Profile readiness`,
    before,
  );
  await peopleFrame.locator("#profile-name").fill(side.name);
  const blockedSave = side.page.waitForResponse((response) => (
    response.request().method() === "POST"
      && response.url().endsWith("/api/apps/people/profile")
  ), { timeout: 30_000 });
  blockedSave.catch(() => {});
  await peopleFrame.evaluate(() => {
    document.querySelector("#profile-submit")?.click();
  });
  const blockedResponse = await blockedSave;
  const blockedBody = await blockedResponse.json().catch(() => ({}));
  assertOk(
    blockedResponse.status() === 409
      && blockedBody?.schema === "elastos.people.profile-protection-required/v1"
      && blockedBody?.status === "recovery_required"
      && blockedBody?.action_target === "system",
    `${side.prefix}: first Profile save did not fail with the exact Recovery-required result`,
    {
      status: blockedResponse.status(),
      body: blockedBody,
    },
  );
  const systemFramePromise = waitForFrame(side, "system", 30_000);
  await peopleFrame.evaluate(() => {
    document.querySelector("#profile-submit")?.click();
  });
  const systemFrame = await systemFramePromise;
  await systemFrame.evaluate(() => {
    const button = document.querySelector('button[data-settings="security"]');
    if (!(button instanceof HTMLButtonElement)) {
      throw new Error("System security button is missing");
    }
    button.click();
  });
  await systemFrame.locator('button[data-settings="security"].active').waitFor({
    state: "visible",
    timeout: 10_000,
  });
  await systemFrame.locator('#recovery-download').waitFor({
    state: "visible",
    timeout: 10_000,
  });
  const downloadTarget = recoveryDownloadPath(side);
  mkdirSync(join(side.fixture.dataRoot, "acceptance-recovery"), {
    recursive: true,
    mode: 0o700,
  });
  const downloadPromise = side.page.waitForEvent("download", { timeout: 45_000 });
  await systemFrame.evaluate(() => {
    const button = document.querySelector("#recovery-download");
    if (!(button instanceof HTMLButtonElement)) {
      throw new Error("System Recovery download button is missing");
    }
    button.click();
  });
  const download = await downloadPromise;
  await download.saveAs(downloadTarget);
  chmodSync(downloadTarget, 0o600);
  const bundle = JSON.parse(readFileSync(downloadTarget, "utf8"));
  assertOk(
    bundle?.schema === "elastos.full-recovery-bundle/v1",
    `${side.prefix}: Recovery download did not produce the expected bundle`,
    { schema: bundle?.schema || "", suggested: download.suggestedFilename() },
  );
  const after = await poll(`${side.prefix}: Recovery changes Profile readiness`, 30_000, 500, async () => {
    const current = await peopleReadiness(peopleFrame);
    return {
      done: current.schema === "elastos.profile.readiness/v1" && current.status === "setup_required",
      value: current,
    };
  });
  return {
    download_count: 1,
    download_path: downloadTarget,
    before_status: before.status,
    blocked_status: blockedBody.status,
    after_status: after.status,
  };
}

async function saveProfile(side, frame, name) {
  await frame.evaluate(() => {
    document.querySelector('[data-section-target="people"]')?.click();
  });
  await frame.locator("#profile-name").waitFor({ state: "visible", timeout: 10_000 });
  await frame.locator("#profile-name").fill(name);
  await frame.evaluate(() => {
    document.querySelector("#profile-submit")?.click();
  });
  // The field holds whatever we typed, so it proves nothing on its own —
  // wait for People's own confirmation that the runtime accepted the save.
  await poll(`${side.prefix}: profile saved as "${name}"`, 30_000, 500, async () => {
    const snapshot = await peopleSnapshot(frame);
    return {
      done: snapshot.profileValue === name && /saved|created/i.test(snapshot.status || ""),
      value: snapshot,
    };
  });
}

async function enableDiscovery(side, frame) {
  // Discovery lives behind the People sidebar's Discovery tab.
  await frame.evaluate(() => {
    document.querySelector('[data-section-target="discovery"]')?.click();
  });
  return poll(`${side.prefix}: discovery enabled`, 45_000, 1_500, async () => {
    const snapshot = await peopleSnapshot(frame);
    if (/turn off/i.test(snapshot.discoveryToggle)) {
      return { done: true, value: snapshot };
    }
    await frame.evaluate(() => {
      document.querySelector("#discovery-toggle")?.click();
    });
    return { done: false, value: snapshot };
  });
}

async function refreshDiscovery(frame) {
  await frame.evaluate(() => {
    document.querySelector("#discovery-refresh")?.click();
  });
}

async function requestContact(side, frame, peerName) {
  let clicked = false;
  return poll(`${side.prefix}: exactly one request sent to ${peerName}`, 180_000, 4_000, async () => {
    const snapshot = await peopleSnapshot(frame);
    const requested = snapshot.contacts.filter((card) => (
      card.text.includes(peerName) && /Request sent|Requested/i.test(card.text)
    ));
    assertOk(requested.length <= 1, `${side.prefix}: duplicate outgoing contact requests`, requested);
    if (requested.length === 1) {
      return { done: true, value: { count: 1 } };
    }
    if (!clicked) {
      const peers = snapshot.discovered.filter((card) => card.text.includes(peerName));
      assertOk(peers.length <= 1, `${side.prefix}: ambiguous Discovery result for ${peerName}`, peers);
      const request = peers[0]?.actions.find((action) => action.action === "discovery-request");
      if (!request || request.disabled) {
        await refreshDiscovery(frame);
        return { done: false, value: snapshot.discovered };
      }
      await frame.evaluate((advertisementId) => {
        document
          .querySelector(`[data-action="discovery-request"][data-advertisement-id="${advertisementId}"]`)
          ?.click();
      }, request.advertisementId);
      clicked = true;
    }
    return { done: false, value: snapshot.contacts };
  });
}

async function acceptContactRequest(side, frame, peerName) {
  await poll(`${side.prefix}: accepted request from ${peerName}`, 120_000, 3_000, async () => {
    const result = await frame.evaluate((name) => {
      const entries = [...document.querySelectorAll("#entry-list article, #entry-list li, #entry-list section")];
      const scope = entries.length > 0 ? entries : [document.querySelector("#entry-list")].filter(Boolean);
      const matches = scope.flatMap((entry) => {
        const text = (entry.textContent || "").replace(/\s+/g, " ");
        if (!text.includes(name)) {
          return [];
        }
        const accept = [...entry.querySelectorAll("button")]
          .find((button) => button.textContent?.trim() === "Accept");
        return accept ? [{ accept }] : [];
      });
      if (matches.length === 1) {
        matches[0].accept.click();
      }
      return { count: matches.length, clicked: matches.length === 1 };
    }, peerName);
    assertOk(result.count <= 1, `${side.prefix}: ambiguous Inbox contact request`, result);
    return { done: result.clicked, value: result };
  });
}

async function assertPeopleHasNoDecisionActions(side, frame) {
  const actions = await frame.evaluate(() => [...document.querySelectorAll("button")]
    .map((button) => button.textContent?.trim() || "")
    .filter((label) => label === "Accept" || label === "Decline"));
  assertOk(actions.length === 0, `${side.prefix}: People exposed a contact decision action`, actions);
}

async function contactConnected(frame, peerName) {
  const snapshot = await peopleSnapshot(frame);
  const contact = snapshot.contacts.find((card) => card.text.includes(peerName));
  return Boolean(
    contact?.actions.some((action) => action.action === "chat" && action.conversationId),
  );
}

async function waitForContact(side, frame, peerName, timeoutMs = 120_000) {
  return poll(`${side.prefix}: contact "${peerName}" connected`, timeoutMs, 3_000, async () => {
    const snapshot = await peopleSnapshot(frame);
    const contact = snapshot.contacts.find((card) => card.text.includes(peerName));
    const chat = contact?.actions.find((action) => action.action === "chat" && action.conversationId);
    const contactId = contact?.actions.find((action) => action.contactId)?.contactId || "";
    if (contact && chat && contactId) {
      return { done: true, value: { contact, contactId, conversationId: chat.conversationId } };
    }
    await refreshDiscovery(frame).catch(() => {});
    return { done: false, value: snapshot.contacts };
  });
}

async function openConversation(side, peopleFrame, peerName, expectedConversationId = null) {
  const contact = await waitForContact(side, peopleFrame, peerName);
  const conversationId = expectedConversationId || contact.conversationId;
  assertOk(
    contact.conversationId === conversationId,
    `${side.prefix}: accepted contact conversation id changed`,
  );
  await peopleFrame.evaluate((id) => {
    const exact = [...document.querySelectorAll('[data-action="chat"][data-conversation-id]')]
      .find((node) => node.dataset.conversationId === id);
    exact?.click();
  }, conversationId);
  const homeGuiFrame = await waitForFrame(side, "home-gui");
  const windowFrameEl = homeGuiFrame
    .locator('section.window[data-target="chat-room"] iframe.window-frame')
    .last();
  try {
    await windowFrameEl.waitFor({ state: "visible", timeout: 30_000 });
  } catch (error) {
    fail(`${side.prefix}: chat window never opened`, {
      console: side.consoleTail.slice(-15),
    });
  }
  const handle = await windowFrameEl.elementHandle();
  const chatFrame = handle ? await handle.contentFrame() : null;
  assertOk(chatFrame, `${side.prefix}: chat window had no content frame`);
  await poll(`${side.prefix}: chat document`, 20_000, 200, async () => (
    { done: chatFrame.url().includes("/apps/chat-room/") }
  ));
  await chatFrame.waitForFunction(() => Boolean(document.body), null, { timeout: 15_000 });
  await chatFrame.waitForFunction(() => {
    const input = document.querySelector("#message-input");
    return input && !input.disabled;
  }, null, { timeout: 60_000 });
  await chatFrame.evaluate((id) => {
    const exact = [...document.querySelectorAll("[data-conversation-choice]")]
      .find((node) => node.dataset.conversationChoice === id);
    exact?.click();
  }, conversationId);
  const selection = await poll(`${side.prefix}: exact direct conversation selected`, 30_000, 250, async () => {
    const state = await chatFrame.evaluate(() => ({
      availableConversationIds: [...document.querySelectorAll("[data-conversation-choice]")]
        .map((node) => node.dataset.conversationChoice || ""),
      selectedConversationId: document.querySelector("[data-conversation-choice].active")
        ?.dataset?.conversationChoice || "",
      chatMode: document.body?.dataset?.chatMode || "",
    }));
    try {
      assertExactDirectConversation({
        expectedConversationId: conversationId,
        ...state,
      });
      return { done: true, value: state };
    } catch {
      return { done: false, value: state };
    }
  });
  assertExactDirectConversation({ expectedConversationId: conversationId, ...selection });
  return { frame: chatFrame, conversationId };
}

async function selectSharedConversation(side, chatFrame) {
  await chatFrame.evaluate(() => {
    document.querySelector('[data-conversation-choice="shared"]')?.click();
  });
  await poll(`${side.prefix}: Shared room selected`, 30_000, 250, async () => {
    const state = await chatFrame.evaluate(() => ({
      selected: document.querySelector("[data-conversation-choice].active")
        ?.dataset?.conversationChoice || "",
      chatMode: document.body?.dataset?.chatMode || "",
      inputDisabled: document.querySelector("#message-input")?.disabled ?? true,
    }));
    return {
      done: state.selected === "shared" && state.chatMode === "shared" && !state.inputDisabled,
      value: state,
    };
  });
}

async function openSharedConversation(side) {
  const chatFrame = await openAppWindow(side, "chat-room");
  await selectSharedConversation(side, chatFrame);
  return chatFrame;
}

async function chatFrameState(chatFrame) {
  return chatFrame.evaluate(() => ({
    chatMode: document.body?.dataset?.chatMode || "",
    selectedConversationId: document.querySelector("[data-conversation-choice].active")
      ?.dataset?.conversationChoice || "",
    title: document.querySelector("#participant-count")?.textContent?.trim() || "",
    inputDisabled: document.querySelector("#message-input")?.disabled ?? true,
    inputValue: document.querySelector("#message-input")?.value || "",
    sendDisabled: document.querySelector("#send-button")?.disabled ?? true,
    messagesTail: (document.querySelector("#message-list")?.textContent || "").slice(-400),
    selectorText: (document.querySelector("#conversation-selector")?.textContent || "").slice(0, 200),
  })).catch((error) => ({ error: String(error).slice(0, 200) }));
}

async function sendMessage(side, chatFrame, text) {
  await chatFrame.locator("#message-input").fill(text);
  await chatFrame.evaluate(() => {
    document.querySelector("#send-button")?.click();
  });
  try {
    await poll(`${side.prefix}: sent "${text}"`, 45_000, 1_000, async () => {
      const listed = await chatFrame.evaluate((needle) => (
        (document.querySelector("#message-list")?.textContent || "").includes(needle)
      ), text);
      return { done: listed };
    });
    const sent = await chatFrameState(chatFrame);
    console.error(`[acceptance] ${side.prefix}: post-send tail: ${sent.messagesTail.slice(-160)}`);
  } catch (error) {
    fail(`${side.prefix}: message never rendered after send`, {
      chat: await chatFrameState(chatFrame),
      console: side.consoleTail.slice(-12),
      network: side.netTail.slice(-14),
    });
  }
}

async function waitForMessage(side, chatFrame, text, timeoutMs = 300_000) {
  try {
    await poll(`${side.prefix}: received "${text}"`, timeoutMs, 2_000, async () => {
      const listed = await chatFrame.evaluate((needle) => (
        (document.querySelector("#message-list")?.textContent || "").includes(needle)
      ), text);
      return { done: listed };
    });
  } catch (error) {
    fail(`${side.prefix}: message never arrived`, {
      expected: text,
      chat: await chatFrameState(chatFrame),
      console: side.consoleTail.slice(-10),
      network: side.netTail.slice(-10),
    });
  }
}

async function removeContact(side, frame, peerName) {
  const snapshot = await peopleSnapshot(frame);
  const contact = snapshot.contacts.find((card) => card.text.includes(peerName));
  const remove = contact?.actions.find((action) => action.action === "remove");
  assertOk(remove?.contactId, `${side.prefix}: no removable contact for ${peerName}`, snapshot.contacts);
  await frame.evaluate((contactId) => {
    document.querySelector(`[data-action="remove"][data-contact-id="${contactId}"]`)?.click();
  }, remove.contactId);
  await poll(`${side.prefix}: contact ${peerName} removed`, 30_000, 1_000, async () => {
    const current = await peopleSnapshot(frame);
    const still = current.contacts.find((card) => card.text.includes(peerName));
    const connected = still?.actions.some((action) => action.action === "remove");
    return { done: !connected, value: current.contacts };
  });
}

async function waitForBilateralRemoval(a, aPeople, aPeerName, b, bPeople, bPeerName) {
  await poll("bilateral contact removal", 120_000, 2_000, async () => {
    const [aSnapshot, bSnapshot] = await Promise.all([
      peopleSnapshot(aPeople),
      peopleSnapshot(bPeople),
    ]);
    const removed = (snapshot, peerName) => {
      const contact = snapshot.contacts.find((card) => card.text.includes(peerName));
      return Boolean(
        contact
        && /Removed|No longer connected/i.test(contact.text)
        && !contact.actions.some((action) => action.action === "chat" || action.action === "remove"),
      );
    };
    const done = removed(aSnapshot, aPeerName) && removed(bSnapshot, bPeerName);
    if (!done) {
      await Promise.all([
        refreshDiscovery(aPeople).catch(() => {}),
        refreshDiscovery(bPeople).catch(() => {}),
      ]);
    }
    return {
      done,
      value: { a: aSnapshot.contacts, b: bSnapshot.contacts },
    };
  });
  assertOk(!(await contactConnected(aPeople, aPeerName)), `${a.prefix}: removed contact still connected`);
  assertOk(!(await contactConnected(bPeople, bPeerName)), `${b.prefix}: removed contact still connected`);
}

async function identityFrameEvidence(side, target, frame, requiredTexts) {
  const evidence = await poll(`${side.prefix}: ${target} identity evidence`, 15_000, 250, async () => {
    const current = await frame.evaluate(() => ({
      frameUrl: window.location.href,
      text: [
        (document.body?.innerText || "").replace(/\s+/g, " ").trim(),
        ...[...document.querySelectorAll("input, textarea, select")]
          .map((node) => {
            if (node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement) {
              return node.value || "";
            }
            if (node instanceof HTMLSelectElement) {
              return node.value || "";
            }
            return "";
          })
          .map((value) => value.replace(/\s+/g, " ").trim())
          .filter(Boolean),
      ].filter(Boolean).join(" "),
    }));
    return {
      done: requiredTexts.every((text) => current.text.includes(text)),
      value: current,
    };
  });
  const scan = assertIdentityFrame({
    baseUrl: side.base,
    target,
    ...evidence,
  });
  const missing = requiredTexts.filter((text) => !evidence.text.includes(text));
  assertOk(missing.length === 0, `${side.prefix}: ${target} frame lacked required acceptance evidence`, missing);
  return { ...scan, required_texts: requiredTexts.length };
}

async function runLeg(report, leg, label, operation) {
  console.error(`[acceptance] ${label}`);
  const evidence = await operation();
  recordAcceptancePass(report, leg, evidence || {});
  return evidence;
}

async function main() {
  const report = createAcceptanceReport(CONFIG);
  let a;
  let b;
  try {
    a = await openSide(SIDE_A);
    b = await openSide(SIDE_B);

    await runLeg(report, "provisioning_and_sign_in", "provision and sign in both fixture Homes", async () => {
      await ensureAccount(a);
      await ensureAccount(b);
      return { a: "signed", b: "signed" };
    });

    let aDeviceDid;
    let bDeviceDid;
    await runLeg(report, "distinct_runtime_instances", "prove two distinct fixture Runtimes in System", async () => {
      [aDeviceDid, bDeviceDid] = await Promise.all([
        systemDeviceDid(a),
        systemDeviceDid(b),
      ]);
      assertDistinctRuntimeEvidence(aDeviceDid, bDeviceDid, CONFIG);
      return { a_device_did: aDeviceDid, b_device_did: bDeviceDid };
    });

    let aPeople = await openAppWindow(a, "people");
    let bPeople = await openAppWindow(b, "people");
    await runLeg(report, "fresh_fixture_precondition", "require fresh contact state", async () => {
      const [aSnapshot, bSnapshot] = await Promise.all([
        peopleSnapshot(aPeople),
        peopleSnapshot(bPeople),
      ]);
      assertFreshFixturePrecondition(aSnapshot.contacts, bSnapshot.contacts);
      return { a_contacts: 0, b_contacts: 0 };
    });

    await runLeg(report, "system_recovery_before_profile", "complete System Recovery on both fresh Homes", async () => {
      const evidence = {
        a: await completeRecoverySetup(a, aPeople),
        b: await completeRecoverySetup(b, bPeople),
      };
      return assertRecoverySetupEvidence(CONFIG, evidence);
    });

    await runLeg(report, "distinct_profile_names", "save two distinct Profile names", async () => {
      await saveProfile(a, aPeople, SIDE_A.name);
      await saveProfile(b, bPeople, SIDE_B.name);
      const [aSnapshot, bSnapshot] = await Promise.all([
        peopleSnapshot(aPeople),
        peopleSnapshot(bPeople),
      ]);
      assertOk(aSnapshot.profileValue === SIDE_A.name, "A Profile name was not retained");
      assertOk(bSnapshot.profileValue === SIDE_B.name, "B Profile name was not retained");
      assertOk(aSnapshot.profileValue !== bSnapshot.profileValue, "fixture Profiles are not distinct");
      return { a_name: aSnapshot.profileValue, b_name: bSnapshot.profileValue };
    });

    await runLeg(report, "overlapping_opt_in_discovery", "enable bounded Discovery on both Homes", async () => {
      const [aDiscovery, bDiscovery] = await Promise.all([
        enableDiscovery(a, aPeople),
        enableDiscovery(b, bPeople),
      ]);
      return {
        a_enabled: /turn off/i.test(aDiscovery.discoveryToggle),
        b_enabled: /turn off/i.test(bDiscovery.discoveryToggle),
      };
    });

    await runLeg(report, "exactly_one_contact_request", "A sends exactly one contact request", async () => {
      await assertPeopleHasNoDecisionActions(b, bPeople);
      const evidence = await requestContact(a, aPeople, SIDE_B.name);
      assertOk(evidence.count === 1, "outgoing request evidence was not exact", evidence);
      await assertPeopleHasNoDecisionActions(b, bPeople);
      return evidence;
    });

    await runLeg(report, "inbox_only_accept", "B accepts only in Inbox", async () => {
      const bInbox = await openAppWindow(b, "inbox");
      await acceptContactRequest(b, bInbox, SIDE_A.name);
      bPeople = await openAppWindow(b, "people");
      await assertPeopleHasNoDecisionActions(b, bPeople);
      return { decision_surface: "inbox" };
    });

    let conversationId;
    let aContactId;
    let bContactId;
    await runLeg(report, "stable_contacts", "stable accepted contact on both Homes", async () => {
      const [aContact, bContact] = await Promise.all([
        waitForContact(a, aPeople, SIDE_B.name),
        waitForContact(b, bPeople, SIDE_A.name),
      ]);
      assertOk(
        aContact.conversationId === bContact.conversationId,
        "accepted contacts disagree on the opaque conversation id",
      );
      conversationId = aContact.conversationId;
      aContactId = aContact.contactId;
      bContactId = bContact.contactId;
      return { conversation_id: conversationId };
    });

    await runLeg(report, "distinct_profile_identities", "prove distinct Profile identities through opaque contacts", async () => {
      assertDistinctProfileContactEvidence(aContactId, bContactId);
      return { a_contact_id: aContactId, b_contact_id: bContactId };
    });

    const aDirect = await openConversation(a, aPeople, SIDE_B.name, conversationId);
    const bDirect = await openConversation(b, bPeople, SIDE_A.name, conversationId);
    const helloFromA = `Hello from ${SIDE_A.name} @ ${Date.now()}`;
    await runLeg(report, "direct_message_a_to_b", "direct message A to B", async () => {
      await sendMessage(a, aDirect.frame, helloFromA);
      await waitForMessage(b, bDirect.frame, helloFromA);
      return { conversation_id: conversationId, message: helloFromA };
    });

    const helloFromB = `Hello back from ${SIDE_B.name} @ ${Date.now()}`;
    await runLeg(report, "direct_message_b_to_a", "direct message B to A", async () => {
      await sendMessage(b, bDirect.frame, helloFromB);
      await waitForMessage(a, aDirect.frame, helloFromB);
      return { conversation_id: conversationId, message: helloFromB };
    });

    await runLeg(report, "rename_propagation", "signed Profile rename propagates", async () => {
      await saveProfile(a, aPeople, RENAMED_A);
      await poll("B sees A's rename", 180_000, 4_000, async () => {
        const snapshot = await peopleSnapshot(bPeople);
        const renamed = snapshot.contacts.some((card) => card.text.includes(RENAMED_A));
        if (!renamed) {
          await refreshDiscovery(bPeople).catch(() => {});
        }
        return { done: renamed, value: snapshot.contacts };
      });
      return { display_name: RENAMED_A };
    });

    await runLeg(report, "bilateral_removal", "signed removal is visible on both Homes", async () => {
      await removeContact(b, bPeople, RENAMED_A);
      await waitForBilateralRemoval(a, aPeople, SIDE_B.name, b, bPeople, RENAMED_A);
      return { a_removed: true, b_removed: true };
    });

    await runLeg(report, "re_add_contact", "re-add through one request and Inbox acceptance", async () => {
      await Promise.all([
        enableDiscovery(a, aPeople),
        enableDiscovery(b, bPeople),
      ]);
      const request = await requestContact(b, bPeople, RENAMED_A);
      assertOk(request.count === 1, "re-add request evidence was not exact", request);
      await assertPeopleHasNoDecisionActions(a, aPeople);
      const aInbox = await openAppWindow(a, "inbox");
      await acceptContactRequest(a, aInbox, SIDE_B.name);
      aPeople = await openAppWindow(a, "people");
      bPeople = await openAppWindow(b, "people");
      const [aContact, bContact] = await Promise.all([
        waitForContact(a, aPeople, SIDE_B.name),
        waitForContact(b, bPeople, RENAMED_A),
      ]);
      assertOk(
        aContact.conversationId === conversationId && bContact.conversationId === conversationId,
        "re-add changed the stable direct conversation id",
      );
      return { conversation_id: conversationId, request_count: 1, decision_surface: "inbox" };
    });

    const sharedMarker = `Shared continuity @ ${Date.now()}`;
    await runLeg(report, "shared_room_before_restart", "shared-room message before restart", async () => {
      const [aShared, bShared] = await Promise.all([
        openSharedConversation(a),
        openSharedConversation(b),
      ]);
      await sendMessage(a, aShared, sharedMarker);
      await waitForMessage(b, bShared, sharedMarker);
      return { message: sharedMarker, selected: "shared" };
    });

    await runLeg(report, "both_runtime_restart", "restart both fixture Runtimes", async () => {
      const aBefore = loadRestartReceipt(SIDE_A);
      const bBefore = loadRestartReceipt(SIDE_B);
      execSync(SIDE_A.restartCmd, {
        stdio: "inherit",
        shell: "/bin/bash",
        timeout: 120_000,
      });
      execSync(SIDE_B.restartCmd, {
        stdio: "inherit",
        shell: "/bin/bash",
        timeout: 120_000,
      });
      await delay(5_000);
      await signIn(a);
      await signIn(b);
      const aAfter = loadRestartReceipt(SIDE_A);
      const bAfter = loadRestartReceipt(SIDE_B);
      const [aDeviceDidAfter, bDeviceDidAfter] = await Promise.all([
        systemDeviceDid(a),
        systemDeviceDid(b),
      ]);
      const aRestart = assertRestartTransition({
        before: aBefore,
        after: aAfter,
        side: SIDE_A,
        systemDeviceDid: aDeviceDidAfter,
      });
      const bRestart = assertRestartTransition({
        before: bBefore,
        after: bAfter,
        side: SIDE_B,
        systemDeviceDid: bDeviceDidAfter,
      });
      assertOk(
        aDeviceDidAfter === aDeviceDid && bDeviceDidAfter === bDeviceDid,
        "Runtime restart changed a stable device identity",
      );
      aPeople = await openAppWindow(a, "people");
      bPeople = await openAppWindow(b, "people");
      return { a: aRestart, b: bRestart };
    });

    let aDirectAfterRestart;
    let bDirectAfterRestart;
    await runLeg(report, "direct_history_after_restart", "direct history survives both restarts", async () => {
      await Promise.all([
        waitForContact(a, aPeople, SIDE_B.name),
        waitForContact(b, bPeople, RENAMED_A),
      ]);
      aDirectAfterRestart = await openConversation(a, aPeople, SIDE_B.name, conversationId);
      bDirectAfterRestart = await openConversation(b, bPeople, RENAMED_A, conversationId);
      await Promise.all([
        waitForMessage(a, aDirectAfterRestart.frame, helloFromB, 60_000),
        waitForMessage(b, bDirectAfterRestart.frame, helloFromA, 60_000),
      ]);
      return { conversation_id: conversationId, a_history: true, b_history: true };
    });

    await runLeg(report, "shared_room_after_restart", "shared-room history survives both restarts", async () => {
      const [aShared, bShared] = await Promise.all([
        openSharedConversation(a),
        openSharedConversation(b),
      ]);
      await Promise.all([
        waitForMessage(a, aShared, sharedMarker, 60_000),
        waitForMessage(b, bShared, sharedMarker, 60_000),
      ]);
      return { message: sharedMarker, a_history: true, b_history: true };
    });

    aPeople = await openAppWindow(a, "people");
    bPeople = await openAppWindow(b, "people");
    await runLeg(report, "identity_scan_people_a", "scan the actual nonempty A People frame", async () => (
      identityFrameEvidence(a, "people", aPeople, [RENAMED_A, SIDE_B.name])
    ));
    await runLeg(report, "identity_scan_people_b", "scan the actual nonempty B People frame", async () => (
      identityFrameEvidence(b, "people", bPeople, [SIDE_B.name, RENAMED_A])
    ));

    aDirectAfterRestart = await openConversation(a, aPeople, SIDE_B.name, conversationId);
    bDirectAfterRestart = await openConversation(b, bPeople, RENAMED_A, conversationId);
    await runLeg(report, "identity_scan_chat_a", "scan the actual nonempty A direct Chat frame", async () => {
      await waitForMessage(a, aDirectAfterRestart.frame, helloFromB, 60_000);
      return identityFrameEvidence(
        a,
        "chat-room",
        aDirectAfterRestart.frame,
        [SIDE_B.name, helloFromA, helloFromB],
      );
    });
    await runLeg(report, "identity_scan_chat_b", "scan the actual nonempty B direct Chat frame", async () => {
      await waitForMessage(b, bDirectAfterRestart.frame, helloFromA, 60_000);
      return identityFrameEvidence(
        b,
        "chat-room",
        bDirectAfterRestart.frame,
        [RENAMED_A, helloFromA, helloFromB],
      );
    });

    finalizeAcceptanceReport(report);
    console.log(JSON.stringify(report, null, 2));
  } catch (error) {
    console.error("FAIL home-two-runtime-acceptance");
    console.error(error.message || error);
    if (error.details !== undefined) {
      console.error(JSON.stringify(error.details, null, 2));
    }
    if (error.stack) {
      console.error(error.stack);
    }
    report.error = String(error.message || error);
    console.log(JSON.stringify(report, null, 2));
    process.exitCode = 1;
  } finally {
    if (a) {
      await a.context.close().catch(() => {});
    }
    if (b) {
      await b.context.close().catch(() => {});
    }
  }
}

await main();
