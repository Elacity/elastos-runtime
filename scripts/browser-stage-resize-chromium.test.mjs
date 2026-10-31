import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import test from "node:test";
const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || "playwright");
const source = await readFile(new URL("../capsules/browser/browser/browser-resize.js", import.meta.url), "utf8");

test("real ResizeObserver uses untransformed content-box CSS pixels at DPR 1, 2 and 3", async () => {
  const browser = await chromium.launch({ headless: true,
    executablePath: process.env.BRAVE_BIN || "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser" });
  try {
    for (const deviceScaleFactor of [1, 2, 3]) {
      const context = await browser.newContext({ viewport: { width: 1280, height: 1400 }, deviceScaleFactor });
      const page = await context.newPage();
      await page.route("http://resize.test/**", route => route.fulfill({ contentType: "text/html", body:
        '<div id="stage" style="width:1000px;height:700px;transform:scale(.5)"></div>' }));
      await page.goto("http://resize.test/");
      await page.evaluate(async moduleSource => {
        const { observeBrowserStage } = await import(`data:text/javascript;base64,${btoa(moduleSource)}`);
        window.sent = [];
        window.resize = observeBrowserStage({ stage: document.querySelector("#stage"), getOwner: () => "same-page",
          send: async event => { window.sent.push(event); return { accepted: true }; }, onError: error => { throw error; } });
        window.resize.opened();
      }, source);
      await page.waitForFunction(() => sent.length === 1);
      const initial = await page.evaluate(() => sent[0]);
      assert.equal(initial.width, 1000); assert.equal(initial.height, 700);
      await page.evaluate(() => {
        const stage = document.querySelector("#stage");
        stage.style.width = "600px"; stage.style.height = "600px";
        stage.style.width = "390px"; stage.style.height = "1200px";
      });
      await page.waitForFunction(() => sent.length === 2);
      const portrait = await page.evaluate(() => sent[1]);
      assert.equal(portrait.width, 390); assert.equal(portrait.height, 1200);
      assert.ok(portrait.sequence > initial.sequence);
      await page.evaluate(() => { document.querySelector("#stage").hidden = true; });
      await page.waitForTimeout(220);
      assert.equal(await page.evaluate(() => sent.length), 2);
      await page.evaluate(() => {
        const stage = document.querySelector("#stage"); stage.hidden = false;
        stage.style.width = "845px"; stage.style.height = "339px";
      });
      await page.waitForFunction(() => sent.length === 3);
      assert.deepEqual(await page.evaluate(() => [sent[2].width, sent[2].height]), [846, 340]);
      await context.close();
    }
  } finally { await browser.close(); }
});
