import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import http from "node:http";
import { extname } from "node:path";
import test from "node:test";
import { openInstalledAssistant, returnInstalledHome } from "./ci-installed-home-navigation.mjs";
import { publicJourneyFailure } from "./ci-installed-home-failure.mjs";

const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");
const dockStyle = readFileSync(new URL("../capsules/home-gui/browser/style.css", import.meta.url));

test("installed recovery uses Assistant's visible Home control while Home's dock is hidden", async t => {
  // The fixture isolates navigation from model execution. Hosted installed CI
  // owns acceptance of the real Runtime, model and complete Home journey.
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, "http://localhost");
    response.setHeader("content-type", "text/html");
    if (url.pathname === "/dock.css") {
      response.setHeader("content-type", "text/css");
      response.end(dockStyle);
    } else if (url.pathname === "/shell") {
      response.end(`<!doctype html><link rel="stylesheet" href="/dock.css">
        <div id="desktop" hidden>Home desktop</div>
        <div id="assistant-space"><iframe id="assistant-space-frame" src="/assistant"></iframe></div>
        <div class="taskbar is-assistant-face"><div class="taskbar-primary">
          <button id="assistant-toggle">Assistant</button>
        </div></div>
        <script>
          addEventListener("message", event => {
            if (event.source !== document.querySelector("#assistant-space-frame").contentWindow ||
                event.data?.type !== "home-agent:close") return;
            if (new URLSearchParams(location.search).get("failure") !== "room_open") {
              document.querySelector("#assistant-space").hidden = true;
              document.querySelector("#desktop").hidden = false;
            }
          });
        </script>`);
    } else if (url.pathname === "/assistant") {
      response.end(`<!doctype html><button id="agent-shelf-flip-back" aria-label="Back to Home">Home</button>
        <script>
          document.querySelector("#agent-shelf-flip-back").addEventListener("click", () => {
            parent.postMessage({ type: "home-agent:close" }, location.origin);
          });
        </script>`);
    } else if (url.pathname === "/") {
      const failure = url.searchParams.get("failure") ?? "";
      response.end(`<!doctype html><body data-home-status="${failure === "home_unready" ? "loading" : "ready"}">
        <iframe id="active-shell-frame" src="/shell?failure=${failure}"></iframe>`);
    } else response.writeHead(404).end();
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  let browser;
  try {
    browser = await chromium.launch({ headless: true });
    for (const failure of ["", "room_open", "home_unready"]) {
      await t.test(failure || "visible Home returns to ready desktop", async () => {
        const page = await browser.newPage();
        try {
          await page.goto(`http://127.0.0.1:${server.address().port}/?failure=${failure}`);
          const shell = page.frameLocator("#active-shell-frame");
          await shell.locator("#assistant-toggle").waitFor({ state: "hidden" });
          const assistantElement = await shell.locator("#assistant-space-frame").elementHandle();
          const assistant = await assistantElement.contentFrame();
          await assistant.locator("#agent-shelf-flip-back").waitFor({ state: "visible" });
          let screenshot;
          const takeScreenshot = page.screenshot.bind(page);
          page.screenshot = async options => {
            screenshot = options;
            const png = await takeScreenshot({ fullPage: options.fullPage });
            assert.equal(png.subarray(1, 4).toString(), "PNG");
          };
          if (failure) {
            const results = { memory_refusal: "passed" };
            let error;
            try { await returnInstalledHome(page, assistant, "/isolated-evidence", 3000); }
            catch (caught) { error = caught; }
            assert(error, "failed recovery keeps item 7 open");
            assert.equal(screenshot, undefined);
            const receipt = publicJourneyFailure(error, "finite_home_recovery");
            results[receipt.failure_stage] = "failed";
            assert.equal(results.memory_refusal, "passed");
            assert.equal(results.finite_home_recovery, "failed");
            assert.equal(receipt.failure, failure === "room_open" ? "timeout" : "assertion_failed");
          } else {
            await returnInstalledHome(page, assistant, "/isolated-evidence", 3000);
            assert.deepEqual(screenshot, { path: "/isolated-evidence/finite-home-recovery.png", fullPage: true });
            assert.equal(await shell.locator("#assistant-space").isVisible(), false);
            assert.equal(await shell.locator("#desktop").isVisible(), true);
          }
        } finally { await page.close(); }
      });
    }
  } finally {
    await browser?.close();
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
});

