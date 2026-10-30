#!/usr/bin/env node
// Browser fixture, not installed Runtime proof. Real Home/System/GUI modules;
// the server models the gateway's actor checks, SameSite and isolation headers.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, resolve, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium, makeSystemSummary, inertSystemApiResponse } from "./system-uiux-fixture.mjs";
import assert from "node:assert/strict";

const repo = fileURLToPath(new URL("../", import.meta.url));
const principalRoot = "localhost://Users/fixture";
const TITLES = { "home-gui": "Desktop", system: "System", library: "Library" };
const records = new Map();
const tokens = new Map();
const reads = [];
const errors = [];
let serial = 0;
let now = 0;
const hour = 3600;
const sessionIdentity = (actor) => ({
  principal_id: actor, session_id: `fixture-session-${actor}`, proof_binding_id: `fixture-proof-${actor}`,
});
let delayRead = null;
let completedReads = 0;
let refuseNextRead = false;
const original = process.env.WALLPAPER_ORIGINAL === "1";
const imageKey = (actor) => records.has(actor)
  ? `/api/apps/home/appearance/background-image?scope=${actor}&v=${records.get(actor).version}` : null;
function homeSummary(actor) {
  return {
    home: { route: "/apps/home/", attach_kind: "iframe" },
    app: { id: "home", route: "/apps/home/" },
    identity: {
      device_did: null,
      profile_readiness: { schema: "elastos.profile.readiness/v1", status: "ready" },
      recovery_readiness: { schema: "elastos.recovery.readiness/v1", status: "ready" },
    },
    authority: { signed_in: true, ...sessionIdentity(actor), wallet_connected: false },
    browser_state: {
      schema: "elastos.home.browser-state/v1", principal_id: actor, localhost_root: principalRoot, layout: null, recent_targets: [],
      session: { browser_context_id: "browser:00112233445566778899aabbccddeeff", root_shell: "home-gui", windows: [] },
    },
    active_shell: {
      schema: "elastos.home.active-shell/v1", active: "home-gui",
      candidates: [{ name: "home-gui", title: "Desktop", description: "Fixture Desktop", route: "/apps/home-gui/", role: "shell", trust_state: "trusted", launchable: true }],
    },
    appearance: {
      schema: "elastos.home.appearance/v1", revision: 0, theme: "dark", accent: "blue", accent_custom: "#4f7fff", dock_auto_hide: false, sounds: false,
      focus_mode: false, background_image_url: imageKey(actor), background_overlay_enabled: true, background_overlay_opacity: 0.55,
    },
    runtime: { running: true, kind: "local", version: null, api_url: null, pid: null, running_capsules: [], note: null },
    site: { staged: false, root_uri: "", path: "", active_release: null, active_channel: null, active_bundle_cid: null, release_count: 0 },
    room: {
      room_slug: "fixture", title: "", member_count: 0, active_member_count: 0, pending_count: 0, active_session_count: 0, latest_request_name: null,
      latest_request_device: null, local_runtime_did: null, local_runtime_role: null, canonical_hosted_guest_url: null, ephemeral_hosted_guest_url: null,
      browser_access_allowed: true, browser_access_block_reason: null, pending_requests: [], active_sessions: [],
    },
    people: { schema: "elastos.people.contacts/v1", contact_count: 0, contacts: [], service_offer_count: 0, service_offers: [] },
    services: {
      schema: "elastos.runtime.services/v1", local_offer_count: 0, remote_offer_count: 0, available_local_offer_count: 0, available_remote_offer_count: 0,
      local_offers: [], remote_offers: [], available_local_offers: [], available_remote_offers: [], grant_model: "principal_scoped_provider_grant",
      carrier_contract: "", capsule_contract: "",
    },
    notifications: { unread_count: 0, attention_count: 0, entries: [] },
    desktop_objects: { schema: "elastos.home.desktop-objects/v1", uri: `${principalRoot}/Desktop`, objects: [], stale: false },
    capsule_catalog: {
      schema: "elastos.capsules.catalog/v1", capsules: [],
      counts: { total: 0, installed: 0, launchable: 0, interfaces: 0, methods: 0, apps: 0, viewers: 0, providers: 0, content: 0, shell: 0 },
      policy: { install_state: "signed-app-install-pending", install_note: "", payment_state: "provider-rail-required", payment_note: "", drm_state: "provider-rail-required", drm_note: "" },
    },
    capsule_interfaces: {
      schema: "elastos.capsules.interfaces/v1", interfaces: [], counts: { capsules: 0, interfaces: 0, methods: 0, executable_methods: 0 },
      policy: { descriptor_state: "manifest-declared", descriptor_note: "", invocation_state: "runtime-gated", invocation_note: "" },
    },
    // As the gateway lists them: Archive is launchable but not a Home target.
    targets: ["system", "library"].map((target) => ({
      target, title: TITLES[target], description: "", role: "app", target_kind: "app", attach_kind: "iframe",
    })),
  };
}

