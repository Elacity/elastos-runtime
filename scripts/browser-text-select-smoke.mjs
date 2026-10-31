#!/usr/bin/env node
// Real-Chromium journey for Browser text selection over the Selkies
// datachannel: a mouse drag must reach the guest as held-button moves (not a
// click), and Cmd+A then Cmd+C must select all and start the copy intent.

import { readFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "./system-uiux-fixture.mjs";

const repoRoot = resolve(fileURLToPath(new URL("../", import.meta.url)));
const browserRoot = join(repoRoot, "capsules/browser/browser");
const origin = "http://browser-text-select.test";

function assert(condition, message, details = undefined) {
  if (!condition) {
    throw new Error(`${message}${details ? `\n${JSON.stringify(details, null, 2)}` : ""}`);
  }
}

// Mirrors dispatchBrowserInput: clicks keep the Runtime route; everything else
// is encoded by the shipped Selkies encoder onto a fake datachannel.
const fixtureHtml = `<!doctype html>
<style>body{margin:0} #panel{width:640px;height:360px} video{width:640px;height:360px;object-fit:fill;display:block}</style>
<div id="panel" tabindex="0"><video id="video"></video></div>
<script type="module">
import { bindBrowserInputSurface } from "./browser-input-surface.js";
import { selkiesMessagesForInput } from "./browser-input.js";
import { createBrowserClipboardBridge } from "./browser-clipboard.js";
window.channel = [];
window.runtime = [];
const page = { page_id: "page:select" };
const view = { width: 1280, height: 720 };
async function sendBrowserInput(event) {
  if (event.type === "click") { window.runtime.push(event); return { accepted: true }; }
  window.channel.push(...selkiesMessagesForInput(event, view));
}
window.hostWrites = [];
window.statuses = [];
const clipboard = createBrowserClipboardBridge({
  friendlyOpenError: String, getCurrentPage: () => page, sendBrowserInput,
  showStatus: (text) => window.statuses.push(text),
  // Stands in for Home's clipboard prompt; resolves as if the user approved.
  writeHostClipboardTextFn: async (text, options) => { window.hostWrites.push({ text, ...options }); },
  createClipboardRequestIdFn: () => "req",
});
// The guest's upstream Selkies reply to "cr" (gstwebrtc_app.send_clipboard_data).
window.deliverGuestMessage = (message) => clipboard.handleRemoteInputChannelMessage({ data: JSON.stringify(message) });
bindBrowserInputSurface({
  copyRemoteClipboardToHost: clipboard.copyRemoteClipboardToHost, friendlyOpenError: String,
  getCurrentPage: () => page, getCurrentView: () => view, keyboardCapture: null,
  getInputOwner: () => page, sameInputOwner: (left, right) => left === right,
  pasteHostClipboardIntoRemote: async () => {}, readHostClipboardText: async () => "",
  remoteVideo: document.getElementById("video"), renderPanel: document.getElementById("panel"),
  releaseDragButton: (event) => window.channel.push(...selkiesMessagesForInput(event, view)),
  sendBrowserInput, showStatus: () => {}, streamsPointerDrag: () => true, unlockRemoteAudioFromGesture: () => {},
});
window.ready = true;
</script>`;

const browser = await chromium.launch({ headless: true, ...(process.env.BRAVE_BIN ? { executablePath: process.env.BRAVE_BIN } : {}) });
try {
  const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  await page.route(`${origin}/**`, async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/") {
      await route.fulfill({ contentType: "text/html", body: fixtureHtml });
      return;
    }
    const file = join(browserRoot, path.slice(1));
    assert(file.startsWith(`${browserRoot}/`), "invalid fixture path", { path });
    await route.fulfill({ contentType: "text/javascript", body: await readFile(file) });
  });
  await page.goto(`${origin}/`);
  await page.waitForFunction(() => window.ready === true);

  // Drag from (50,50) to (300,100) CSS px: 2x scale onto the 1280x720 guest.
  await page.mouse.move(50, 50);
  await page.mouse.down();
  await page.mouse.move(52, 50);
  await page.mouse.move(150, 70, { steps: 5 });
  await page.mouse.move(300, 100, { steps: 5 });
  await page.mouse.up();
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
  const drag = await page.evaluate(() => ({ channel: window.channel.splice(0), runtime: window.runtime.splice(0) }));
  assert(drag.runtime.length === 0, "a drag must not also send a click", drag);
  assert(drag.channel[0] === "m,100,100,1,0", "drag must press at the start point", drag);
  assert(drag.channel.at(-1) === "m,600,200,0,0", "drag must release at the final point", drag);
  const held = drag.channel.slice(1, -1);
  assert(held.length >= 1 && held.every((message) => /^m,\d+,\d+,1,0$/.test(message)), "drag moves must hold the primary button", drag);
  assert(held.length <= 10, "drag moves must be frame-throttled", drag);

  // A plain click stays on the Runtime route.
  await page.mouse.click(100, 100);
  const click = await page.evaluate(() => ({ channel: window.channel.splice(0), runtime: window.runtime.splice(0) }));
  assert(click.channel.length === 0 && click.runtime.length === 1 && click.runtime[0].x === 200, "a still click keeps the Runtime click", click);

  // A drag that leaves the video extends to its edge rather than stopping.
  await page.mouse.move(600, 300);
  await page.mouse.down();
  await page.mouse.move(610, 310);
  await page.mouse.move(700, 500, { steps: 3 });
  await page.mouse.up();
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
  const edge = await page.evaluate(() => ({ channel: window.channel.splice(0), runtime: window.runtime.splice(0) }));
  assert(edge.runtime.length === 0 && edge.channel.at(-1) === "m,1280,720,0,0", "a drag past the video must release at its edge", edge);

  await page.focus("#panel");
  await page.keyboard.press("Meta+A");
  await page.keyboard.press("Meta+C");
  await page.waitForFunction(() => window.channel.includes("cr"), null, { timeout: 2000 });
  const keys = await page.evaluate(() => window.channel.splice(0));
  assert(
    JSON.stringify(keys) === JSON.stringify(["kd,65507", "kd,97", "ku,97", "ku,65507", "kd,65507", "kd,99", "ku,99", "ku,65507", "cr"]),
    "Cmd+A then Cmd+C must select all, copy, then read the guest clipboard",
    keys,
  );
  // A second Cmd+C while the copy is pending is quiet and sends nothing new.
  await page.keyboard.press("Meta+C");
  const content = Buffer.from("selected guest text ✓", "utf8").toString("base64");
  await page.evaluate((value) => window.deliverGuestMessage({ type: "clipboard", data: { content: value } }), content);
  await page.waitForFunction(() => window.hostWrites.length > 0, null, { timeout: 2000 });
  const copied = await page.evaluate(() => ({ hostWrites: window.hostWrites, statuses: window.statuses, channel: window.channel.splice(0) }));
  assert(
    JSON.stringify(copied.hostWrites) === JSON.stringify([{ text: "selected guest text ✓", requestId: "req" }]) &&
      JSON.stringify(copied.statuses) === JSON.stringify(["Copied from Browser."]) &&
      copied.channel.length === 0,
    "the guest's upstream clipboard reply must reach the Home clipboard write once, with no pending-copy error",
    copied,
  );
  assert(pageErrors.length === 0, "fixture page errors", pageErrors);
  console.log(JSON.stringify({ drag: drag.channel, click: click.runtime, keys }, null, 2));
} finally {
  await browser.close();
}
