#!/usr/bin/env node
// Upgrade lockout smoke: a 0.7.1-style passkey the browser cannot discover
// still signs in, through the account-named fallback, in headless Chromium.
//
// The virtual authenticator is a security key without resident-key support
// (hasResidentKey: false). The first owner passkey is created with the exact
// v0.7.1 options (residentKey "preferred", requireResidentKey false), so the
// authenticator stores a non-discoverable credential, as some security keys
// and Android Password Manager do. The smoke then proves, on the real Home:
//   1. the anonymous lock-face sign-in (empty allow-list) cannot find it;
//   2. an unknown account name gets decoys and reads like a missing passkey;
//   3. the account name the Home shows signs in through the fallback;
//   4. System then offers Add passkey once.
//
// Needs a fresh Home on http://localhost (no passkey yet):
//   ELASTOS_BASE_URL=http://localhost:PORT node scripts/home-passkey-named-fallback-smoke.mjs
// Patterns follow scripts/home-passkey-virtual-auth-smoke.mjs.

import { createRequire } from "node:module";

const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");

const BASE_URL = (process.env.ELASTOS_BASE_URL || "http://localhost:8090").replace(/\/+$/, "");
const HOME_URL = process.env.HOME_URL || `${BASE_URL}/home/`;
const ACCOUNT_NAME = process.env.HOME_NAMED_FALLBACK_NAME || "Upgrade Smoke Owner";
const HEADLESS = process.env.HOME_NAMED_FALLBACK_HEADED !== "1";
// The anonymous and decoy prompts can never succeed; bound how long the
// browser keeps them open instead of the Home's five-minute ceremony.
const UNFINDABLE_PROMPT_MS = 5_000;

function assert(condition, message, details = undefined) {
  if (!condition) throw Object.assign(new Error(message), { details });
}

async function homeState(page) {
  return page.evaluate(() => ({
    status: document.body?.dataset?.homeStatus || "",
    authority: document.body?.dataset?.homeAuthority || "",
    title: document.querySelector("#home-unlock-title")?.textContent?.trim() || "",
    faceStatus: document.querySelector("#home-unlock-face-status")?.textContent?.trim() || "",
    cardStatus: document.querySelector("#home-unlock-status")?.textContent?.trim() || "",
  }));
}

async function waitForSignedHome(page, timeoutMs = 60_000) {
  await page.waitForFunction(
    () => document.body?.dataset?.homeStatus === "ready"
      && document.body?.dataset?.homeAuthority === "signed",
    null,
    { timeout: timeoutMs },
  );
}

async function waitForLockFace(page) {
  await page.waitForFunction(() => {
    const person = document.querySelector("#home-unlock-person");
    return document.body?.dataset?.homeStatus === "locked"
      && person && !person.disabled && person.getClientRects().length > 0;
  }, null, { timeout: 30_000 });
}

async function signOut(page) {
  const result = await page.evaluate(async () => {
    const refresh = await fetch("/api/auth/sessions/refresh", { method: "POST", credentials: "same-origin" });
    const token = refresh.ok ? (await refresh.json()).home_token : "";
    const response = await fetch("/api/auth/sessions/sign-out", {
      method: "POST",
      credentials: "same-origin",
      headers: { "content-type": "application/json", ...(token ? { "x-elastos-home-token": token } : {}) },
    });
    return { ok: response.ok, status: response.status };
  });
  assert(result.ok, "Home sign-out failed", result);
  await page.goto(HOME_URL, { waitUntil: "domcontentloaded" });
  await waitForLockFace(page);
}

function systemFrame(page) {
  const origin = new URL(HOME_URL).origin;
  return page.frames().find((frame) => {
    try {
      const url = new URL(frame.url());
      return url.origin === origin && url.pathname.startsWith("/apps/system/");
    } catch {
      return false;
    }
  }) || null;
}