test("installed contender opens the real Assistant through a signed, ready Home", async t => {
  // Serve the real capsule entry, modules and styles; only Home and Runtime responses are fixtures.
  const capsuleRoots = {
    assistant: new URL("../capsules/assistant/browser/", import.meta.url),
    home: new URL("../capsules/home/browser/", import.meta.url),
  };
  const schema = "elastos.assistant.workspace/v2";
  let workspace = { schema, revision: 0, document: {} };
  const errors = [];
  const server = http.createServer(async (request, response) => {
    const url = new URL(request.url, "http://fixture");
    const headers = { "access-control-allow-origin": "null", "access-control-allow-headers": "content-type,x-elastos-home-token",
      "access-control-allow-methods": "GET,POST,PUT,OPTIONS", "cache-control": "no-store" };
    const json = (value, status = 200) => { response.writeHead(status, { ...headers, "content-type": "application/json" }); response.end(JSON.stringify(value)); };
    try {
      if (request.method === "OPTIONS") { response.writeHead(204, headers).end(); return; }
      if (url.pathname === "/home/") {
        const mode = url.searchParams.get("mode") || "refreshed";
        response.writeHead(200, { "content-type": "text/html" });
        response.end(`<!doctype html><style>html,body,iframe{width:100%;height:100%;margin:0;border:0}</style>
          <body data-home-status="${mode === "unready" ? "loading" : "ready"}" data-home-authority="${["unsigned", "unlock"].includes(mode) ? "unsigned" : "signed"}">
          <div id="home-unlock" ${mode === "unlock" ? "" : "hidden"}><button id="home-unlock-primary">Sign in</button></div>
          <iframe id="active-shell-frame" src="/shell?mode=${mode}"></iframe>`);
        return;
      }
      if (url.pathname === "/shell") {
        response.writeHead(200, { "content-type": "text/html" });
        response.end(`<!doctype html><style>html,body,iframe{width:100%;height:100%;margin:0;border:0}</style>
          <body data-home-status="ready" data-home-authority="signed">
          <div id="desktop">Home desktop</div>
          <div id="setup-sheet"><button id="setup-sheet-close">Close setup</button></div>
          <button id="assistant-toggle">Assistant</button>
          <div id="assistant-space" hidden><iframe id="assistant-space-frame" sandbox="allow-scripts allow-forms" hidden></iframe></div>
          <script>
            window.launches=0;
            const frame=document.querySelector('#assistant-space-frame');
            document.querySelector('#setup-sheet-close').addEventListener('click',()=>document.querySelector('#setup-sheet').hidden=true);
            document.querySelector('#assistant-toggle').addEventListener('click',()=>{
              window.launches++;document.querySelector('#desktop').hidden=true;
              document.querySelector('#assistant-space').hidden=false;frame.hidden=false;
              frame.src='/apps/assistant/#home_token=fixture';
            });
            addEventListener('message',event=>{
              if(event.source!==frame.contentWindow||event.data?.type!=='home-agent:ready'||
                new URLSearchParams(location.search).get('mode')==='room_unready')return;
              frame.contentWindow.postMessage({type:'home-agent:open'},'*');
              frame.contentWindow.postMessage({type:'home-agent:shelf-handover',on:true},'*');
            });
          </script>`);
        return;
      }
      if (url.pathname.startsWith("/api/")) {
        assert.equal(request.headers["x-elastos-home-token"], "fixture");
        if (url.pathname === "/api/apps/assistant/workspace-v2") {
          if (request.method === "PUT") {
            let body = ""; for await (const chunk of request) body += chunk;
            const input = JSON.parse(body);
            assert.equal(input.schema, schema);
            if (input.if_revision !== workspace.revision) { json({ error: "workspace revision conflict" }, 409); return; }
            workspace = { schema, revision: workspace.revision + 1, document: input.document };
          }
          json(workspace); return;
        }
        if (url.pathname === "/api/provider/model/offers_list") { json({ offers: [] }); return; }
        if (url.pathname === "/api/capsules/catalog") { json({ schema: "elastos.capsules.catalog/v1", model_catalog_state: "verified", capsules: [] }); return; }
        throw new Error("unexpected fixture operation");
      }
      if (url.pathname === "/favicon.ico") { response.writeHead(204).end(); return; }
      const path = /^\/apps\/(assistant|home)\/(.*)$/.exec(url.pathname);
      assert(path, "fixture serves only Assistant and its Home dependencies");
      const root = capsuleRoots[path[1]];
      const asset = new URL(path[2] || "index.html", root);
      assert(asset.href.startsWith(root.href));
      const bytes = await readFile(asset);
      response.writeHead(200, { ...headers, "content-type": ({ ".html": "text/html", ".js": "text/javascript", ".css": "text/css",
        ".mjs": "text/javascript", ".svg": "image/svg+xml", ".woff2": "font/woff2" })[extname(asset.pathname)] || "application/octet-stream" });
      response.end(bytes);
    } catch (error) { errors.push(error); json({ error: "fixture failure" }, 500); }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  let browser;
  try {
    browser = await chromium.launch({ headless: true });
    const origin = `http://127.0.0.1:${server.address().port}`;
    await t.test("direct top-level capsule cannot raise its own room", async () => {
      const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
      try {
        await page.goto(`${origin}/apps/assistant/#home_token=fixture`);
        await page.waitForFunction(() => document.querySelector("#agent-harness-new-chat")?.dataset.boundNewChat === "1", null, { timeout: 5000 });
        await page.evaluate(() => window.postMessage({ type: "home-agent:open" }, "*"));
        await assert.rejects(page.locator("#agent-harness-new-chat").click({ timeout: 1000 }), error => error.name === "TimeoutError");
        assert.equal(await page.locator("#agent-harness").getAttribute("aria-hidden"), "true");
        assert.equal(await page.locator("#agent-harness").evaluate(room => getComputedStyle(room).pointerEvents), "none");
      } finally { await page.close(); }
    });
    for (const mode of ["refreshed", "unlock", "unsigned", "unready", "room_unready"]) {
      await t.test(mode, async () => {
        const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
        const pageErrors = [];
        page.on("pageerror", error => pageErrors.push(error));
        try {
          const launch = () => openInstalledAssistant(page, `${origin}/home/?mode=${mode}`, mode === "refreshed" ? 5000 : 1000);
          if (mode !== "refreshed") {
            await assert.rejects(launch, error => error.name === (mode === "unlock" ? "AssertionError" : "TimeoutError"));
            assert.equal(await page.frameLocator("#active-shell-frame").locator("body").evaluate(() => window.launches), mode === "room_unready" ? 1 : 0);
          } else {
            const assistant = await launch();
            assert.notEqual(assistant, page.mainFrame());
            assert.equal(await page.frameLocator("#active-shell-frame").locator("#setup-sheet").isVisible(), false);
            await assistant.locator("#agent-harness-new-chat").click();
            await assistant.locator('[data-sidebar-nav="configure"]').click();
            await assistant.locator('[data-configure-section="prompt"]').click();
            await assistant.locator("[data-agent-max-tokens]").fill("16");
            await assistant.locator("[data-page-close]").click();
            assert.equal(await assistant.locator("#agent-harness").evaluate(room => getComputedStyle(room).pointerEvents), "auto");
          }
          assert.deepEqual(pageErrors, []);
        } finally { await page.close(); }
      });
    }
    assert.deepEqual(errors, []);
  } finally {
    await browser?.close();
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
});

test("installed contender waits for real Home GUI session restoration before Assistant", async t => {
  // Runtime responses are controlled; Shell restoration and the Assistant handoff use their real modules.
  const capsuleRoots = Object.fromEntries(["home-gui", "assistant", "home"]
    .map(name => [name, new URL(`../capsules/${name}/browser/`, import.meta.url)]));
  const browserContextId = `browser:${"a".repeat(32)}`;
  const targets = ["marketplace", "wallet", "assistant"].map(target => ({
    target, title: target[0].toUpperCase() + target.slice(1), role: "app", attach_kind: "iframe", launchable: true,
  }));
  const summary = {
    authority: { signed_in: true, principal_id: "did:key:fixture", proof_binding_id: "fixture", session_id: "fixture" },
    identity: { display_name: "Fixture Home", recovery_readiness: { schema: "elastos.recovery.readiness/v1", status: "ready" },
      profile_readiness: { schema: "elastos.profile.readiness/v1", status: "ready" } },
    targets, active_shell: { active: "home-gui", candidates: [] },
    appearance: { schema: "elastos.home.appearance/v1", revision: 0, theme: "dark", accent: "blue", accent_custom: "#4f7fff",
      dock_auto_hide: false, sounds: false, focus_mode: false, background_image_url: null,
      background_overlay_enabled: true, background_overlay_opacity: 0.55 },
    browser_state: { principal_id: "did:key:fixture", recent_targets: [],
      session: { browser_context_id: browserContextId, root_shell: "home-gui", active_stage: "desk-1", space_order: ["desk-1"],
        windows: [{ target: "marketplace", x: 20, y: 80, width: 480, height: 360 },
          { target: "wallet", x: 520, y: 80, width: 480, height: 360 }] },
      layout: { taskbar: ["marketplace", "wallet"], desktop: {}, desktopHidden: [], desktopLabels: {}, desktopIconsVisible: true } },
    desktop_objects: { objects: [] }, people: { contacts: [], discovery: { enabled: true, status: "visible", expires_at: 9999999999 } },
    notifications: { unread_count: 0, attention_count: 0, entries: [] }, room: {}, runtime: {}, services: {},
  };
  const schema = "elastos.assistant.workspace/v2";
  let workspace = { schema, revision: 0, document: {} };
  const errors = [];
  const server = http.createServer(async (request, response) => {
    const url = new URL(request.url, "http://fixture");
    const headers = { "access-control-allow-origin": "null", "access-control-allow-headers": "content-type,x-elastos-home-token",
      "access-control-allow-methods": "GET,POST,PUT,OPTIONS", "cache-control": "no-store" };
    const json = (value, status = 200) => { response.writeHead(status, { ...headers, "content-type": "application/json" }); response.end(JSON.stringify(value)); };
    try {
      if (request.method === "OPTIONS") { response.writeHead(204, headers).end(); return; }
      if (url.pathname === "/home/") {
        const mode = url.searchParams.get("mode") || "restored";
        const origin = `http://127.0.0.1:${server.address().port}`;
        response.writeHead(200, { "content-type": "text/html", "cache-control": "no-store" });
        response.end(`<!doctype html><style>html,body,iframe{width:100%;height:100%;margin:0;border:0}</style>
          <body data-home-status="ready" data-home-authority="signed"><div id="home-unlock" hidden></div>
          <iframe id="active-shell-frame" sandbox="allow-scripts allow-forms"
            src="/apps/home-gui/?home_origin=${encodeURIComponent(origin)}#home_token=fixture"></iframe>
          <script>
            window.calls=[];window.pendingSavedWindows=[];let released=false;
            const frame=document.querySelector('#active-shell-frame');
            const reply=(message)=>frame.contentWindow.postMessage(message,'*');
            const launch=(message)=>{
              if(message.target==='assistant'&&${JSON.stringify(mode)}==='launch_refused'){
                reply({type:'home:shell-response',requestId:message.requestId,error:'Fixture Assistant launch refused',status:403});return;
              }
              reply({type:'home:shell-response',requestId:message.requestId,result:{target:message.target,title:message.target,
                attach_kind:'iframe',route:message.target==='assistant'?'/apps/assistant/?home_origin=${encodeURIComponent(origin)}#home_token=fixture'
                  :'/restored-app?target='+message.target+'#home_token=fixture'}});
            };
            window.releaseSavedWindows=()=>{released=true;window.pendingSavedWindows.splice(0).forEach(launch);};
            addEventListener('message',event=>{
              if(event.source!==frame.contentWindow||event.data?.homeToken!=='fixture')return;
              const message=event.data;window.calls.push(message);
              if(message.type==='home:shell-ready'){
                reply({type:'home:shell-context',browserContextId:${JSON.stringify(browserContextId)}});
                reply({type:'home:shell-summary',summary:${JSON.stringify(summary)}});
              }
              if(message.type==='home:launch-target'){
                if(message.target!=='assistant'&&!released)window.pendingSavedWindows.push(message);
                else launch(message);
              }
            });
          </script>`);
        return;
      }
      if (url.pathname === "/restored-app") {
        response.writeHead(200, { ...headers, "content-type": "text/html" }); response.end("<!doctype html><p>Restored app</p>"); return;
      }
      if (url.pathname.startsWith("/api/")) {
        assert.equal(request.headers["x-elastos-home-token"], "fixture");
        if (url.pathname === "/api/apps/home/state") { json({}); return; }
        if (url.pathname === "/api/apps/assistant/workspace-v2") {
          if (request.method === "PUT") {
            let body = ""; for await (const chunk of request) body += chunk;
            const input = JSON.parse(body); assert.equal(input.schema, schema);
            if (input.if_revision !== workspace.revision) { json({ error: "workspace revision conflict" }, 409); return; }
            workspace = { schema, revision: workspace.revision + 1, document: input.document };
          }
          json(workspace); return;
        }
        if (url.pathname === "/api/provider/model/offers_list") { json({ offers: [] }); return; }
        if (url.pathname === "/api/capsules/catalog") { json({ schema: "elastos.capsules.catalog/v1", model_catalog_state: "verified", capsules: [] }); return; }
        throw new Error("unexpected restoration fixture operation");
      }
      if (url.pathname === "/favicon.ico") { response.writeHead(204).end(); return; }
      const path = /^\/apps\/(home-gui|assistant|home)\/(.*)$/.exec(url.pathname);
      assert(path, "restoration fixture serves Home GUI, Assistant and Home dependencies");
      const root = capsuleRoots[path[1]];
      const asset = new URL(path[2] || "index.html", root);
      assert(asset.href.startsWith(root.href));
      const bytes = await readFile(asset);
      response.writeHead(200, { ...headers, "content-type": ({ ".html": "text/html", ".js": "text/javascript", ".css": "text/css",
        ".mjs": "text/javascript", ".svg": "image/svg+xml", ".woff2": "font/woff2" })[extname(asset.pathname)] || "application/octet-stream" });
      response.end(bytes);
    } catch (error) { errors.push(error); json({ error: "fixture failure" }, 500); }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  let browser;
  try {
    browser = await chromium.launch({ headless: true });
    const origin = `http://127.0.0.1:${server.address().port}`;
    for (const mode of ["restored", "shell_unready", "launch_refused"]) {
      await t.test(mode, async () => {
        const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
        const pageErrors = [];
        page.on("pageerror", error => pageErrors.push(error));
        await page.addInitScript(() => {
          window.assistantClicks = [];
          document.addEventListener("click", event => {
            if (event.target.closest?.("#assistant-toggle")) window.assistantClicks.push({
              status: document.body.dataset.homeStatus, authority: document.body.dataset.homeAuthority,
            });
          }, true);
        });
        let opening;
        try {
          opening = openInstalledAssistant(page, `${origin}/home/?mode=${mode}`, mode === "restored" ? 10000 : 2000);
          opening.catch(() => {});
          await page.waitForFunction(() => window.pendingSavedWindows?.some(message => message.target === "marketplace"), null, { timeout: 5000 });
          const shell = page.frameLocator("#active-shell-frame");
          assert.equal(await page.getAttribute("body", "data-home-status"), "ready");
          assert.equal(await shell.locator("body").getAttribute("data-home-authority"), "signed");
          assert.equal(await shell.locator("body").getAttribute("data-home-status"), "booting");
          assert.equal(await shell.locator("#desktop").isVisible(), true);
          assert.equal(await shell.locator("#assistant-toggle").isVisible(), true);
          assert.deepEqual(await shell.locator("body").evaluate(() => window.assistantClicks), []);
          if (mode === "shell_unready") {
            await assert.rejects(opening, error => error.name === "TimeoutError");
            assert.deepEqual(await shell.locator("body").evaluate(() => window.assistantClicks), []);
          } else {
            await page.evaluate(() => window.releaseSavedWindows());
            if (mode === "launch_refused") {
              await assert.rejects(opening, error => error.name === "TimeoutError");
              assert.equal(await shell.locator("#assistant-space-error").isVisible(), true);
              assert.match(await shell.locator("#assistant-space-error-detail").textContent(), /Fixture Assistant launch refused/);
              assert.equal(await shell.locator("#assistant-space-frame").isVisible(), false);
            } else {
              const assistant = await opening;
              await assistant.locator('html[data-shelf-handover="1"]').waitFor({ state: "attached", timeout: 5000 });
              assert.equal(await assistant.locator("html").getAttribute("data-shelf-handover"), "1");
              await assistant.locator("#agent-harness-new-chat").click();
              await assistant.locator("#agent-shelf-flip-back").click();
              await shell.locator("#assistant-space").waitFor({ state: "hidden", timeout: 5000 });
              assert.equal(await shell.locator("#desktop").isVisible(), true);
            }
            assert.deepEqual(await shell.locator("body").evaluate(() => window.assistantClicks), [{ status: "ready", authority: "signed" }]);
            assert.equal(await shell.locator('.window[data-target="marketplace"]').count(), 1);
            assert.equal(await shell.locator('.window[data-target="wallet"]').count(), 1);
          }
          const launches = await page.evaluate(() => window.calls.filter(message => message.type === "home:launch-target").map(message => message.target));
          assert.equal(launches.filter(target => target === "assistant").length, mode === "shell_unready" ? 0 : 1);
          assert.equal(launches.filter(target => target === "marketplace").length, 1);
          assert(launches.every(target => ["marketplace", "wallet", "assistant"].includes(target)));
          assert.deepEqual(pageErrors, []);
        } finally { await page.close(); await opening?.catch(() => {}); }
      });
    }
    assert.deepEqual(errors, []);
  } finally {
    await browser?.close();
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
});
