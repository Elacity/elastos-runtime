import test from "node:test";
import assert from "node:assert/strict";
import { chromium, brave, jsonResponse, makeAppearanceRecord, makeSystemSummary, startSystemFixtureServer } from "./system-uiux-fixture.mjs";

test("System channel choice confirms the exact owner intent, refreshes offers and keeps a test install", async () => {
  let channel = "canary", role = "admin", reads = 0, refuse = false, phase = "ready";
  const writes = [];
  const server = await startSystemFixtureServer({
    title: "System update channel",
    hostScript: `window.approvals = [];
      window.addEventListener("message", event => {
        if (event.source !== document.querySelector("iframe").contentWindow) return;
        if (event.data?.type !== "elastos.home.passkey-step-up.request/v1") return;
        window.approvals.push(event.data);
        event.source.postMessage({type:"elastos.home.passkey-step-up.result/v1", requestId:event.data.requestId, stepUpToken:"fixture-approval"}, "*");
      });`,
    async onApiRequest({ request, response, url }) {
      if (url.pathname === "/api/apps/system/summary") {
        reads++;
        const summary = makeSystemSummary(makeAppearanceRecord());
        summary.runtime.version = "2.1.0-rc.1";
        summary.access.role = role;
        summary.source = { configured:true, name:"default", channel, mode:"release", installed_version:"2.1.0-rc.1", update_checks_allowed:true };
        summary.runtime_update = { configured:true, available:false, message:`Checked ${channel} updates.`, controller:{phase} };
        jsonResponse(response, summary);
        return true;
      }
      if (url.pathname === "/api/apps/system/update-channel") {
        let body = "";
        for await (const chunk of request) body += chunk;
        writes.push({ body:JSON.parse(body), token:request.headers["x-elastos-home-token"] });
        if (refuse) jsonResponse(response, {error:"Channel change refused."}, 403);
        else {
          channel = JSON.parse(body).channel;
          jsonResponse(response, {channel, installed_version:"2.1.0-rc.1"});
        }
        return true;
      }
      return false;
    },
  });
  const browser = await chromium.launch({ executablePath:brave, headless:true });
  try {
    const page = await browser.newPage({viewport:{width:390,height:844}});
    async function open(busy = false) {
      await page.goto(`${server.baseUrl}/fixture?settings=about`);
      const frame = await page.waitForSelector("#system-frame").then(handle => handle.contentFrame());
      await frame.waitForSelector("#source-channel-choice", {timeout:3000});
      await frame.waitForFunction(() => document.querySelector('[data-field="source-installed-version"]').textContent === "2.1.0-rc.1");
      if (!busy) await frame.waitForFunction(() => !document.querySelector("#source-channel-choice").disabled);
      return frame;
    }
    let frame = await open();
    const select = frame.locator("#source-channel-choice");
    assert.equal(await select.inputValue(), "canary");
    assert.deepEqual(await select.locator("option").evaluateAll(nodes => nodes.map(n => [n.value,n.textContent])), [["stable","Stable"],["canary","Test versions"]]);
    for (const width of [390, 1280]) {
      await page.setViewportSize({width,height:844});
      await select.scrollIntoViewIfNeeded();
      const bounds = await select.evaluate(node => {
        const r = node.getBoundingClientRect();
        const p = node.closest(".settings-content-container").getBoundingClientRect();
        return {left:r.left,right:r.right,containerLeft:p.left,containerRight:p.right,width:innerWidth};
      });
      assert.ok(bounds.left >= bounds.containerLeft && bounds.right <= bounds.containerRight && bounds.right <= bounds.width,
        `channel choice fits at ${width}px: ${JSON.stringify(bounds)}`);
      await page.screenshot({path:`/tmp/elastos-201-channel-${width}.png`});
    }
    const before = reads;
    await select.selectOption("stable");
    await frame.waitForFunction(() => document.querySelector("#source-channel-status").textContent.includes("next stable release newer"));
    await frame.waitForFunction(() => document.querySelector("#runtime-update-status").textContent === "Checked stable updates.");
    assert.ok(reads > before, "channel change rechecks updates");
    assert.deepEqual(writes[0], {body:{source_name:"default", channel:"stable", step_up_token:"fixture-approval"}, token:"system-token"});
    assert.deepEqual(await page.evaluate(() => window.approvals.map(a => [a.operation,a.request])), [["system.source.switch-channel",{source_name:"default",channel:"stable"}]]);
    assert.equal(await frame.locator('[data-field="source-installed-version"]').textContent(), "2.1.0-rc.1");
    refuse = true;
    await select.selectOption("canary");
    await frame.waitForFunction(() => !document.querySelector("#source-channel-choice").disabled);
    assert.equal(await select.inputValue(), "stable", "failed change keeps saved channel");
    assert.match(await frame.locator("#source-channel-status").textContent(), /could not be confirmed/i);
    refuse = false;
    for (phase of ["staging", "downloading", "verifying", "restarting"]) {
      frame = await open(true);
      assert.equal(await frame.locator("#source-channel-choice").isDisabled(), true, phase);
      const before = writes.length;
      await frame.locator("#source-channel-choice").evaluate(node => {
        node.value = "canary";
        node.dispatchEvent(new Event("change", {bubbles:true}));
      });
      assert.equal(writes.length, before, "busy controller keeps the channel");
      assert.equal(await frame.locator("#source-channel-choice").inputValue(), "stable");
    }
    phase = "ready";
    channel = "jetson-test";
    frame = await open();
    assert.equal(await frame.locator("#source-channel-choice").inputValue(), "");
    assert.match(await frame.locator("#source-channel-status").textContent(), /This channel has ended/);
    assert.equal(await frame.locator('[data-field="source-channel"]').textContent(), "Retired test channel");
    await frame.selectOption("#source-channel-choice", "canary");
    await frame.waitForFunction(() => document.querySelector("#runtime-update-status").textContent === "Checked canary updates.");
    assert.equal(channel, "canary", "retired channel can leave");
    role = "guest";
    await page.reload();
    frame = await page.locator("#system-frame").elementHandle().then(h => h.contentFrame());
    await frame.waitForSelector("#source-channel-choice", {state:"attached"});
    await frame.waitForFunction(() => document.querySelector('[data-field="source-installed-version"]').textContent === "2.1.0-rc.1");
    assert.equal(await frame.locator("#source-channel-choice").isVisible(), false);
    assert.deepEqual(server.requestFailures, []);
  } finally {
    await browser.close();
    await server.close();
  }
});