async function main() {
  assert(new URL(HOME_URL).hostname === "localhost",
    "Use http://localhost: browsers refuse IP addresses as WebAuthn relying parties", { HOME_URL });
  const browser = await chromium.launch({
    headless: HEADLESS,
    executablePath: process.env.ELASTOS_BROWSER_EXECUTABLE || undefined,
  });
  const evidence = { schema: "elastos.home.passkey-named-fallback-smoke/v1", home_url: HOME_URL, steps: [] };
  const step = (name, details = {}) => evidence.steps.push({ name, ...details });
  try {
    const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
    const page = await context.newPage();
    const requests = [];
    page.on("request", (request) => {
      const url = new URL(request.url());
      if (url.pathname.startsWith("/api/auth/passkey/")) requests.push(url.pathname);
    });

    // A security key without resident-key support.
    const cdp = await context.newCDPSession(page);
    await cdp.send("WebAuthn.enable");
    const { authenticatorId } = await cdp.send("WebAuthn.addVirtualAuthenticator", {
      options: {
        protocol: "ctap2",
        transport: "usb",
        hasResidentKey: false,
        hasUserVerification: true,
        isUserVerified: true,
        automaticPresenceSimulation: true,
      },
    });

    // Register the way v0.7.1 did: residentKey "preferred".
    await page.route("**/api/auth/passkey/register/begin", async (route) => {
      const response = await route.fetch();
      const body = await response.json();
      const selection = body?.options?.publicKey?.authenticatorSelection;
      assert(selection?.residentKey === "required", "0.8.0 registration no longer requires resident keys", body);
      body.options.publicKey.authenticatorSelection = {
        ...selection,
        residentKey: "preferred",
        requireResidentKey: false,
      };
      await route.fulfill({ response, json: body });
    });
    // The prompts that cannot succeed end after a few seconds.
    for (const path of ["**/api/auth/passkey/authenticate/begin", "**/api/auth/passkey/authenticate/named/begin"]) {
      await page.route(path, async (route) => {
        const response = await route.fetch();
        const body = await response.json();
        if (body?.options?.publicKey) body.options.publicKey.timeout = UNFINDABLE_PROMPT_MS;
        await route.fulfill({ response, json: body });
      });
    }

    await page.goto(HOME_URL, { waitUntil: "domcontentloaded" });
    const status = await page.evaluate(async () => (await fetch("/api/auth/passkey/status")).json());
    assert(status.registered === false, "The smoke needs a fresh Home with no passkey", status);
    const name = page.locator("#home-unlock-name");
    await name.waitFor({ state: "visible", timeout: 30_000 });
    await name.fill(ACCOUNT_NAME);
    await page.locator("#home-unlock-primary").click();
    await waitForSignedHome(page, 120_000);
    await page.unroute("**/api/auth/passkey/register/begin");
    const { credentials } = await cdp.send("WebAuthn.getCredentials", { authenticatorId });
    assert(credentials.length === 1 && credentials[0].isResidentCredential === false,
      "The v0.7.1-style registration did not store one non-discoverable credential", credentials);
    const credentialId = Buffer.from(credentials[0].credentialId, "base64").toString("base64url");
    step("registered-v071-style", { resident: credentials[0].isResidentCredential });

    // 1. The anonymous lock face cannot find it.
    await signOut(page);
    requests.length = 0;
    const anonymousBegin = page.waitForResponse((response) =>
      new URL(response.url()).pathname === "/api/auth/passkey/authenticate/begin");
    await page.locator("#home-unlock-person").click();
    const anonymous = await (await anonymousBegin).json();
    assert(Array.isArray(anonymous.options.publicKey.allowCredentials)
      && anonymous.options.publicKey.allowCredentials.length === 0,
      "The anonymous begin listed credentials", anonymous.options.publicKey);
    await page.waitForFunction(() => {
      const node = document.querySelector("#home-unlock-face-status");
      return node && !node.hidden && /No passkey was used/.test(node.textContent || "");
    }, null, { timeout: UNFINDABLE_PROMPT_MS + 30_000 });
    let state = await homeState(page);
    assert(state.authority !== "signed", "The anonymous sign-in signed in", state);
    assert(!requests.includes("/api/auth/passkey/authenticate/complete"),
      "The anonymous sign-in reached completion", requests);
    step("anonymous-cannot-find", { face_status: state.faceStatus });

    // 2. An unknown name gets decoys and reads like a missing passkey.
    await page.locator("#home-unlock-named").click();
    await page.locator("#home-unlock-account").waitFor({ state: "visible", timeout: 10_000 });
    const decoyBegin = page.waitForResponse((response) =>
      new URL(response.url()).pathname === "/api/auth/passkey/authenticate/named/begin");
    await page.locator("#home-unlock-account").fill("Nobody At This Home");
    await page.locator("#home-unlock-primary").click();
    const decoys = await (await decoyBegin).json();
    const decoyIds = decoys.options.publicKey.allowCredentials.map((entry) => entry.id);
    assert(decoyIds.length > 0 && !decoyIds.includes(credentialId),
      "An unknown name did not get decoys", decoys.options.publicKey);
    await page.waitForFunction(() => /No passkey for that account name was used/
      .test(document.querySelector("#home-unlock-status")?.textContent || ""),
    null, { timeout: UNFINDABLE_PROMPT_MS + 30_000 });
    state = await homeState(page);
    assert(state.authority !== "signed", "A decoy sign-in signed in", state);
    step("unknown-name-decoys", { decoy_count: decoyIds.length, card_status: state.cardStatus });

    // 3. The account name the Home shows (case and spacing folded) signs in.
    const namedBegin = page.waitForResponse((response) =>
      new URL(response.url()).pathname === "/api/auth/passkey/authenticate/named/begin");
    await page.locator("#home-unlock-account").fill(`  ${ACCOUNT_NAME.toUpperCase()} `);
    await page.locator("#home-unlock-primary").click();
    const named = await (await namedBegin).json();
    assert(JSON.stringify(named.options.publicKey.allowCredentials)
      === JSON.stringify([{ type: "public-key", id: credentialId }]),
      "The named begin did not list exactly this account's credential", named.options.publicKey);
    assert(named.ceremony_id.startsWith("passkey:authenticate-named:"), "Unexpected named ceremony", named);
    await waitForSignedHome(page);
    step("named-fallback-signed-in", { listed: named.options.publicKey.allowCredentials.length });

    // 4. System offers Add passkey once.
    const deadline = Date.now() + 60_000;
    let offer = null;
    while (Date.now() < deadline && !offer?.focused) {
      const frame = systemFrame(page);
      offer = frame ? await frame.evaluate(() => ({
        status: document.querySelector('[data-field="passkey-status"]')?.textContent?.trim() || "",
        buttonVisible: !document.querySelector("#passkey-add")?.hidden,
        focused: document.activeElement?.id === "passkey-add",
      })).catch(() => null) : null;
      if (!offer?.focused) await new Promise((resolve) => setTimeout(resolve, 250));
    }
    assert(offer?.focused && offer.buttonVisible && /signed in with your account name/.test(offer.status),
      "System did not offer Add passkey after the fallback sign-in", offer);
    step("add-passkey-offered", { status: offer.status });

    evidence.ok = true;
    console.log(JSON.stringify(evidence, null, 2));
  } finally {
    await browser.close();
  }
}

main().catch((error) => {
  console.error("FAIL home-passkey-named-fallback-smoke");
  console.error(error.message);
  if (error.details !== undefined) console.error(JSON.stringify(error.details, null, 2));
  process.exit(1);
});
