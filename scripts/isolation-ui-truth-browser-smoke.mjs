#!/usr/bin/env node
// Source UI fixtures for #170. Installed journeys require the candidate receipt.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, resolve, relative, isAbsolute, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const playwrightModule = process.env.ELASTOS_PLAYWRIGHT_MODULE
  ? await import(pathToFileURL(process.env.ELASTOS_PLAYWRIGHT_MODULE).href)
  : createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url))("playwright");
const { chromium } = playwrightModule.chromium ? playwrightModule : playwrightModule.default;
const cid = `bafybei${"a".repeat(52)}`;
const requests = [];
const misses = [];
const errors = [];
let modelSignature = "catalog-signature-verified";
let catalogState = "verified";
const cases = [
  { name: "blank", author: "" },
  { name: "unknown", author: "Unknown publisher" },
  { name: "forged", author: "ElastOS" },
  { name: "invalid", author: "Untrusted publisher", signature: "invalid" },
  { name: "malformed", author: "Untrusted publisher", signature: { broken: true } },
  { name: "spoofed-state", author: "ElastOS", signature_state: "catalog-signature-verified" },
];
function catalog() {
  return { model_catalog_state: catalogState, capsules: [
    ...cases.map(item => ({ ...item, title: `Fixture ${item.name}`, role: "app", type: "web-projection", cid,
      source: "local", installed: false, launchable: false, trust_state: "cid-with-manifest-signature",
      signature_state: item.signature_state || "manifest-signature-declared", payment_state: "not-declared", drm_state: "unknown" })),
    { name: "launchable", title: "Fixture launchable", author: "Publisher", role: "app", installed: true,
      launchable: true, launch_target: "launchable", trust_state: "local-dev", signature_state: "no-manifest-signature",
      payment_state: "provider", drm_state: "provider" },
    { name: "model", title: "Fixture model", role: "content", source: "signed-model-catalog", installed: false,
      launchable: false, signature_state: modelSignature, trust_state: "publisher-verified-admission-pending",
      cid, publisher_did: "did:key:zFixturePublisher", content_size_bytes: 1024, model_content: { format: "gguf" } },
  ] };
}
const targets = ["system", "marketplace", "library", "documents", "people", "inbox", "wallet"]
  .map(target => ({ target, title: target[0].toUpperCase() + target.slice(1), role: "app", attach_kind: "iframe", launchable: true }));
