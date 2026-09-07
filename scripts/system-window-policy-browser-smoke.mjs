#!/usr/bin/env node
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { extname } from "node:path";
import test from "node:test";

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.ELASTOS_PLAYWRIGHT_MODULE || "playwright");
const origin = "https://system-window.test";
const read = (path) => readFile(new URL(`../${path}`, import.meta.url), "utf8");
const template = await read("capsules/home-gui/browser/home-gui-template.html");
const topHtml = `<iframe id="shell" sandbox="allow-scripts allow-forms allow-downloads" src="/shell"></iframe>
<script>
let cancelNextPasskey = false;
window.cancelFixturePasskey = () => { cancelNextPasskey = true; };
addEventListener('message', (event) => {
  const data = event.data;
  if (event.origin !== 'null' || !data || !/^system-token-[0-9]+$/.test(data.homeToken || '')) return;
  if (data.type === 'home:app-ready') event.source.postMessage({
    type: 'home:clipboard-ready', schema: 'elastos.home.clipboard.ready/v1',
    targetId: 'system', homeToken: data.homeToken, parentOrigin: location.origin,
    generation: 'fixture-generation',
  }, '*');
  if (data.type === 'elastos.home.passkey-step-up.request/v1') {
    event.source.postMessage({
      type: 'elastos.home.passkey-step-up.result/v1', requestId: data.requestId,
      ...(cancelNextPasskey ? {error:'Passkey verification cancelled'} : {stepUpToken:'fixture-step-up'}),
    }, '*');
    cancelNextPasskey = false;
  }
});
</script>`;
const shellHtml = `<link rel="stylesheet" href="/apps/home-gui/style.css">${template}
<script type="module">
import * as core from '/apps/home-gui/shell-core.js?v=home-20260813a';
import * as windows from '/apps/home-gui/shell-windows.js?v=home-20260813a';
document.querySelector('.desktop-workspace').hidden = false;
document.querySelector('.desktop-workspace').inert = false;
const target = {target:'system', title:'System', route:'/apps/system/', window_policy:'single'};
core.shellState.currentSummary = { targets:[target] };
const calls = [];
let release;
let gate = null;
windows.configureWindowHooks({
  clearIdentitySurface() {}, hideLauncher() {}, refreshLauncherIfVisible() {},
  renderDesktop() {}, renderTaskbar() {}, updateTaskbarState() {}, syncMenubar() {},
  async launchTarget(targetId, query) {
    if (targetId !== 'system') throw Error('unexpected target');
    calls.push({targetId, query});
    const serial = calls.length;
    if (gate) await gate;
    const params = new URLSearchParams({home_origin:${JSON.stringify(origin)}, ...query});
    return { ...target, attach_kind:'iframe', launch_status:'launched',
      route:'/apps/system/?'+params+'#home_token=system-token-'+serial };
  },
});
window.fixture = {
  windows, calls, state:core.shellState,
  hold() { gate = new Promise(resolve => { release = resolve; }); },
  release() { gate = null; release(); },
};
</script>`;