function headers(req, extra = {}) {
  return { "cache-control": "no-store", "cross-origin-opener-policy": "same-origin",
    "cross-origin-embedder-policy": "require-corp",
    ...(req.headers.origin === "null" ? { "access-control-allow-origin": "null" } : {}), ...extra };
}
function send(req, res, status, body, extra = {}) {
  res.writeHead(status, headers(req, extra)); res.end(body);
}
function json(req, res, body, status = 200) {
  send(req, res, status, JSON.stringify(body), { "content-type": "application/json" });
}
async function body(req) {
  const chunks = []; for await (const chunk of req) chunks.push(chunk);
  return Buffer.concat(chunks);
}
const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url, `http://${req.headers.host}`);
    const cookieActor = /fixture-session=(admin|guest)/.exec(req.headers.cookie || "")?.[1];
    const token = tokens.get(req.headers["x-elastos-home-token"]);
    const actor = token?.actor || cookieActor;
    if (req.method === "OPTIONS") return send(req, res, 204, "", {
      "access-control-allow-methods": "GET,POST,DELETE,OPTIONS",
      "access-control-allow-headers": "content-type,x-elastos-home-token",
    });
    if (url.pathname === "/api/auth/sessions/refresh") {
      const homeToken = `home-${cookieActor}-${++serial}`;
      tokens.set(homeToken, { actor: cookieActor, target: "home", expires: now + 12 * hour });
      return json(req, res, { home_token: homeToken, ...sessionIdentity(cookieActor) });
    }
    if (url.pathname === "/api/apps/home/summary") return json(req, res, homeSummary(actor));
    if (url.pathname === "/api/apps/home/runtime/ensure") return json(req, res, { ready: true });
    if (url.pathname === "/api/apps/home/events/stream") {
      res.writeHead(200, headers(req, { "content-type": "text/event-stream" })); res.write(": fixture\n\n"); return;
    }
    if (url.pathname === "/api/apps/home/events") return json(req, res, { events: [], cursor: "" });
    if (url.pathname === "/api/apps/home/state") return json(req, res, { saved: true });
    if (url.pathname === "/api/apps/home/launch") {
      const { target, query = {} } = JSON.parse(await body(req));
      assert.equal(token?.target, "home");
      assert.ok(TITLES[target]);
      const launch = `${actor}-${target}-${++serial}`;
      tokens.set(launch, { actor, target });
      const params = new URLSearchParams({ ...query, home_origin: url.origin });
      return json(req, res, { target, title: TITLES[target], attach_kind: "iframe", launch_status: "launched",
        route: `/apps/${target}/?${params}#home_token=${launch}` });
    }
    if (url.pathname === "/api/apps/home/appearance/background-image") {
      const allowed = (req.headers["x-elastos-home-token"]
        ? token?.target === "home" && token.actor === cookieActor && token.expires > now
        : cookieActor)
        && req.headers["sec-fetch-site"] === "same-origin"
        && (!req.headers.origin || req.headers.origin === url.origin)
        && (!req.headers.referer || new URL(req.headers.referer).origin === url.origin);
      const record = records.get(actor);
      const status = !allowed || refuseNextRead ? 403 : record ? 200 : 404;
      refuseNextRead = false;
      reads.push({ actor, status, target: token?.target || "cookie-or-none",
        site: req.headers["sec-fetch-site"], origin: req.headers.origin || "", cookie: !!cookieActor, cookieActor });
      const wait = delayRead; delayRead = null;
      if (wait) await wait;
      completedReads += 1;
      return send(req, res, status, status === 200 ? record.bytes : "Image unavailable", {
        "content-type": status === 200 ? record.type : "text/plain",
        "cross-origin-resource-policy": "cross-origin", "x-content-type-options": "nosniff",
      });
    }
    if (url.pathname === "/api/apps/system/appearance/background-image") {
      if (token?.target !== "system") return json(req, res, { error: "System authority required" }, 403);
      if (req.method === "POST") records.set(actor, { bytes: await body(req), type: req.headers["content-type"], version: ++serial });
      else if (req.method === "DELETE") records.delete(actor);
      else return json(req, res, {}, 405);
      return json(req, res, homeSummary(actor).appearance);
    }
    if (url.pathname === "/api/apps/system/summary") {
      const summary = makeSystemSummary(homeSummary(actor).appearance);
      summary.access.role = actor === "guest" ? "guest" : "admin";
      return json(req, res, summary);
    }
    const inert = inertSystemApiResponse(url.pathname);
    if (inert !== null) return json(req, res, inert);
    if (url.pathname.startsWith("/api/")) return json(req, res, { error: "fixture route missing" }, 404);
    const match = url.pathname.match(/^\/apps\/(home|home-gui|system)\/(.*)$/);
    if (!match) return send(req, res, 404, "missing");
    const root = resolve(repo, "capsules", match[1], "browser");
    const path = resolve(root, match[2] || "index.html");
    assert.ok(!relative(root, path).startsWith(".."));
    let bytes = await readFile(path);
    if (original && ["home-gui.js", "home-gui-shell.js", "system.js", "home-shell-host.js", "shell-core.js", "shell-auth.js", "home-wallet-connector-host.js"].includes(match[2])) {
      // Read the preserved starting commit without another checkout or worktree.
      const { execFileSync } = await import("node:child_process");
      bytes = execFileSync("git", ["show", `3c654ebac11761b4f9acc4a176f88d6460fada2a:capsules/${match[1]}/browser/${match[2]}`], { cwd: repo });
    }
    const mime = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css",
      ".svg": "image/svg+xml", ".png": "image/png", ".webp": "image/webp", ".woff2": "font/woff2" }[extname(path)];
    send(req, res, 200, bytes, { "content-type": mime || "application/octet-stream", "cross-origin-resource-policy": "cross-origin" });
  } catch (error) { errors.push(String(error.stack || error)); if (!res.headersSent) json(req, res, { error: String(error) }, 500); else res.end(); }
});
await new Promise((resolveListen) => server.listen(0, "127.0.0.1", resolveListen));
const origin = `http://127.0.0.1:${server.address().port}`;
let browser;
try {
  browser = await chromium.launch({ headless: true,
    ...(process.env.ELASTOS_BROWSER_EXECUTABLE ? { executablePath: process.env.ELASTOS_BROWSER_EXECUTABLE } : {}),
    args: ["--no-first-run", "--disable-extensions", "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1"] });
  async function open(actor) {
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    await context.addCookies([{ name: "fixture-session", value: actor, url: origin, sameSite: "Strict", httpOnly: true }]);
    await context.addInitScript(() => {
      const create = URL.createObjectURL.bind(URL), revoke = URL.revokeObjectURL.bind(URL);
      window.imageUrls = new Set();
      URL.createObjectURL = (blob) => { const url = create(blob); window.imageUrls.add(url); return url; };
      URL.revokeObjectURL = (url) => { window.imageUrls.delete(url); revoke(url); };
    });
    const page = await context.newPage();
    page.on("pageerror", (error) => console.error("PAGE", String(error)));
    await page.goto(`${origin}/apps/home/`);
    await page.locator("#active-shell-frame").waitFor();
    const gui = () => page.frames().find((f) => f.url().includes("/apps/home-gui/"));
    await page.frameLocator("#active-shell-frame").locator(".desktop-backdrop").waitFor({ state: "attached" });
    async function system() {
      await page.evaluate(() => window.postMessage({ type: "home:open-target", target: "system", query: { settings: "personalization" } }, location.origin));
      const frame = await waitFor(() => page.frames().find((f) => f.url().includes("/apps/system/")));
      await frame.locator("#background-input").waitFor({ state: "attached" });
      await frame.waitForFunction(() => !document.querySelector("#background-input").disabled);
      return frame;
    }
    return { page, context, gui, system };
  }
  async function waitFor(fn) {
    for (let i = 0; i < 200; i++) { const result = await fn(); if (result) return result; await new Promise((r) => setTimeout(r, 50)); }
    throw new Error(`Timed out; reads=${JSON.stringify(reads)} errors=${JSON.stringify(errors)}`);
  }
  // A real PNG generated by Chromium, then uploaded through System's file input.
  const admin = await open("admin");
  const adminReadCount = () => reads.filter((read) => read.cookieActor === "admin").length;
  let system = await admin.system();
  async function png(color) {
    return Buffer.from(await admin.page.evaluate((fill) => {
      const c = document.createElement("canvas"); c.width = c.height = 8;
      const ctx = c.getContext("2d"); ctx.fillStyle = fill; ctx.fillRect(0, 0, 8, 8);
      return c.toDataURL("image/png").split(",")[1];
    }, color), "base64");
  }
  const red = await png("#e02030"), blue = await png("#2050e0");
  async function upload(frame, bytes) {
    await frame.locator("#background-input").setInputFiles({ name: "background.png", mimeType: "image/png", buffer: bytes });
    await frame.locator('[data-field="background-status"]').filter({ hasText: "Updated." }).waitFor();
  }
  async function pixel(frame, selector, color) {
    const expected = color[0] === 224 ? [...red] : [...blue];
    await frame.waitForFunction(async ({ selector, expected }) => {
      const css = getComputedStyle(document.querySelector(selector)).backgroundImage;
      if (!css.includes("blob:") || window.imageUrls.size !== 1) return false;
      const image = new Image(); image.src = css.match(/url\("?([^"\)]+)/)[1];
      try {
        await image.decode();
        const bytes = new Uint8Array(await (await fetch(image.src)).arrayBuffer());
        return image.naturalWidth === 8 && image.naturalHeight === 8
          && bytes.length === expected.length && bytes.every((v, i) => v === expected[i]);
      } catch { return false; }
    }, { selector, expected });
  }
  await upload(system, red);
  if (original) {
    await admin.gui().waitForFunction(() => getComputedStyle(document.querySelector(".desktop-backdrop")).backgroundImage.includes("/api/apps/home/appearance/background-image"));
    await waitFor(() => reads.filter((r) => r.status === 403).length >= 2);
    console.log("Original browser reproduction: protected CSS image GET refused", JSON.stringify(reads));
    assert.ok(reads.every((r) => r.status === 403));
  } else {
    await pixel(system, "#background-preview", [224, 32, 48, 255]);
    await pixel(admin.gui(), ".desktop-backdrop", [224, 32, 48, 255]);
    await upload(system, blue);
    await pixel(system, "#background-preview", [32, 80, 224, 255]);
    await pixel(admin.gui(), ".desktop-backdrop", [32, 80, 224, 255]);
    // Renew Home before the original mint expires, then keep that Desktop
    // alive past expiry. A fresh System upload must reach the existing reader.
    await admin.gui().locator('.window[data-target="system"] [data-action="close"]').first().click();
    await waitFor(() => !admin.page.frames().some((f) => f.url().includes("/apps/system/")));
    const expiringHomeToken = [...tokens].reverse().find(([_, value]) => value.actor === "admin" && value.target === "home")[0];
    now = 11 * hour;
    await admin.page.evaluate(async () => (await import("/apps/home/shell-auth.js?v=home-wallpaper-20261030b")).refreshHomeSession());
    now = 12 * hour + 1;
    assert.equal(await admin.page.evaluate(async (homeToken) => (await fetch("/api/apps/home/appearance/background-image", {
      headers: { "x-elastos-home-token": homeToken },
    })).status, expiringHomeToken), 403);
    system = await admin.system();
    await upload(system, red);
    await pixel(system, "#background-preview", [224, 32, 48, 255]);
    await pixel(admin.gui(), ".desktop-backdrop", [224, 32, 48, 255]);
    await upload(system, blue);
    await pixel(admin.gui(), ".desktop-backdrop", [32, 80, 224, 255]);
    await admin.page.reload();
    await admin.page.frameLocator("#active-shell-frame").locator(".desktop-backdrop").waitFor({ state: "attached" });
    system = await admin.system();
    await pixel(system, "#background-preview", [32, 80, 224, 255]);
    await pixel(admin.gui(), ".desktop-backdrop", [32, 80, 224, 255]);
    const guest = await open("guest");
    const guestSystem = await guest.system();
    assert.equal(await guest.gui().evaluate(() => window.imageUrls.size), 0);
    await upload(guestSystem, red);
    await pixel(guestSystem, "#background-preview", [224, 32, 48, 255]);
    await pixel(guest.gui(), ".desktop-backdrop", [224, 32, 48, 255]);
    await pixel(admin.gui(), ".desktop-backdrop", [32, 80, 224, 255]);
    const guestRead = await guest.page.evaluate(async () => {
      const response = await fetch("/api/apps/home/appearance/background-image?scope=admin&v=1");
      return { status: response.status, bytes: [...new Uint8Array(await response.arrayBuffer())] };
    });
    assert.equal(guestRead.status, 200); assert.deepEqual(Buffer.from(guestRead.bytes), red);
    // System can upload but its direct opaque-origin image GET stays refused.
    const systemToken = [...tokens].reverse().find(([_, value]) => value.actor === "admin" && value.target === "system")[0];
    const direct = await system.evaluate(async (homeToken) => {
      const response = await fetch("/api/apps/home/appearance/background-image", { headers: { "x-elastos-home-token": homeToken } });
      return response.status;
    }, systemToken);
    assert.equal(direct, 403);
    // A different frame with the right System token still lacks the registered source.
    const count = adminReadCount();
    await admin.page.evaluate(async (homeToken) => {
      const frame = document.createElement("iframe"); frame.sandbox = "allow-scripts";
      frame.srcdoc = `<script>top.postMessage({type:"home:background-image",requestId:"wrong-frame",homeToken:${JSON.stringify(homeToken)}},"*")<\/script>`;
      document.body.append(frame);
      await new Promise((resolve) => { frame.onload = () => setTimeout(resolve, 100); });
      frame.remove();
    }, systemToken);
    assert.equal(adminReadCount(), count);
    assert.equal(await system.evaluate(() => {
      try { return !!window.parent.document; } catch { return false; }
    }), false);
    if (process.env.WALLPAPER_PROOF_DIR) {
      await admin.page.screenshot({ path: `${process.env.WALLPAPER_PROOF_DIR}/admin.png` });
      await guest.page.screenshot({ path: `${process.env.WALLPAPER_PROOF_DIR}/guest.png` });
    }
    await system.locator("#background-reset").click();
    await waitFor(async () => await system.evaluate(() => window.imageUrls.size === 0));
    await waitFor(async () => await admin.gui().evaluate(() => window.imageUrls.size === 0));
    await pixel(guest.gui(), ".desktop-backdrop", [224, 32, 48, 255]);
    // Reset invalidates an in-flight handoff, so late bytes cannot restore it.
    let releaseRead;
    delayRead = new Promise((resolve) => { releaseRead = resolve; });
    const beforeDelay = reads.length;
    await upload(system, red);
    await waitFor(() => reads.length > beforeDelay);
    await system.locator("#background-reset").click();
    await waitFor(async () => await system.evaluate(() => window.imageUrls.size === 0));
    const completed = completedReads;
    releaseRead();
    await waitFor(() => completedReads > completed);
    await new Promise((resolve) => setTimeout(resolve, 100));
    assert.equal(await system.evaluate(() => window.imageUrls.size), 0);
    await waitFor(async () => await admin.gui().evaluate(() => window.imageUrls.size === 0));
    // The actual System error gives a recovery action; reopening retries.
    refuseNextRead = true;
    await upload(system, blue);
    await system.locator('[data-field="background-status"]').filter({ hasText: "Close System and open it to retry." }).waitFor();
    assert.equal(await system.evaluate(() => window.imageUrls.size), 0);
    await admin.gui().locator('.window[data-target="system"] [data-action="close"]').first().click();
    await waitFor(() => !admin.page.frames().some((f) => f.url().includes("/apps/system/")));
    system = await admin.system();
    await pixel(system, "#background-preview", [32, 80, 224, 255]);
    // A session change retires an existing frame's reader and late results.
    const currentSystemToken = [...tokens].reverse().find(([_, value]) => value.actor === "admin" && value.target === "system")[0];
    const readFromSystem = (requestId) => system.evaluate(([homeToken, requestId]) => new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("handoff timed out")), 5000);
      async function receive(event) {
        if (event.source !== top || event.origin !== new URL(location.href).searchParams.get("home_origin")
          || event.data?.type !== "home:background-image-result" || event.data.requestId !== requestId) return;
        removeEventListener("message", receive); clearTimeout(timer);
        resolve(event.data.blob ? [...new Uint8Array(await event.data.blob.arrayBuffer())] : null);
      }
      addEventListener("message", receive);
      top.postMessage({ type: "home:background-image", requestId, homeToken }, new URL(location.href).searchParams.get("home_origin"));
    }), [currentSystemToken, requestId]);
    delayRead = new Promise((resolve) => { releaseRead = resolve; });
    const beforeSignOut = reads.length;
    const late = readFromSystem("session-change");
    await waitFor(() => reads.length > beforeSignOut);
    await admin.page.evaluate(async () => (await import("/apps/home/shell-core.js?v=home-wallpaper-20261030b")).setHomeAuthorityToken("home-guest", {
      principal_id: "guest", session_id: "fixture-session-guest", proof_binding_id: "fixture-proof-guest",
    }));
    releaseRead();
    assert.equal(await late, null);
    const afterSessionChange = adminReadCount();
    assert.equal(await readFromSystem("retired-frame"), null);
    assert.equal(adminReadCount(), afterSessionChange);
    assert.ok(reads.filter((r) => r.status === 200).every((r) => r.site === "same-origin"));
    assert.deepEqual(errors, []);
    console.log("PASS browser fixture: upload, decoded preview/desktop bytes, replacement, reload, token renewal/expiry, guest isolation, wrong-frame rejection, session binding, delayed reset, retry and URL cleanup", JSON.stringify(reads));
    await guest.context.close();
  }
  await admin.context.close();
} catch (error) {
  console.error("Fixture evidence", JSON.stringify({ reads, errors }));
  throw error;
} finally {
  await browser?.close();
  server.closeAllConnections();
  await new Promise((resolveClose) => server.close(resolveClose));
}