const homeSummary = {
  authority: { signed_in: true, principal_id: "did:key:fixture", proof_binding_id: "fixture", session_id: "fixture" },
  identity: { display_name: "Fixture Home", recovery_readiness: { status: "ready" }, profile_readiness: { status: "ready" } },
  targets, active_shell: { active: "home-gui", candidates: [] },
  appearance: { schema: "elastos.home.appearance/v1", revision: 0, theme: "dark", accent: "blue", accent_custom: "#4f7fff",
    dock_auto_hide: false, sounds: false, focus_mode: false, background_image_url: null,
    background_overlay_enabled: true, background_overlay_opacity: 0.55 },
  browser_state: { principal_id: "did:key:fixture", recent_targets: [], session: null,
    layout: { taskbar: ["system", "marketplace"], desktop: { system: { x: 20, y: 20 } }, desktopHidden: targets.filter(t => t.target !== "system").map(t => t.target),
      desktopLabels: {}, desktopIconsVisible: true } },
  desktop_objects: { objects: [{ uri: "localhost://fixture/Desktop/file.txt", name: "file.txt", kind: "file", mime: "text/plain",
    capabilities: ["open", "download", "properties", "rename", "trash"], viewers: [{ id: "documents", default: true }] }] },
  people: { contacts: [], discovery: { enabled: true, status: "visible", expires_at: 9999999999 } },
  notifications: { unread_count: 0, attention_count: 0, entries: [] }, room: {}, runtime: {}, services: {},
};
const server = createServer(async (req, res) => {
  const url = new URL(req.url, "http://fixture");
  const cors = { "access-control-allow-origin": "null", "access-control-allow-headers": "content-type,x-elastos-home-token", "access-control-allow-methods": "GET,POST,OPTIONS" };
  const json = (value, status = 200) => { res.writeHead(status, { ...cors, "content-type": "application/json" }); res.end(JSON.stringify(value)); };
  try {
    if (req.method === "OPTIONS") { res.writeHead(204, cors); res.end(); return; }
    if (url.pathname === "/") {
      const app = url.searchParams.get("app") || "marketplace";
      const origin = `http://127.0.0.1:${server.address().port}`;
      res.writeHead(200, { "content-type": "text/html" });
      res.end(`<style>body{margin:0}iframe{border:0;width:100%;height:100vh}</style>
        <iframe sandbox="allow-scripts allow-forms" src="/apps/${app}/?home_origin=${encodeURIComponent(origin)}#home_token=fixture-token"></iframe>
        <script>window.calls=[];window.addEventListener('message',event=>{
          const m=event.data;window.calls.push(m);
          const frame=document.querySelector('iframe');
          if(m.type==='home:shell-ready')frame.contentWindow.postMessage({type:'home:shell-summary',summary:${JSON.stringify(homeSummary)}},'*');
          if(m.type==='home:launch-target')frame.contentWindow.postMessage({type:'home:shell-response',requestId:m.requestId,result:{
            target:m.target,title:m.target,attach_kind:'iframe',route:'/fixture-app?target='+m.target+'#home_token=fixture-app'}},'*');
          if(m.type==='home:ui-preference'){
            const summary=${JSON.stringify(homeSummary)};const fields={theme:'theme',accent:'accent',focusMode:'focus_mode',sounds:'sounds',dockAutoHide:'dock_auto_hide'};
            if(fields[m.key])summary.appearance[fields[m.key]]=['focusMode','sounds','dockAutoHide'].includes(m.key)?m.value==='on':m.value;
            frame.contentWindow.postMessage({type:'home:shell-response',requestId:m.requestId,result:{}},'*');
            frame.contentWindow.postMessage({type:'home:shell-summary',summary},'*');
          }
        });</script>`); return;
    }
    if (url.pathname === "/fixture-app") { res.writeHead(200, { ...cors, "content-type": "text/html" }); res.end(`<p>Fixture launched</p><script>window.addEventListener('message',e=>{
        const m=e.data;
        if(m.type==='elastos.documents.window-close.request/v1')parent.postMessage({type:'elastos.documents.window-close.result/v1',requestId:m.requestId,homeToken:m.homeToken,state:'terminal',ok:true,reason:'',sessionId:1},'*');
        if(m.type==='elastos.system.window-ready.request/v1')parent.postMessage({type:'elastos.system.window-ready.result/v1',requestId:m.requestId,homeToken:m.homeToken,documentNonce:'fixture-app'},'*');
        if(m.type==='elastos.system.window.request/v1')parent.postMessage({type:'elastos.system.window.result/v1',requestId:m.requestId,homeToken:m.homeToken,documentNonce:m.documentNonce,ok:true},'*');
      });</script>`); return; }
    if (url.pathname.startsWith("/api/")) {
      requests.push({ path: url.pathname, method: req.method });
      if (url.pathname === "/api/capsules/catalog") { json(catalog()); return; }
      if (url.pathname === "/api/capsules/interfaces") { json({ interfaces: [] }); return; }
      if (url.pathname === "/api/apps/home/state") { json({}); return; }
      if (url.pathname === "/api/apps/services/summary") { json({ remote_offers: [], available_remote_offers: [] }); return; }
      if (url.pathname === "/api/provider/model/offers_list") { json({ offers: [], remote_services: [] }); return; }
      if (url.pathname === "/api/provider/object/list_runtime_custody") {
        json({ schema: "elastos.library.runtime-custody-listings/v1", listings: [], truncated: false }); return;
      }
      misses.push(url.pathname); json({ error: "Fixture route unavailable" }, 404); return;
    }
    const match = /^\/apps\/(home-gui|home|marketplace)\/(.*)$/.exec(url.pathname);
    if (!match) { if (url.pathname === "/favicon.ico") { res.writeHead(204); res.end(); return; } throw new Error("Unknown fixture asset"); }
    const dir = resolve(root, "capsules", match[1], "browser");
    const path = resolve(dir, decodeURIComponent(match[2]) || "index.html");
    const rel = relative(dir, path);
    assert.ok(rel !== ".." && !rel.startsWith(`..${sep}`) && !isAbsolute(rel));
    const body = await readFile(path);
    res.writeHead(200, { ...cors, "content-type": ({ ".js": "text/javascript", ".html": "text/html", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" })[extname(path)] || "application/octet-stream" });
    res.end(body);
  } catch (error) { errors.push(error.message); res.writeHead(500); res.end("Fixture failed"); }
});
await new Promise(done => server.listen(0, "127.0.0.1", done));
const origin = `http://127.0.0.1:${server.address().port}`;
let browser;
try {
  browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: "reduce" });
  const pageErrors = []; page.on("pageerror", error => pageErrors.push(error.message));
  const frame = () => page.frames().find(f => f !== page.mainFrame());
  await page.goto(origin);
  await frame().locator('.store-row[data-app="blank"]').waitFor();
  for (const item of cases) {
    const row = frame().locator(`.store-row[data-app="${item.name}"]`).first();
    const expected = !item.author || item.author === "Unknown publisher" ? "Publisher unknown" : `Declared author · ${item.author}`;
    assert.equal(await row.locator(".store-row-sub").textContent(), expected);
    await row.focus(); await row.press("Enter");
    assert.equal(await frame().locator(".modal-developer").textContent(), expected);
    const text = await frame().locator("#detail-content").textContent();
    assert.doesNotMatch(text, /Runtime-verified|Verified|Signed local|Supports payments|Uses protected content/);
    assert.match(text, /Publisher verification unavailable/);
    assert.equal(await frame().locator('#detail-content [data-action="open"]').count(), 0);
    assert.equal(await frame().locator('#detail-content [data-action="install"]').count(), 0);
    await frame().locator("#detail-content").press("Escape");
    assert.equal(await row.evaluate(el => document.activeElement === el), true);
  }
  const launch = frame().locator('.store-row[data-app="launchable"]').first();
  await launch.click();
  assert.match(await frame().locator("#detail-content").textContent(), /Supports payments/);
  assert.match(await frame().locator("#detail-content").textContent(), /Uses protected content/);
  await frame().locator("#detail-content").press("Escape");
  await launch.locator('[data-action="open"]').press("Enter");
  await page.waitForFunction(() => window.calls.some(m => m.type === "home:open-target" && m.target === "launchable"));
  assert.equal(await frame().locator("#detail-modal.active").count(), 0);
  await frame().locator('[data-destination="models"]').first().click();
  await frame().locator('[data-action="model-detail"]').first().click();
  assert.match(await frame().locator(".modal-developer").textContent(), /Runtime-verified publisher/);
  assert.match(await frame().locator("#detail-content").textContent(), /Runtime verified the catalog publisher signature/);
  for (const [signature, state] of [["invalid", "verified"], [{ malformed: true }, "verified"], ["catalog-signature-verified", "unavailable"]]) {
    modelSignature = signature; catalogState = state; await page.reload();
    await frame().locator("#load-error:not(.hidden)").waitFor();
    assert.equal(await frame().locator(".store-row").count(), 0);
    assert.doesNotMatch(await frame().locator("body").textContent(), /Runtime-verified publisher|Verified publisher/);
  }
  modelSignature = "catalog-signature-verified"; catalogState = "verified";
  await page.goto(`${origin}/?app=home-gui`);
  await frame().locator('body[data-home-status="ready"]').waitFor();
  assert.equal(await frame().locator("#control-centre-discovery").count(), 0);
  await frame().locator("#desktop-shortcut-system").click({ button: "right" });
  assert.deepEqual(await frame().locator('#desktop-context-menu [role="menuitem"]').allTextContents(),
    ["Open System", "Rename", "Remove from Desktop", "Remove from Shelf"]);
  await frame().getByRole("menuitem", { name: "Rename", exact: true }).click();
  const rename = frame().locator(".desktop-shortcut-rename"); await rename.fill("Fixture shortcut"); await rename.press("Enter");
  assert.equal(await frame().locator("#desktop-shortcut-system .desktop-shortcut-title").textContent(), "Fixture shortcut");
  await frame().locator("#desktop-shortcut-system").press("Enter");
  await page.waitForFunction(() => window.calls.some(m => m.type === "home:launch-target" && m.target === "system"));
  await page.waitForTimeout(2700); // Home protects a new window from the opening pointer's close click.
  await frame().locator('.window [data-action="close"]').first().click();
  await frame().locator('.window').waitFor({ state: 'detached' });
  const file = frame().locator('[data-kind="object"]').first();
  await file.click({ button: "right" });
  assert.deepEqual(await frame().locator('#desktop-context-menu [role="menuitem"]').allTextContents(), ["Open", "Show in Library", "Download", "Properties"]);
  await file.press("Escape");
  await file.press("F2"); await file.press("Delete");
  assert.equal(await frame().locator(".desktop-shortcut-rename").count(), 0, "File edit shortcuts stay unavailable");
  await file.dblclick();
  await page.waitForFunction(() => window.calls.some(m => m.type === "home:launch-target" && m.target === "documents"));
  await page.waitForTimeout(2700); // Home protects a new window from the opening pointer's close click.
  await frame().locator('.window [data-action="close"]').first().click();
  await frame().locator('.window').waitFor({ state: 'detached' });
  await frame().locator("#desktop").click({ button: "right", position: { x: 600, y: 400 } });
  assert.deepEqual(await frame().locator('#desktop-context-menu [role="menuitem"]').allTextContents(), ["Hide Desktop Icons", "Auto-arrange Icons", "Change Wallpaper…"]);
  await frame().getByRole("menuitem", { name: "Hide Desktop Icons" }).press("Enter");
  assert.equal(await frame().locator("#desktop-shortcuts").isVisible(), false);
  await frame().locator("#desktop").click({ button: "right", position: { x: 600, y: 400 } });
  await frame().getByRole("menuitem", { name: "Show Desktop Icons" }).click();
  assert.equal(await frame().locator("#desktop-shortcuts").isVisible(), true);
  await frame().locator("#toolbar-control-centre").click();
  for (const id of ["control-centre-focus", "control-centre-sounds", "control-centre-dock", "control-centre-desktop-icons"]) {
    const toggle = frame().locator(`#${id}`); const before = await toggle.getAttribute("aria-checked");
    await toggle.press("Space");
    await page.waitForTimeout(50);
    assert.notEqual(await toggle.getAttribute("aria-checked"), before, `${id} keyboard effect`);
    await toggle.click();
  }
  for (const option of await frame().locator('#control-centre-theme [data-theme-option]').all()) {
    await option.press("Enter");
    await page.waitForTimeout(50);
    assert.equal(await option.getAttribute("aria-checked"), "true", "Theme radio takes effect");
  }
  for (const option of await frame().locator('#control-centre-accent [data-accent-option]').all()) {
    await option.click();
    await page.waitForTimeout(50);
    assert.equal(await option.getAttribute("aria-checked"), "true", "Accent radio takes effect");
  }
  await frame().locator('#control-centre-accent-hex').fill('#123456');
  await frame().locator('#control-centre-accent-hex').press('Enter');
  await page.waitForFunction(() => window.calls.some(m => m.type === 'home:ui-preference' && m.key === 'accentCustom' && m.value === '#123456'));
  await frame().locator('#control-centre-show-windows').press('Enter');
  await frame().locator('#mission-spaces-shelf').waitFor({ state: 'visible' });
  await frame().locator('body').press('Escape');
  await frame().locator('#toolbar-control-centre').click();
  await frame().locator('#control-centre-approvals').click();
  await frame().locator('#inbox-rail').waitFor({ state: 'visible' });
  await page.waitForFunction(() => window.calls.some(m => m.type === 'home:launch-target' && m.target === 'inbox'));
  await frame().locator('#inbox-rail-close').press('Enter');
  await frame().locator('#toolbar-control-centre').click();
  await frame().locator('#control-centre-system').press('Enter');
  await frame().locator('.window[data-target="system"]').waitFor();
  await page.waitForTimeout(2700);
  await frame().locator('.window[data-target="system"] [data-action="close"]').click();
  await frame().locator('.window').waitFor({ state: 'detached' });
  assert.deepEqual(misses, [], "Visible UI stays on supported routes");
  assert.deepEqual(errors, [], "Fixture assets load");
  assert.deepEqual(pageErrors, [], "Source UI runs without script errors");
  assert.ok(requests.some(r => r.path === "/api/apps/home/state"), "Supported layout changes reach their route");
  assert.ok(requests.every(r => !["/api/apps/home/discovery", "/api/apps/home/desktop/objects"].includes(r.path)));
  console.log("isolation-ui-truth-browser-smoke: PASS (rendered refused publisher cases, signed models, keyboard Open, Home shortcut/file/layout controls)");
} finally {
  try {
    await browser?.close();
  } finally {
    await new Promise(done => server.close(done));
  }
}