const browser = await chromium.launch({
  executablePath: process.env.BRAVE_BIN || "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
  headless: true,
});
try {
  async function fixture(t) {
    const context = await browser.newContext();
    const page = await context.newPage();
    const errors = [];
    t.after(async () => {
      await context.close();
      assert.deepEqual(errors, [], 'fixture finishes without page errors');
    });
    let exports = 0;
    page.on("pageerror", error => errors.push(error.message));
    // All traffic stays in this disposable fixture. No Runtime or user profile is used.
    await context.route("**/*", async route => {
      const url = new URL(route.request().url());
      assert.equal(url.origin, origin);
      const headers = { "access-control-allow-origin": "*" };
      if (route.request().method() === "OPTIONS") {
        return route.fulfill({ status: 204, headers: { ...headers,
          "access-control-allow-headers": "content-type,x-elastos-home-token,x-elastos-step-up",
          "access-control-allow-methods": "GET,POST,OPTIONS" } });
      }
      if (url.pathname.startsWith("/api/")) {
        let body;
        if (url.pathname === "/api/apps/system/summary") body = {
          // HomeAuthoritySummary and System's exact parseAppearance contract.
          authority: {signed_in:true, principal_id:'person:local:system-smoke',
            session_id:'fixture-session', proof_binding_id:'proof:passkey:system-smoke', wallet_connected:false},
          access: {role:'admin', localhost_root:'localhost://system', guest_registration_enabled:false},
          appearance:{schema:'elastos.home.appearance/v1', revision:5, theme:'dark', accent:'blue',
            accent_custom:'#4f7fff', dock_auto_hide:true, sounds:false, focus_mode:false,
            background_image_url:null, background_overlay_enabled:true, background_overlay_opacity:0.55},
          identity:{profile_readiness:{schema:'elastos.profile.readiness/v1', status:'ready'}},
        };
        else if (url.pathname === "/api/auth/recovery/status") body = {
          // PrincipalRootRecoveryStatusV1; fixture values contain no recovery material.
          schema:'elastos.principal.root-recovery.status/v1', root_encrypted:true,
          recovery_configured:true, recovery_download_available:true, protection_configured:true,
          principal_id:'person:local:system-smoke', localhost_root:'localhost://system', required_actions:[],
          crypto:{cipher:'aes-256-gcm', signatures:['ed25519','ml-dsa-65'],
            kems:['x25519','ml-kem-768'], recovery_kdf:'argon2id'},
        };
        else if (url.pathname === "/api/auth/recovery/full-export") {
          exports += 1;
          body = {schema:'elastos.full-recovery-bundle/v1', included:{people_identity:true}, bundle:{items:[]}};
        } else if (url.pathname === "/api/apps/home/active-shell") body = {active:'home-gui', candidates:[]};
        else if (url.pathname === "/api/auth/passkey/status") body = {registered:true};
        else if (url.pathname === "/api/auth/passkeys") body = {passkeys:[]};
        else if (url.pathname === "/api/capsules/catalog") body = {capsules:[]};
        else if (url.pathname === "/api/capsules/interfaces") body = {interfaces:[]};
        else if (url.pathname === "/api/provider/chain/networks") body = {status:'ok', data:{networks:[]}};
        else throw Error(`unexpected fixture API: ${url.pathname}`);
        return route.fulfill({ headers, json:body });
      }
      let body;
      let contentType = "text/html";
      if (url.pathname === "/") body = topHtml;
      else if (url.pathname === "/shell") body = shellHtml;
      else {
        const match = /^\/apps\/(home|home-gui|system)\/(.*)$/.exec(url.pathname);
        assert(match && !match[2].includes(".."), "fixture serves only the three capsule asset roots");
        const path = `capsules/${match[1]}/browser/${match[2] || "index.html"}`;
        body = await readFile(new URL(`../${path}`, import.meta.url));
        contentType = ({'.js':'text/javascript', '.mjs':'text/javascript', '.css':'text/css',
          '.svg':'image/svg+xml', '.png':'image/png', '.webp':'image/webp', '.woff2':'font/woff2'})[extname(path)] || 'text/html';
      }
      await route.fulfill({body, contentType, headers});
    });
    await page.goto(origin);
    const shell = await page.locator("#shell").elementHandle().then(handle => handle.contentFrame());
    await shell.waitForFunction(() => Boolean(window.fixture));
    async function currentSystem() {
      await shell.waitForSelector('.window-frame');
      const frame = await shell.locator('.window-frame').first().elementHandle().then(handle => handle.contentFrame());
      const bootstrap = await frame.waitForFunction(() => {
        const error = document.querySelector('.system-error:not([hidden]), [data-field="recovery-note"][data-tone="error"]:not([hidden])');
        if (error) return { ok:false };
        const status = document.querySelector('[data-field="recovery-status"]');
        const button = document.querySelector('#recovery-download');
        return status?.dataset.tone === 'success' && button && !button.disabled ? {ok:true} : false;
      });
      assert.equal((await bootstrap.jsonValue()).ok, true, 'System bootstrap must render verified recovery state without a handled error');
      assert.deepEqual(errors, [], "real Home/System bootstrap has no script error");
      return frame;
    }
    return {page, shell, currentSystem, exports:() => exports};
  }

  await test("ordinary System Open reuses its frame and preserves an editable draft", async t => {
    const f = await fixture(t);
    await f.shell.evaluate(() => fixture.windows.openTarget('system', {query:{settings:'security'}}));
    const system = await f.currentSystem();
    await system.fill('#recovery-password', 'fixture-unsaved-draft');
    await f.shell.evaluate(() => fixture.windows.openTarget('system'));
    await f.shell.evaluate(() => new Promise(requestAnimationFrame));
    assert.equal(await f.shell.locator('.window-frame').count(), 1);
    assert.equal(await system.inputValue('#recovery-password'), 'fixture-unsaved-draft');
  });

  await test("concurrent ordinary Open, settings link and Home Save share one System frame", async t => {
    const f = await fixture(t);
    await f.shell.evaluate(() => {
      fixture.hold();
      fixture.windows.openTarget('system');
      fixture.windows.openTarget('system', {query:{settings:'about'}});
      fixture.save = fixture.windows.openSystemRecoverySave();
      fixture.release();
    });
    await f.currentSystem();
    assert.equal(await f.shell.locator('.window-frame').count(), 1);
    assert.equal(await f.shell.evaluate(() => fixture.save), true);
    assert.equal(f.exports(), 1);
  });

  await test("an explicit settings link navigates the existing document without clearing its draft", async t => {
    const f = await fixture(t);
    await f.shell.evaluate(() => fixture.windows.openTarget('system', {query:{settings:'security'}}));
    const system = await f.currentSystem();
    await system.fill('#recovery-password', 'fixture-unsaved-draft');
    await f.shell.evaluate(() => fixture.windows.openTarget('system', {query:{settings:'about'}}));
    await system.waitForSelector('.settings-content.active[data-settings="about"]', {timeout:3000});
    assert.equal(await f.shell.locator('.window-frame').count(), 1);
    assert.equal(await system.inputValue('#recovery-password'), 'fixture-unsaved-draft');
    await f.shell.evaluate(() => fixture.windows.openTarget('system', {query:{settings:'security', recovery:'import'}}));
    await system.waitForFunction(() => document.activeElement?.id === 'recovery-import');
    assert.equal(await f.shell.locator('.window-frame').count(), 1);
    assert.equal(await system.inputValue('#recovery-password'), 'fixture-unsaved-draft');
    assert.equal(f.exports(), 0, 'settings navigation never exports');
  });

  await test("sequential Save, cancelled Save and retry use the same running System document", async t => {
    const f = await fixture(t);
    async function save() {
      await f.shell.evaluate(() => {
        fixture.saveResult = 'pending';
        fixture.windows.openSystemRecoverySave().then(ok => { fixture.saveResult = ok; });
      });
      await f.shell.waitForFunction(() => fixture.saveResult !== 'pending', null, {timeout:5000});
      return f.shell.evaluate(() => fixture.saveResult);
    }
    assert.equal(await save(), true);
    const system = await f.currentSystem();
    await system.evaluate(() => { window.fixtureDocumentMarker = 'original-document'; });
    assert.equal(f.exports(), 1);
    await f.page.evaluate(() => cancelFixturePasskey());
    assert.equal(await save(), false);
    assert.equal(f.exports(), 1);
    assert.equal(await save(), true);
    assert.equal(f.exports(), 2);
    assert.equal(await system.evaluate(() => window.fixtureDocumentMarker), 'original-document');
    assert.equal(await f.shell.locator('.window-frame').count(), 1);
    assert.equal(await f.shell.evaluate(() => fixture.calls.length), 1, 'reuse issues no new Runtime launch grant');
  });
} finally {
  await browser.close();
}
