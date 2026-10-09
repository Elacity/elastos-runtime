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
// Each fresh Home enrolls its first owner passkey with a virtual authenticator.
// Both fixture profiles must have empty credential stores before this run.
// This run proves owner enrollment; guest account enrollment is a separate leg.
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
import { chmodSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { createRequire } from "node:module";

import {
  assertRecoverySetupEvidence,
  assertRecoveryBundleEvidence,
  assertDistinctProfileContactEvidence,
  assertDistinctRuntimeEvidence,
  assertExactDirectConversation,
  assertFreshFixturePrecondition,
  assertFreshOwnerEnrollmentPrecondition,
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
    const config = loadAcceptanceConfig(process.env);
    // Admit both sides before either browser starts or enrolls a passkey.
    assertFreshOwnerEnrollmentPrecondition(
      readCredentialStore(config.a.profile).length,
      readCredentialStore(config.b.profile).length,
    );
    return config;
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
  return credentials.length;
}

async function openSide(side) {
  const stored = readCredentialStore(side.profile);
  // Recheck before launch if fixture state changed after configuration admission.
  assertFreshOwnerEnrollmentPrecondition(stored.length, 0);
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
    return { enrollment: "resumed" };
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
  await side.page.locator("#home-unlock-primary").click();
  const completion = await registered;
  assertOk(completion.ok(), `${side.prefix}: passkey registration failed`, {
    status: completion.status(),
  });
  const verified = await completion.json();
  assertOk(verified?.schema === "elastos.auth.passkey.verify/v2"
    && typeof verified.principal_id === "string" && verified.principal_id.length > 0
    && verified.profile_readiness?.schema === "elastos.profile.readiness/v1"
    && verified.profile_readiness.status === "ready",
  `${side.prefix}: signup did not create a ready signed Profile`);
  side.signupPrincipalId = verified.principal_id;
  const credentials = await persistCredentials(side);
  assertOk(credentials === 1, `${side.prefix}: fresh enrollment did not create exactly one passkey`, { credential_count: credentials });
  side.hasStoredCredential = true;
  await signIn(side);
  return { enrollment: "enrolled", credential_count: credentials };
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
      const person = side.page.locator("#home-unlock-person");
      const primary = side.page.locator("#home-unlock-primary");
      const action = await person.isVisible() ? person : primary;
      if (await action.isVisible() && await action.isEnabled()) {
        await action.click();
      }
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
  // Keep existing capsule documents and their drafts alive when Home is ready.
  const ready = await side.page.evaluate(() => (
    document.body?.dataset?.homeAuthority === "signed"
      && document.body?.dataset?.homeStatus === "ready"
  )).catch(() => false);
  if (!ready) {
    await signIn(side);
  }
  const homeGuiFrame = await waitForFrame(side, "home-gui");
  if (target === "system" && await homeGuiFrame.locator("#setup-sheet:not(.is-yielded)").isVisible()) {
    const saveRecoveryKit = homeGuiFrame.locator("#setup-sheet-recovery");
    assertOk(
      (await saveRecoveryKit.innerText()).trim() === "Save Recovery Kit",
      `${side.prefix}: fresh Home Welcome did not offer Save Recovery Kit`,
    );
    await saveRecoveryKit.click();
    return waitForAppWindow(side, target);
  }
  const activeWindow = homeGuiFrame.locator(
    `section.window.window-active[data-target="${target}"] iframe.window-frame`,
  ).last();
  if (await activeWindow.isVisible()) return waitForAppWindow(side, target);
  const dock = homeGuiFrame.locator(
    `#taskbar-targets .taskbar-item[data-target="${target}"]`,
  ).first();
  if (await dock.isVisible()) {
    await dock.click();
    await activeWindow.waitFor({ state: "visible", timeout: 20_000 });
    return waitForAppWindow(side, target);
  }
  await homeGuiFrame.locator("#launcher-toggle").click();
  const card = homeGuiFrame.locator(`#launcher-grid [data-target="${target}"]`).first();
  await card.waitFor({ state: "visible", timeout: 10_000 });
  // Summary refreshes rebuild the launcher grid, which detaches cards
  // between actionability checks; dispatch the click on the current node.
  await homeGuiFrame.evaluate((appTarget) => {
    document.querySelector(`#launcher-grid [data-target="${appTarget}"]`)?.click();
  }, target);
  return waitForAppWindow(side, target);
}

async function waitForAppWindow(side, target, expectedConversationId = null) {
  const homeGuiFrame = await waitForFrame(side, "home-gui");
  const appFrame = await poll(`${side.prefix}: active ${target} window`, 20_000, 100, async () => {
    const windowFrameEl = homeGuiFrame
      .locator(`section.window.window-active[data-target="${target}"] iframe.window-frame`).last();
    if (!(await windowFrameEl.isVisible())) return { done: false };
    if (expectedConversationId) {
      const route = await windowFrameEl.getAttribute("data-route");
      if (!route) return { done: false };
      const url = new URL(route, side.base);
      const ids = [...url.searchParams.getAll("conversation_id"),
        ...new URLSearchParams(url.hash.replace(/^#/, "")).getAll("conversation_id")];
      if (url.origin !== new URL(side.base).origin || !url.pathname.startsWith(`/apps/${target}/`)
        || ids.length !== 1 || ids[0] !== expectedConversationId) return { done: false };
    }
    const handle = await windowFrameEl.elementHandle();
    const frame = handle ? await handle.contentFrame() : null;
    if (!frame || !frame.url().includes(`/apps/${target}/`)) return { done: false };
    if (expectedConversationId) {
      const state = await frame.evaluate(() => ({
        availableConversationIds: [...document.querySelectorAll("[data-conversation-choice]")]
          .map(node => node.dataset.conversationChoice || ""),
        selectedConversationId: document.querySelector("[data-conversation-choice].active")?.dataset?.conversationChoice || "",
        chatMode: document.body?.dataset?.chatMode || "",
      })).catch(() => null);
      try { assertExactDirectConversation({ expectedConversationId, ...state }); }
      catch { return { done: false }; }
    }
    if (!(await handle.evaluate(node => node.closest("section.window")?.classList.contains("window-active"))))
      return { done: false };
    return { done: true, value: frame };
  });
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

async function homeRecoveryState(side, systemFrame = null) {
  const home = await side.page.evaluate(async () => {
    const response = await fetch("/api/apps/home/summary", { credentials: "same-origin" });
    if (!response.ok) throw new Error(`Home Recovery acceptance read failed: HTTP ${response.status}`);
    const home = await response.json();
    return {
      signedIn: home?.authority?.signed_in === true,
      profileSchema: home?.identity?.profile_readiness?.schema || "",
      profileStatus: home?.identity?.profile_readiness?.status || "",
      profileName: home?.identity?.profile?.display_name || "",
      recoverySchema: home?.identity?.recovery_readiness?.schema || "",
      recoveryStatus: home?.identity?.recovery_readiness?.status || "",
    };
  });
  if (!systemFrame) return home;
  const recovery = await systemFrame.evaluate(async () => {
    const token = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
    if (!token) throw new Error("Recovery acceptance requires the launched System window");
    const response = await fetch("/api/auth/recovery/status", {
      credentials: "same-origin",
      headers: { "x-elastos-home-token": token },
    });
    if (!response.ok) throw new Error(`System Recovery acceptance read failed: HTTP ${response.status}`);
    const recovery = await response.json();
    return {
      archiveSchema: recovery?.schema || "",
      principalId: recovery?.principal_id || "",
      localhostRoot: recovery?.localhost_root || "",
      rootEncrypted: recovery?.root_encrypted === true,
      recoveryConfigured: recovery?.recovery_configured === true,
      archiveAvailable: recovery?.recovery_download_available === true,
      profileCovered: Array.isArray(recovery?.required_actions)
        && !recovery.required_actions.includes("download_recovery_kit_with_profile"),
    };
  });
  return { ...home, ...recovery };
}

async function completeRecoverySetup(side) {
  const before = await homeRecoveryState(side);
  assertOk(before.signedIn
    && before.profileSchema === "elastos.profile.readiness/v1" && before.profileStatus === "ready"
    && before.profileName === `${side.name} Admin`
    && before.recoverySchema === "elastos.recovery.readiness/v1" && before.recoveryStatus === "setup_required",
  `${side.prefix}: fresh signup did not create its consented Profile with Recovery still required`);
  const homeGuiFrame = await waitForFrame(side, "home-gui");
  await homeGuiFrame.locator("#setup-sheet:not(.is-yielded)").waitFor({ state: "visible", timeout: 30_000 });
  let downloads = 0;
  const countDownload = () => { downloads += 1; };
  side.page.on("download", countDownload);
  const downloadPromise = side.page.waitForEvent("download", { timeout: 45_000 });
  downloadPromise.catch(() => {});
  try {
    const [systemFrame, download] = await Promise.all([
      openAppWindow(side, "system"),
      downloadPromise,
    ]);
    await systemFrame.locator('button[data-settings="security"].active').waitFor({
      state: "visible", timeout: 10_000,
    });
    const downloadTarget = recoveryDownloadPath(side);
    mkdirSync(join(side.fixture.dataRoot, "acceptance-recovery"), { recursive: true, mode: 0o700 });
    await download.saveAs(downloadTarget);
    chmodSync(downloadTarget, 0o600);
    const bytes = statSync(downloadTarget).size;
    assertOk(bytes > 0 && bytes <= 8 * 1024 * 1024, `${side.prefix}: Recovery Kit size is invalid`);
    let bundle;
    try { bundle = JSON.parse(readFileSync(downloadTarget, "utf8")); }
    catch { fail(`${side.prefix}: Recovery Kit is not valid JSON`); }
    const after = await poll(`${side.prefix}: saved Recovery Kit is ready`, 30_000, 500, async () => {
      const current = await homeRecoveryState(side, systemFrame);
      return {
        done: current.recoverySchema === "elastos.recovery.readiness/v1" && current.recoveryStatus === "ready"
          && current.rootEncrypted && current.recoveryConfigured && current.archiveAvailable && current.profileCovered,
        value: current,
      };
    });
    assertOk(after.signedIn && after.archiveSchema === "elastos.principal.root-recovery.status/v1"
      && after.principalId === side.signupPrincipalId && after.localhostRoot.startsWith("localhost://Users/")
      && after.profileSchema === "elastos.profile.readiness/v1" && after.profileStatus === "ready"
      && after.profileName === before.profileName,
    `${side.prefix}: Recovery export changed the signup account or Profile`);
    const proof = assertRecoveryBundleEvidence(bundle, {
      principalId: side.signupPrincipalId,
      localhostRoot: after.localhostRoot,
      profileName: before.profileName,
    });
    // Home exposes the Profile name. Mutual accepted contacts later bind this
    // private Recovery identity through Runtime's opaque contact projection.
    side.recoveryProfileDid = bundle.people_identity.profile_authority_bundle.signed_profile.payload.profile_did;
    assertOk(downloads === 1, `${side.prefix}: Welcome did not save exactly one Recovery Kit`, { download_count: downloads });
    return {
      download_count: downloads,
      download_path: downloadTarget,
      profile_status: after.profileStatus,
      profile_name: after.profileName,
      before_recovery_status: before.recoveryStatus,
      after_recovery_status: after.recoveryStatus,
      ...proof,
      archive_available: after.archiveAvailable,
    };
  } finally {
    side.page.off("download", countDownload);
  }
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
      const entries = [...document.querySelectorAll("#entry-rows .entry-rail-card, #entry-rows .entry-row")];
      const matches = entries.flatMap((entry) => {
        const text = (entry.textContent || "").replace(/\s+/g, " ");
        if (!text.includes(name)) {
          return [];
        }
        let accept = [...entry.querySelectorAll("button")]
          .find((button) => button.textContent?.trim() === "Accept");
        if (!accept && entry.classList.contains("entry-row")) {
          entry.click();
          const detail = document.querySelector("#entry-detail");
          if (detail?.textContent?.includes(name)) {
            accept = [...detail.querySelectorAll("button")]
              .find((button) => button.textContent?.trim() === "Accept");
          }
        }
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
  const chatFrame = await waitForAppWindow(side, "chat-room", conversationId);
  await assertDirectSelection(side, chatFrame, conversationId);
  return { frame: chatFrame, conversationId };
}

async function assertDirectSelection(side, chatFrame, conversationId) {
  await chatFrame.waitForFunction(() => {
    const input = document.querySelector("#message-input");
    return input && !input.disabled;
  }, null, { timeout: 60_000 });
  // A launch must select its requested contact without a corrective rail click.
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
  return selection;
}

async function selectDirectConversation(side, chatFrame, conversationId) {
  await chatFrame.evaluate((id) => {
    const exact = [...document.querySelectorAll("[data-conversation-choice]")]
      .find((node) => node.dataset.conversationChoice === id);
    exact?.click();
  }, conversationId);
  await assertDirectSelection(side, chatFrame, conversationId);
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
  // Close the remembered People drawer through its visible user control.
  const peopleDrawer = chatFrame.locator("#presence-card");
  if (await peopleDrawer.isVisible()) {
    await chatFrame.locator("#participant-close").click();
    await peopleDrawer.waitFor({ state: "hidden", timeout: 10_000 });
  }
}

async function openSharedConversation(side) {
  const chatFrame = await openAppWindow(side, "chat-room");
  await selectSharedConversation(side, chatFrame);
  return chatFrame;
}

async function openParticipantCard(side, chatFrame, peerName, expectedAction) {
  if (!(await chatFrame.locator("#participant-list").isVisible())) {
    await chatFrame.locator("#participant-toggle").click();
  }
  let opened = false;
  let nextRefresh = 0;
  await poll(`${side.prefix}: Community Profile for ${peerName}`, 120_000, 1_000, async () => {
    const shouldOpen = !opened || Date.now() >= nextRefresh;
    const result = await chatFrame.evaluate(({ name, action, shouldOpen }) => {
      const peers = [...document.querySelectorAll("#participant-list [data-participant-ref]")]
        .filter((node) => node.querySelector(".participant-name")?.textContent?.trim() === name);
      const actionButton = document.querySelector("#participant-card-action");
      const pending = actionButton?.hidden === false && actionButton.disabled === true;
      if (peers.length === 1 && shouldOpen && !pending) {
        peers[0].click();
      }
      const card = document.querySelector("#participant-card");
      const button = document.querySelector("#participant-card-action");
      return {
        count: peers.length,
        opened: peers.length === 1 && shouldOpen && !pending,
        name: document.querySelector("#participant-card-name")?.textContent?.trim() || "",
        action: button?.dataset?.cardAction || "",
        ready: card?.hidden === false && button?.hidden === false && !button?.disabled
          && button?.dataset?.cardAction === action,
      };
    }, { name: peerName, action: expectedAction, shouldOpen });
    assertOk(result.count <= 1, `${side.prefix}: ambiguous Community Profile`, result);
    if (result.opened) {
      opened = true;
      nextRefresh = Date.now() + 5_000;
    }
    return { done: result.count === 1 && result.name === peerName && result.ready, value: result };
  });
}

async function requestCommunityContact(side, chatFrame, peopleFrame, peerName) {
  await openParticipantCard(side, chatFrame, peerName, "add-contact");
  const requestResponse = side.page.waitForResponse((response) => (
    response.request().method() === "POST"
      && new URL(response.url()).pathname === "/api/apps/chat-room/contacts/request"
  ), { timeout: 30_000 });
  requestResponse.catch(() => {});
  await chatFrame.locator("#participant-card-action").click();
  const response = await requestResponse;
  assertOk(response.ok(), `${side.prefix}: Community contact request failed`, { status: response.status() });
  await poll(`${side.prefix}: one Community contact request`, 60_000, 1_000, async () => {
    const snapshot = await peopleSnapshot(peopleFrame);
    const requested = snapshot.contacts.filter((card) => (
      card.text.includes(peerName) && /Request sent|Requested/i.test(card.text)
    ));
    assertOk(requested.length <= 1, `${side.prefix}: duplicate outgoing contact requests`, { count: requested.length });
    return { done: requested.length === 1, value: { count: requested.length } };
  });
  return { count: 1, request_surface: "community_profile" };
}

async function openInboxFromCommunity(side, chatFrame, peerName) {
  await openParticipantCard(side, chatFrame, peerName, "inbox");
  await chatFrame.locator("#participant-card-action").click();
  // No launcher fallback: the Profile action itself must open Inbox.
  return waitForAppWindow(side, "inbox");
}

async function openDirectFromCommunity(side, chatFrame, peerName, conversationId) {
  await selectSharedConversation(side, chatFrame);
  await openParticipantCard(side, chatFrame, peerName, "message");
  await chatFrame.locator("#participant-card-action").click();
  await assertDirectSelection(side, chatFrame, conversationId);
  return { frame: chatFrame, conversationId };
}

async function openDirectFromInbox(side, inboxFrame, senderName, conversationId) {
  // Open the actual Inbox through Home before reading its visible notification.
  inboxFrame = await openAppWindow(side, "inbox");
  await poll(`${side.prefix}: Inbox notification opens its conversation`, 60_000, 1_000, async () => {
    const result = await inboxFrame.evaluate((name) => {
      const title = `New message from ${name}`;
      const body = `${name} sent you a message in Chat.`;
      const entries = [...document.querySelectorAll("#entry-rows .entry-rail-card, #entry-rows .entry-row")]
        .filter((node) => (
          node.querySelector(".entry-row-title")?.textContent?.trim() === title
          && node.querySelector(".entry-row-snippet")?.textContent?.trim() === body
        ));
      if (entries.length !== 1) {
        return { count: entries.length, opened: false };
      }
      const row = entries[0];
      let button = [...row.querySelectorAll("button")]
        .find((node) => node.textContent?.trim() === "Open");
      if (!button) {
        row.click();
        const detail = document.querySelector("#entry-detail");
        if (detail?.querySelector(".entry-title")?.textContent?.trim() === title
          && detail?.querySelector(".entry-body")?.textContent?.trim() === body) {
          button = [...detail.querySelectorAll("button")]
            .find((node) => node.textContent?.trim() === "Open");
        }
      }
      if (button && !button.disabled) {
        button.click();
        return { count: 1, opened: true };
      }
      return { count: 1, opened: false };
    }, senderName);
    assertOk(result.count <= 1, `${side.prefix}: ambiguous Inbox direct notification`, result);
    return { done: result.opened, value: result };
  });
  const chatFrame = await waitForAppWindow(side, "chat-room", conversationId);
  await assertDirectSelection(side, chatFrame, conversationId);
  return { frame: chatFrame, conversationId };
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
      const count = await exactMessageCount(chatFrame, text);
      assertOk(count <= 1, `${side.prefix}: duplicate rendered message`, { count });
      return { done: count === 1, value: { count } };
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
      const count = await exactMessageCount(chatFrame, text);
      assertOk(count <= 1, `${side.prefix}: duplicate received message`, { count });
      return { done: count === 1, value: { count } };
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

async function exactMessageCount(chatFrame, text) {
  return chatFrame.evaluate((needle) => [...document.querySelectorAll("#message-list .message-body")]
    .filter((node) => node.textContent === needle).length, text);
}

async function assertDraft(side, chatFrame, selected, text) {
  const state = await chatFrameState(chatFrame);
  assertOk(
    state.selectedConversationId === selected && state.inputValue === text,
    `${side.prefix}: conversation draft changed`,
    { selected: state.selectedConversationId, expected: selected, draft_preserved: state.inputValue === text },
  );
}

async function withHeldSendResponse(side, chatFrame, path, sentText, conversationId, operation) {
  const matcher = (url) => url.origin === side.base && url.pathname === path;
  let responseStatus;
  let release;
  let handlerCompletion;
  let handlerError;
  let intercepted = 0;
  let deadlineExpired = false;
  const handler = async (route) => {
    try {
      const request = route.request();
      if (request.method() !== "POST") {
        await route.continue();
        return;
      }
      const payload = request.postDataJSON();
      if ((payload?.text ?? payload?.body) !== sentText) {
        await route.continue();
        return;
      }
      intercepted += 1;
      assertOk(intercepted === 1, "held send was submitted more than once");
      if (conversationId !== "shared") {
        assertOk(payload.conversation_id === conversationId, "held send targeted the wrong contact");
      }
      const response = await route.fetch({ timeout: 10_000 });
      responseStatus = response.status();
      const gate = new Promise((resolve) => { release = resolve; });
      const deadline = setTimeout(() => { deadlineExpired = true; release(); }, 20_000);
      handlerCompletion = (async () => {
        try {
          await gate;
          await route.fulfill({ response });
        } finally {
          clearTimeout(deadline);
        }
      })();
      await handlerCompletion;
    } catch (error) {
      handlerError = error;
      await route.abort().catch(() => {});
    }
  };
  await side.page.route(matcher, handler);
  try {
    await chatFrame.locator("#message-input").fill(sentText);
    await chatFrame.locator("#send-button").click();
    await poll("successful send response is held", 15_000, 100, async () => {
      assertOk(!handlerError, "held-response interception failed");
      return { done: Boolean(release), value: { status: responseStatus } };
    });
    assertOk(responseStatus >= 200 && responseStatus < 300, "held send did not succeed", { status: responseStatus });
    await operation(async () => {
      assertOk(!deadlineExpired, "held response deadline elapsed before draft checks");
      release();
      await handlerCompletion;
      assertOk(!handlerError, "held-response release failed");
    });
    assertOk(!handlerError && !deadlineExpired, "held-response proof did not complete within its bound");
    return responseStatus;
  } finally {
    release?.();
    await handlerCompletion?.catch(() => {});
    await side.page.unroute(matcher, handler);
  }
}

async function proveDraftPreservation(a, chatFrame, b, bChatFrame, conversationId) {
  const directDraft = `Direct draft @ ${Date.now()}`;
  const sharedDraft = `Community draft @ ${Date.now()}`;
  await chatFrame.locator("#message-input").fill(directDraft);
  await selectSharedConversation(a, chatFrame);
  await assertDraft(a, chatFrame, "shared", "");
  await chatFrame.locator("#message-input").fill(sharedDraft);
  await selectDirectConversation(a, chatFrame, conversationId);
  await assertDraft(a, chatFrame, conversationId, directDraft);

  // An edit within the same direct selection must invalidate its old send.
  const directText = `Held direct response @ ${Date.now()}`;
  const newerDirectDraft = `Newer direct draft @ ${Date.now()}`;
  const directStatus = await withHeldSendResponse(
    a, chatFrame, "/api/apps/chat-room/direct/messages/send", directText, conversationId,
    async (release) => {
      await chatFrame.locator("#message-input").fill(newerDirectDraft);
      await release();
      await waitForMessage(a, chatFrame, directText, 30_000);
      await delay(1_000);
      await assertDraft(a, chatFrame, conversationId, newerDirectDraft);
      await waitForMessage(b, bChatFrame, directText, 60_000);
    },
  );
  await selectSharedConversation(a, chatFrame);
  await assertDraft(a, chatFrame, "shared", sharedDraft);
  await selectSharedConversation(b, bChatFrame);

  // Returning to Community must still invalidate a response from its older
  // selection, even though the conversation name is the same again.
  const sharedText = `Held Community response @ ${Date.now()}`;
  const newerSharedDraft = `Newer Community draft @ ${Date.now()}`;
  const sharedStatus = await withHeldSendResponse(
    a, chatFrame, "/api/apps/chat-room/objects/send", sharedText, "shared",
    async (release) => {
      await chatFrame.locator("#message-input").fill(newerSharedDraft);
      await selectDirectConversation(a, chatFrame, conversationId);
      await assertDraft(a, chatFrame, conversationId, newerDirectDraft);
      await selectSharedConversation(a, chatFrame);
      await assertDraft(a, chatFrame, "shared", newerSharedDraft);
      await release();
      await waitForMessage(a, chatFrame, sharedText, 30_000);
      await delay(1_000);
      await assertDraft(a, chatFrame, "shared", newerSharedDraft);
      await waitForMessage(b, bChatFrame, sharedText, 60_000);
    },
  );
  await chatFrame.locator("#message-input").fill("");
  await selectDirectConversation(a, chatFrame, conversationId);
  await assertDraft(a, chatFrame, conversationId, newerDirectDraft);
  await chatFrame.locator("#message-input").fill("");
  await selectDirectConversation(b, bChatFrame, conversationId);
  return {
    switched_drafts_preserved: true,
    direct_newer_draft_after_response: true,
    shared_newer_draft_after_response: true,
    held_send_status: { direct: directStatus, shared: sharedStatus },
    receiver_message_count: { direct: 1, shared: 1 },
  };
}

async function proveSessionRecovery(side, chatFrame, receiver, receiverFrame, {
  conversationId = "shared",
  lossSource = "poll",
} = {}) {
  const direct = conversationId !== "shared";
  assertOk(lossSource === "poll" || direct && lossSource === "send", "unsupported recovery fault");
  if (direct) {
    await selectDirectConversation(side, chatFrame, conversationId);
    await selectDirectConversation(receiver, receiverFrame, conversationId);
  } else {
    await selectSharedConversation(side, chatFrame);
  }
  const documentStartedAt = await chatFrame.evaluate(() => performance.timeOrigin);
  const oldHomeToken = await chatFrame.evaluate(() => globalThis.elastosChatHomeToken?.());
  assertOk(typeof oldHomeToken === "string" && oldHomeToken.length > 0, "Chat had no launch authority before session loss");
  const draft = `${direct ? "Direct" : "Community"} ${lossSource} Reconnect draft @ ${Date.now()}`;
  await chatFrame.locator("#message-input").fill(draft);
  assertOk(!(await chatFrame.locator("#reconnect-button").isVisible()), "Reconnect was already visible before session loss");
  let injected = false;
  let handlerError;
  let starts = 0;
  let authPosts = 0;
  const startTokens = [];
  const countStarts = (request) => {
    if (request.method() === "POST") {
      const path = new URL(request.url()).pathname;
      if (path === "/api/apps/chat-room/session/start") {
        starts += 1;
        startTokens.push(request.headers()["x-elastos-home-token"] || "");
      } else if (path.startsWith("/api/auth/")) {
        authPosts += 1;
      }
    }
  };
  const faultPath = direct
    ? lossSource === "send" ? "/api/apps/chat-room/direct/messages/send"
      : `/api/apps/chat-room/direct/conversations/${encodeURIComponent(conversationId)}/messages`
    : "/api/apps/chat-room/poll";
  const matcher = (url) => url.origin === side.base && url.pathname === faultPath;
  const handler = async (route) => {
    try {
      const request = route.request();
      if (request.method() !== (!direct || lossSource === "send" ? "POST" : "GET") || injected) {
        await route.continue();
        return;
      }
      if (lossSource === "send") {
        const payload = request.postDataJSON();
        if (payload?.conversation_id !== conversationId || payload?.text !== draft) {
          await route.continue();
          return;
        }
      }
      assertOk(request.headers()["x-elastos-home-token"] === oldHomeToken, "recovery fault targeted stale Chat authority");
      injected = true;
      await route.fulfill({ status: 401, contentType: "application/json", body: JSON.stringify({
        error: direct ? "invalid or expired launch token" : "invalid or expired session",
      }) });
    } catch (error) {
      handlerError = error;
      await route.abort().catch(() => {});
    }
  };
  side.page.on("request", countStarts);
  await side.page.route(matcher, handler);
  try {
    if (lossSource === "send") {
      await chatFrame.locator("#send-button").click();
    }
    await chatFrame.locator("#reconnect-button").waitFor({ state: "visible", timeout: 30_000 });
    assertOk(!handlerError, "session-loss interception failed");
    assertOk(injected, "Reconnect appeared without the injected session loss");
    assertOk(await chatFrame.locator("#reconnect-button").isEnabled(), "Reconnect control was disabled");
    const explanation = chatFrame.locator("#error-text");
    assertOk(await explanation.isVisible() && Boolean((await explanation.innerText()).trim()), "Reconnect had no visible explanation");
    await assertDraft(side, chatFrame, conversationId, draft);
    const lost = await chatFrameState(chatFrame);
    assertOk(lost.sendDisabled && (direct || lost.inputDisabled), "Chat kept sending enabled after session loss");
    await delay(1_500);
    assertOk(starts === 0, "room session restarted before the Reconnect click", { session_starts: starts });
    assertOk(authPosts === 0, "sign-in started before the Reconnect click", { auth_posts: authPosts });
    if (lossSource === "send") {
      assertOk(await exactMessageCount(receiverFrame, draft) === 0, "refused direct send reached its peer");
    }
    await side.page.unroute(matcher, handler);
    await chatFrame.locator("#reconnect-button").click();
    await poll(`${side.prefix}: user Reconnect resumes ${direct ? "Direct" : "Community"}`, 30_000, 250, async () => {
      const state = await chatFrameState(chatFrame);
      return {
        done: starts > 0 && !state.inputDisabled && !(await chatFrame.locator("#reconnect-button").isVisible()),
        value: { session_starts: starts, input_disabled: state.inputDisabled },
      };
    });
    assertOk(
      (await waitForAppWindow(side, "chat-room")) === chatFrame
        && (await chatFrame.evaluate(() => performance.timeOrigin)) === documentStartedAt,
      "Reconnect replaced the Chat document",
    );
    const freshHomeToken = await chatFrame.evaluate(() => globalThis.elastosChatHomeToken?.());
    assertOk(typeof freshHomeToken === "string" && freshHomeToken.length > 0 && freshHomeToken !== oldHomeToken,
      "Reconnect retained its old launch authority");
    assertOk(startTokens.length > 0 && startTokens.every((token) => token === freshHomeToken),
      "Reconnect started a room session with stale launch authority");
    await assertDraft(side, chatFrame, conversationId, draft);
    if (direct) {
      await assertDirectSelection(side, chatFrame, conversationId);
    }
    await chatFrame.locator("#message-input").fill("");
    if (!direct) {
      await selectSharedConversation(receiver, receiverFrame);
    }
    const message = `${direct ? "Direct" : "Community"} after ${lossSource} Reconnect @ ${Date.now()}`;
    await sendMessage(side, chatFrame, message);
    await waitForMessage(receiver, receiverFrame, message, 120_000);
    assertOk(!handlerError, "session-loss interception did not finish cleanly");
    return { loss_source: lossSource, injected_status: 401, conversation_id: conversationId,
      visible_reconnect: true, session_starts_before_click: 0, auth_posts_before_click: 0,
      session_starts_after_click: starts, fresh_home_token: true, same_document: true,
      draft_preserved: true, receiver_message_count: 1, message };
  } finally {
    side.page.off("request", countStarts);
    await side.page.unroute(matcher, handler);
  }
}

async function removeContact(side, frame, peerName) {
  frame = await openAppWindow(side, "people");
  const snapshot = await peopleSnapshot(frame);
  const contact = snapshot.contacts.find((card) => card.text.includes(peerName));
  const remove = contact?.actions.find((action) => action.action === "remove");
  assertOk(remove?.contactId, `${side.prefix}: no removable contact for ${peerName}`, snapshot.contacts);
  const removeButton = frame.locator(`[data-action="remove"][data-contact-id="${remove.contactId}"]`);
  assertOk(await removeButton.count() === 1, `${side.prefix}: ambiguous Remove control`);
  await removeButton.click();
  const confirmation = frame.locator(`[data-action="confirm-remove"][data-contact-id="${remove.contactId}"]`);
  await confirmation.waitFor({ state: "visible", timeout: 10_000 });
  assertOk((await confirmation.locator("xpath=../..").innerText()).includes(`Remove ${peerName} from People?`),
    `${side.prefix}: Remove confirmation named the wrong person`);
  await confirmation.click();
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
    report.toolchain = {
      node: process.version,
      playwright: require("playwright/package.json").version,
      chromium: a.context.browser()?.version() || "",
    };

    await runLeg(report, "provisioning_and_sign_in", "provision and sign in both fixture Homes", async () => {
      const aEnrollment = await ensureAccount(a);
      const bEnrollment = await ensureAccount(b);
      assertOk(aEnrollment.enrollment === "enrolled" && bEnrollment.enrollment === "enrolled",
        "signup proof requires fresh owner enrollment on both fixture Homes");
      return { a: aEnrollment, b: bEnrollment, enrollment_surface: "home_owner_passkey" };
    });

    await runLeg(report, "system_recovery_after_signup", "save each signup Profile in a Recovery Kit from Welcome", async () => {
      const evidence = {
        a: await completeRecoverySetup(a),
        b: await completeRecoverySetup(b),
      };
      return assertRecoverySetupEvidence(CONFIG, evidence);
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

    // Enter Community through Home on both sides. Signed room messages make
    // the peer's Profile available through the normal participant controls.
    const [aCommunity, bCommunity] = await Promise.all([
      openSharedConversation(a),
      openSharedConversation(b),
    ]);
    const aCommunityMarker = `Community hello A @ ${Date.now()}`;
    const bCommunityMarker = `Community hello B @ ${Date.now()}`;
    await sendMessage(a, aCommunity, aCommunityMarker);
    await waitForMessage(b, bCommunity, aCommunityMarker, 120_000);
    await sendMessage(b, bCommunity, bCommunityMarker);
    await waitForMessage(a, aCommunity, bCommunityMarker, 120_000);

    await runLeg(report, "exactly_one_contact_request", "A requests the Community Profile once", async () => {
      await assertPeopleHasNoDecisionActions(b, bPeople);
      const evidence = await requestCommunityContact(a, aCommunity, aPeople, SIDE_B.name);
      assertOk(evidence.count === 1, "outgoing request evidence was not exact", evidence);
      await assertPeopleHasNoDecisionActions(b, bPeople);
      return evidence;
    });

    let bInbox;
    await runLeg(report, "inbox_only_accept", "B opens Inbox from the Community Profile and accepts", async () => {
      bInbox = await openInboxFromCommunity(b, bCommunity, SIDE_A.name);
      await acceptContactRequest(b, bInbox, SIDE_A.name);
      bPeople = await openAppWindow(b, "people");
      await assertPeopleHasNoDecisionActions(b, bPeople);
      return { decision_surface: "inbox", launch_surface: "community_profile" };
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
      const binding = assertDistinctProfileContactEvidence(aContactId, bContactId, a.recoveryProfileDid, b.recoveryProfileDid);
      return { a_contact_id: aContactId, b_contact_id: bContactId, ...binding };
    });

    let aDirect;
    await runLeg(report, "community_profile_launch", "Community Profile Message selects the accepted conversation", async () => {
      aDirect = await openDirectFromCommunity(a, aCommunity, SIDE_B.name, conversationId);
      return { conversation_id: conversationId, launch_surface: "community_profile", corrective_selection: false };
    });
    let bDirect = await openConversation(b, bPeople, SIDE_A.name, conversationId);
    // Leave B in Community so Inbox Open must change its selection.
    await selectSharedConversation(b, bDirect.frame);
    const helloFromA = `Hello from ${SIDE_A.name} @ ${Date.now()}`;
    await runLeg(report, "direct_message_a_to_b", "direct message A to B", async () => {
      await sendMessage(a, aDirect.frame, helloFromA);
      bDirect = await openDirectFromInbox(b, bInbox, SIDE_A.name, conversationId);
      await waitForMessage(b, bDirect.frame, helloFromA);
      return { conversation_id: conversationId, message: helloFromA };
    });

    await runLeg(report, "inbox_direct_launch", "Inbox Open selects the incoming direct conversation", async () => {
      await assertDirectSelection(b, bDirect.frame, conversationId);
      return { conversation_id: conversationId, launch_surface: "inbox", corrective_selection: false };
    });

    const helloFromB = `Hello back from ${SIDE_B.name} @ ${Date.now()}`;
    await runLeg(report, "direct_message_b_to_a", "direct message B to A", async () => {
      await sendMessage(b, bDirect.frame, helloFromB);
      await waitForMessage(a, aDirect.frame, helloFromB);
      return { conversation_id: conversationId, message: helloFromB };
    });

    await runLeg(report, "conversation_draft_preservation", "preserve drafts through selection and a late send response", async () => (
      proveDraftPreservation(a, aDirect.frame, b, bDirect.frame, conversationId)
    ));

    await runLeg(report, "shared_session_recovery", "recover the room only after visible Reconnect", async () => (
      proveSessionRecovery(a, aDirect.frame, b, bDirect.frame)
    ));

    await runLeg(report, "direct_session_recovery", "recover Direct polling and sending only after visible Reconnect", async () => ({
      poll: await proveSessionRecovery(a, aDirect.frame, b, bDirect.frame, { conversationId }),
      send: await proveSessionRecovery(a, aDirect.frame, b, bDirect.frame, { conversationId, lossSource: "send" }),
    }));

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
