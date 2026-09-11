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
let offers, rows, failed = false;
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
      if (url.pathname.endsWith("offers_list")) { json({ offers }, failed ? 503 : 200); return; }
      if (url.pathname === "/api/capsules/catalog") { json({ schema: "elastos.capsules.catalog/v1", model_catalog_state: "verified", capsules: rows }); return; }
      if (url.pathname.endsWith("workspace")) {
        if (req.method === "PUT") { json({ ...input, revision: input.if_revision + 1 }); return; }
        json(url.pathname.includes("home-agent")
          ? { schema: "elastos.home-agent.workspace/v1", revision: 0, document: { v: 1, liveOfferId: "chosen", selectedModelCid: cid, composerDraft: { text: "Keep my draft", parts: [] } } }
          : { schema: "elastos.assistant.workspace/v1", revision: 0, sessions: [], draft: "Keep my draft", selected_offer_id: "chosen", selected_model_cid: cid }); return;
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
  for (const app of ["assistant", "home-agent"]) {
    offers = [offer("chosen", "Prepared model"), offer("hosted", "Hosted service")]; failed = false;
    rows = [{ source: "signed-model-catalog", role: "content", installed: false, launchable: false, cid,
      title: "Prepared model", signature_state: "catalog-signature-verified", model_runtime: { admitted: true, dispatch_ready: true, offer_id: "chosen" } }];
    const page = await browser.newPage({ viewport: { width: 1100, height: 800 } });
    const pageErrors = []; page.on("pageerror", error => pageErrors.push(String(error)));
    await page.goto(`http://127.0.0.1:${server.address().port}/?app=${app}`);
    const frame = page.frameLocator("iframe");
    const agent = app === "home-agent";
    const draft = frame.locator(agent ? "#agent-composer-input" : "#assistant-composer-input");
    const send = frame.locator(agent ? "#agent-composer-send" : "#assistant-send");
    await draft.filter({ visible: true }).waitFor({ timeout: 5000 });
    await frame.locator(`${agent ? "#agent-composer-send" : "#assistant-send"}:enabled`).waitFor();
    assert.equal(await draft.inputValue(), "Keep my draft");
    if (!agent) {
      for (const width of [680, 420]) {
        await page.setViewportSize({ width, height: 800 });
        const controls = await frame.locator('[data-action="refresh-models"], [data-action="open-models"]').evaluateAll(buttons => buttons.map(button => {
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
      }
      await page.setViewportSize({ width: 1100, height: 800 });
    }
    if (agent) await frame.locator("#agent-model-picker").click();
    const menu = agent ? frame.locator("#agent-model-menu") : frame.locator("#assistant-offer-select");
    assert.equal(await menu.getByRole("option", { name: "Prepared model", exact: true }).count(), 1);
    assert.equal(await menu.getByRole("option", { name: "Hosted service", exact: true }).count(), 1);
    offers = [offer("hosted", "Hosted service")]; rows = [];
    await (agent ? menu : frame).getByRole("button", { name: "Refresh models", exact: true }).click();
    await menu.getByRole("option", { name: "Prepared model", exact: true }).waitFor({ state: "detached" });
    assert.equal(await send.isDisabled(), true);
    assert.equal(await draft.inputValue(), "Keep my draft");
    if (agent) await menu.getByRole("option", { name: "Hosted service", exact: true }).click();
    else await menu.selectOption("hosted");
    assert.equal(await send.isEnabled(), true);
    assert.equal(await draft.inputValue(), "Keep my draft");
    failed = true;
    if (agent) await frame.locator("#agent-model-picker").click();
    await (agent ? menu : frame).getByRole("button", { name: "Refresh models", exact: true }).click();
    await frame.locator(`${agent ? "#agent-composer-send" : "#assistant-send"}:disabled`).waitFor();
    if (!agent) assert.equal(await menu.isDisabled(), true);
    await (agent ? menu : frame).getByRole("button", { name: "Open Models", exact: true }).click();
    const message = await page.waitForFunction(() => window.messages.find(m => m.type === "home:open-target"));
    assert.deepEqual(await message.jsonValue(), { type: "home:open-target", homeToken: "fixture", target: "system", query: { settings: "models" } });
    assert.equal(await draft.inputValue(), "Keep my draft");
    assert.deepEqual(pageErrors, []);
    await page.close();
  }
  assert.equal(calls.filter(call => call.path.endsWith("runs_create")).length, 0);
  assert.deepEqual(errors, []);
  console.log("PASS Assistant/Home Agent model selection browser smoke");
} finally { await browser.close(); server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); }
