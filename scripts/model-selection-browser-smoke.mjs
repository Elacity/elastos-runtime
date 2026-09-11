#!/usr/bin/env node
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { resolve, extname } from "node:path";
import { once } from "node:events";
import { chromium, brave } from "./system-uiux-fixture.mjs";

const root = resolve(new URL("../", import.meta.url).pathname);
const cid = `bafybei${"a".repeat(52)}`;
const offer = (id, title) => ({ id, title, operation: "text.generate", input_modalities: ["text/plain"], output_modalities: ["text/plain"] });
let offers, rows, workspace, failed = false, responseDelay = 0;
const calls = [], errors = [];
const server = createServer(async (req, res) => {
  const url = new URL(req.url, "http://fixture");
  const headers = { "access-control-allow-origin": "null", "access-control-allow-headers": "content-type,x-elastos-home-token", "access-control-allow-methods": "GET,POST,PUT,OPTIONS" };
  const json = (data, code = 200) => { res.writeHead(code, { ...headers, "content-type": "application/json" }); res.end(JSON.stringify(data)); };
  try {
    if (req.method === "OPTIONS") { res.writeHead(204, headers); res.end(); return; }
    if (url.pathname === "/") {
      const origin = `http://127.0.0.1:${server.address().port}`;
      res.setHeader("content-type", "text/html");
      res.end(`<iframe style="width:100%;height:95vh;border:0" sandbox="allow-scripts allow-forms" src="/apps/${url.searchParams.get("app")}/?home_origin=${encodeURIComponent(origin)}#home_token=fixture"></iframe><script>window.messages=[];addEventListener('message',event=>{messages.push(event.data);if(event.data.type==='home-agent:ready'){event.source.postMessage({type:'home-agent:open'},'*');event.source.postMessage({type:'home-agent:shelf-handover',on:true},'*');}});</script>`); return;
    }
    if (url.pathname.startsWith("/api/")) {
      assert.equal(req.headers["x-elastos-home-token"], "fixture");
      let body = ""; for await (const chunk of req) body += chunk;
      const input = body ? JSON.parse(body) : null;
      calls.push({ path: url.pathname, input });
      if (url.pathname.endsWith("offers_list")) {
        if (responseDelay) await new Promise(resolve => setTimeout(resolve, responseDelay));
        json({ offers }, failed ? 503 : 200); return;
      }
      if (url.pathname === "/api/capsules/catalog") { json({ schema: "elastos.capsules.catalog/v1", model_catalog_state: "verified", capsules: rows }); return; }
      if (url.pathname === "/api/apps/assistant/workspace-v2") {
        if (req.method === "PUT") {
          assert.equal(input.schema, "elastos.assistant.workspace/v2");
          if (input.if_revision !== workspace.revision) { json({ error: "revision conflict" }, 409); return; }
          workspace = { schema: input.schema, revision: workspace.revision + 1, document: input.document };
        }
        json(workspace); return;
      }
      throw new Error(`Unexpected operation ${url.pathname}`);
    }
    const match = url.pathname.match(/^\/apps\/([^/]+)\/(.*)$/);
    if (!match) { res.writeHead(404); res.end(); return; }
    const path = resolve(root, "capsules", match[1], "browser", match[2] || "index.html");
    assert.ok(path.startsWith(`${root}/capsules/`));
    const bytes = await readFile(path);
    res.writeHead(200, { ...headers, "content-type": ({ ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css" })[extname(path)] || "application/octet-stream" }); res.end(bytes);
  } catch (error) { errors.push(String(error)); json({ error: "fixture failure" }, 500); }
});
server.listen(0, "127.0.0.1"); await once(server, "listening");
const browser = await chromium.launch({ executablePath: brave, headless: true });
try {
  // The retired standalone renderer's disabled-select/CID/controller behavior
  // remains covered in assistant-shell-smoke.mjs. Render the canonical UI here.
  {
    const app = "assistant";
    workspace = { schema: "elastos.assistant.workspace/v2", revision: 0, document: {
      v: 1, liveOfferId: "chosen", selectedModelCid: cid, sessions: [],
      composerDraft: { text: "Keep my draft", parts: [] },
    } };
    offers = [offer("chosen", "Prepared model"), offer("hosted", "Hosted service")]; failed = false;
    rows = [{ source: "signed-model-catalog", role: "content", installed: false, launchable: false, cid,
      title: "Prepared model", signature_state: "catalog-signature-verified", model_runtime: { admitted: true, dispatch_ready: true, offer_id: "chosen" } }];
    const page = await browser.newPage({ viewport: { width: 1100, height: 800 } });
    const pageErrors = []; page.on("pageerror", error => pageErrors.push(String(error)));
    await page.goto(`http://127.0.0.1:${server.address().port}/?app=${app}`);
    const frame = page.frameLocator("iframe");
    const draft = frame.locator("#agent-composer-input");
    const send = frame.locator("#agent-composer-send");
    await draft.filter({ visible: true }).waitFor({ timeout: 5000 });
    await frame.locator("#agent-composer-send:enabled").waitFor();
    assert.equal(await draft.inputValue(), "Keep my draft");
    {
      for (const width of [680, 420]) {
        await page.setViewportSize({ width, height: 800 });
        await frame.locator("#agent-model-picker").click();
        await frame.locator("#agent-model-menu").waitFor({ state: "visible" });
        await frame.locator("#agent-model-menu").evaluate(async element => {
          await Promise.all(element.getAnimations().map(animation => animation.finished));
        });
        const controls = await frame.locator('#agent-model-menu [data-model-action="refresh-models"], #agent-model-menu [data-model-action="open-models"]').evaluateAll(buttons => buttons.map(button => {
          const rect = button.getBoundingClientRect();
          return { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom,
            width: rect.width, height: rect.height, viewportWidth: innerWidth, viewportHeight: innerHeight,
            clipped: button.scrollWidth > button.clientWidth };
        }));
        assert.equal(controls.length, 2);
        for (const rect of controls) {
          assert.ok(rect.width > 0 && rect.height > 0 && rect.left >= 0 && rect.top >= 0
            && rect.right <= rect.viewportWidth && rect.bottom <= rect.viewportHeight && !rect.clipped,
          `Assistant model control clipped at ${width}px: ${JSON.stringify(rect)}`);
        }
        const [a, b] = controls;
        assert.ok(a.right <= b.left || b.right <= a.left || a.bottom <= b.top || b.bottom <= a.top,
          `Assistant model controls overlap at ${width}px`);
        await frame.locator("#agent-model-picker").click();
      }
      await page.setViewportSize({ width: 1100, height: 800 });
    }
    await frame.locator("#agent-model-picker").click();
    const menu = frame.locator("#agent-model-menu");
    assert.equal(await menu.getByRole("option", { name: "Prepared model", exact: true }).count(), 1);
    assert.equal(await menu.getByRole("option", { name: "Hosted service", exact: true }).count(), 1);
    {
      offers = [offer("chosen", "Prepared model")];
      await menu.getByRole("button", { name: "Refresh models", exact: true }).click();
      const selected = menu.locator(`[data-model-cid="${cid}"][aria-selected="true"]`);
      await selected.waitFor({ state: "visible" });
      assert.equal(await menu.getByRole("option").count(), 1);
      assert.equal(await menu.locator(".agent-model-menu-empty").count(), 0,
        "A ready content model must replace the empty state");
      await frame.locator("#agent-model-picker").click();
      assert.equal(await frame.locator("#agent-model-picker").getAttribute("aria-expanded"), "false");
      await frame.locator("#agent-model-picker").click();
      responseDelay = 150;
      offers = [offer("chosen", "Prepared model"), offer("extra-a", "Another service"), offer("extra-b", "Third service")];
      await menu.getByRole("button", { name: "Refresh models", exact: true }).click();
      await selected.waitFor({ state: "visible" });
      await menu.getByRole("option", { name: "Third service", exact: true }).waitFor();
      responseDelay = 0;
      const geometry = await menu.evaluate(async element => {
        await Promise.all(element.getAnimations().map(animation => animation.finished));
        const rect = element.getBoundingClientRect();
        const anchor = document.querySelector("#agent-model-picker").getBoundingClientRect();
        return { top: rect.top, bottom: rect.bottom, left: rect.left, right: rect.right,
          gap: anchor.top - rect.bottom, viewportWidth: innerWidth, viewportHeight: innerHeight };
      });
      assert.ok(geometry.gap >= 9 && geometry.top >= 0 && geometry.left >= 0
        && geometry.right <= geometry.viewportWidth && geometry.bottom <= geometry.viewportHeight,
      `Refreshed model menu overlaps its picker or viewport: ${JSON.stringify(geometry)}`);
      await frame.locator("#agent-model-picker").click();
      assert.equal(await frame.locator("#agent-model-picker").getAttribute("aria-expanded"), "false");
      assert.equal(await draft.inputValue(), "Keep my draft");
      await frame.locator("#agent-model-picker").click();
    }
    offers = [offer("hosted", "Hosted service")]; rows = [];
    await menu.getByRole("button", { name: "Refresh models", exact: true }).click();
    await menu.getByRole("option", { name: "Prepared model", exact: true }).waitFor({ state: "detached" });
    assert.equal(await send.isDisabled(), true);
    assert.equal(await draft.inputValue(), "Keep my draft");
    await menu.getByRole("option", { name: "Hosted service", exact: true }).click();
    assert.equal(await send.isEnabled(), true);
    assert.equal(await draft.inputValue(), "Keep my draft");
    failed = true;
    await frame.locator("#agent-model-picker").click();
    await menu.getByRole("button", { name: "Refresh models", exact: true }).click();
    await frame.locator("#agent-composer-send:disabled").waitFor();
    await menu.locator('[role="option"][aria-selected="true"]').waitFor({ state: "detached" });
    assert.equal(await menu.locator('[role="option"][aria-selected="true"]').count(), 0, "failed refresh leaves no selectable current model");
    await menu.getByRole("button", { name: "Open Models", exact: true }).click();
    const message = await page.waitForFunction(() => window.messages.find(m => m.type === "home:open-target"));
    assert.deepEqual(await message.jsonValue(), { type: "home:open-target", homeToken: "fixture", target: "system", query: { settings: "models" } });
    assert.equal(await draft.inputValue(), "Keep my draft");
    assert.deepEqual(pageErrors, []);
    await page.close();
  }
  assert.equal(calls.filter(call => call.path.endsWith("runs_create")).length, 0);
  assert.deepEqual(errors, []);
  console.log("PASS canonical Assistant model selection browser smoke");
} finally { await browser.close(); server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); }
