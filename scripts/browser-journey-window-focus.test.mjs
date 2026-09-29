import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import test from "node:test";
import vm from "node:vm";

const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");
const main = readFileSync(new URL("./home-passkey-virtual-auth-smoke.mjs", import.meta.url), "utf8");
const windows = readFileSync(new URL("../capsules/home-gui/browser/shell-windows.js", import.meta.url), "utf8");
const surface = readFileSync(new URL("../capsules/home-gui/browser/shell-surface.js", import.meta.url), "utf8");
const guiSource = readFileSync(new URL("../capsules/home-gui/browser/home-gui.js", import.meta.url), "utf8");
function declaration(source, name) {
  const start = source.search(new RegExp(`(?:async )?function ${name}\\(`));
  assert.ok(start >= 0, name);
  const next = source.slice(start + 1).search(/\n(?:export )?(?:async )?function /);
  return source.slice(start, start + 1 + next);
}
const menuListener = guiSource.match(/  desktopContextMenu\?\.addEventListener\("click", \(event\) => \{[\s\S]*?\n  \}\);/)[0];
// Use Home's actual menu, dispatch and focus functions. Only unrelated shell
// services and fixture creation/close are substitutes; pointer hit testing is real.
const program = `
const shellState = { windows: new Map(), zIndexCounter: 10, currentSummary: {}, contextMenuTarget: { kind: "target", source: "taskbar", targetId: "browser" } };
const desktopContextMenu = document.querySelector("#desktop-context-menu");
const evidence = window.evidence = { actions: [], closes: [], contextClicks: [] };
const rememberRecentTarget = () => {}, fitLaunchedWindow = () => {}, refreshWindowUi = () => {}, persistBrowserSession = () => {};
const supportsMenuNewWindow = () => true, browserWindowDisplayTitle = entry => entry.id;
const taskbarPinMenuItem = () => ({ action: "pin-taskbar", label: "Keep in Shelf" }), appendTargetGroupManagementItems = () => {};
const contextMenuItems = target => targetContextMenuItems(target);
const hideDesktopContextMenu = () => { desktopContextMenu.hidden = true; };
${["browserWindowEntries", "browserWindowEntriesForTarget", "sortWindowEntriesByZOrder", "focusWindow", "focusWindowControl"].map(name => declaration(windows, name)).join("\n")}
${["targetContextMenuItems", "renderContextMenu", "handleContextAction"].map(name => declaration(surface, name)).join("\n")}
${menuListener}
desktopContextMenu.addEventListener("click", event => evidence.actions.push({ action: event.target.dataset.contextAction, trusted: event.isTrusted }));
function addWindow(id, z, active = false) {
  const node = document.createElement("section");
  node.className = "window" + (active ? " window-active" : "");
  node.dataset.target = "browser"; node.dataset.windowId = id; node.style.zIndex = String(z);
  node.innerHTML = '<button aria-label="Close" data-action="close">Close</button><iframe class="window-frame" sandbox="allow-scripts" src="/apps/browser/?browser_instance=' + id + '#home_token=token-' + id + '"></iframe>';
  document.querySelector("#windows").append(node);
  shellState.windows.set(id, { id, node, kind: "browser", targetId: "browser" });
  node.querySelector("button").addEventListener("click", event => {
    evidence.closes.push({ id, trusted: event.isTrusted }); shellState.windows.delete(id); node.remove();
  });
}
window.restoreC = () => { if (!shellState.windows.has("browser--3")) addWindow("browser--3", 20); focusWindow("browser--3"); };
document.querySelector("#shelf").addEventListener("contextmenu", event => {
  event.preventDefault(); evidence.contextClicks.push(event.isTrusted);
  renderContextMenu(shellState.contextMenuTarget); desktopContextMenu.hidden = false;
});
document.querySelector("#shelf").addEventListener("click", () => {
  if (shellState.windows.size) throw new Error("Generic Shelf must not toggle an existing Browser");
  addWindow("browser--2", 2, true);
});
if (!location.search.includes("empty=1")) { addWindow("browser--2", 2, true); addWindow("browser--1", 1); }
`;
const shell = `<style>
body { margin: 0; } .window { position:absolute; left:20px; top:20px; width:400px; height:300px; background:#ddd; }
.window > button { position:absolute; right:0; top:0; width:70px; height:35px; }
iframe.window-frame { position:absolute; top:40px; width:390px; height:250px; border:0; }
#taskbar-targets { position:fixed; bottom:5px; left:10px; z-index:100; }
#desktop-context-menu { position:fixed; bottom:40px; left:10px; z-index:200; background:white; }
#desktop-context-menu button { display:block; height:30px; } [hidden] { display:none; }
</style><div id="windows"></div><div id="taskbar-targets"><button id="shelf" data-target="browser">Browser</button></div>
<div id="desktop-context-menu" hidden></div><script>${program}</script>`;

test("real Home menu focuses captured Browser through overlap; launcher binds foreground rather than DOM-last", async () => {
  const server = createServer((req, res) => {
    res.setHeader("content-type", "text/html");
    if (req.url.startsWith("/apps/home-gui/")) res.end(shell);
    else if (req.url.startsWith("/apps/browser/")) res.end("<!doctype html><title>Fixture Browser</title>");
    else res.end('<iframe style="width:800px;height:600px" src="/apps/home-gui/' + (req.url.includes("empty=1") ? "?empty=1" : "") + '#home_token=gui-token"></iframe>');
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  let browser;
  try {
    browser = await chromium.launch({ headless: true,
      ...(process.env.ELASTOS_BROWSER_EXECUTABLE ? { executablePath: process.env.ELASTOS_BROWSER_EXECUTABLE } : {}),
      args: ["--disable-background-networking", "--disable-component-update", "--no-first-run"] });
    const page = await browser.newPage({ viewport: { width: 900, height: 700 } });
    const errors = []; page.on("pageerror", error => errors.push(error.message));
    const globals = { URL, URLSearchParams, Date, HOME_URL: `${origin}/apps/home/`,
      delay: ms => new Promise(resolve => setTimeout(resolve, ms)), assert: (value, message) => assert.ok(value, message) };
    for (const name of ["launchTokenFromRoute", "assertIsolatedLaunchRoute", "assertBrowserWindowIdentity", "captureBrowserWindowIdentity",
      "focusCapturedBrowserWindow", "clickBrowserWindowClose", "waitForBrowserWindowDetached"]) {
      globals[name] = vm.runInNewContext(`(${declaration(main, name)})`, globals);
    }
    for (const empty of [false, true]) {
      await page.goto(`${origin}/apps/home/${empty ? "?empty=1" : ""}`);
      const gui = page.frames().find(frame => frame.url().includes("/apps/home-gui/"));
      await gui.locator("#shelf").waitFor();
      const open = vm.runInNewContext(`(${declaration(main, "openDesktopAppWindow")})`, {
        ...globals, waitForSignedHome: async () => {}, waitForCapsuleFrame: async () => gui,
      });
      let picked;
      const frame = await open(page, "browser", async selected => {
        picked = selected;
        // Restore takes foreground after capture, before later readiness waits.
        await gui.evaluate(() => window.restoreC());
      });
      assert.equal(frame, picked);
      assert.equal(new URL(frame.url()).searchParams.get("browser_instance"), "browser--2");
      const identity = await globals.captureBrowserWindowIdentity(frame);
      assert.equal(identity.windowId, "browser--2");
      assert.equal(identity.token, "token-browser--2");
      await gui.evaluate(() => window.restoreC());
      const button = await identity.section.$('[data-action="close"]');
      await assert.rejects(button.click({ trial: true, timeout: 250 }), /intercepts pointer events/);
      await assert.rejects(globals.clickBrowserWindowClose(identity, frame, "wrong-token"), /authority identity/);
      assert.deepEqual(await gui.evaluate(() => evidence.closes), []);
      await globals.clickBrowserWindowClose(identity, frame, identity.token);
      await globals.waitForBrowserWindowDetached(identity);
      assert.equal(await gui.locator('[data-window-id="browser--3"]').count(), 1);
      assert.equal(await gui.locator('[data-window-id="browser--1"]').count(), empty ? 0 : 1);
      const evidence = await gui.evaluate(() => window.evidence);
      assert.deepEqual(evidence.closes, [{ id: "browser--2", trusted: true }]);
      assert.equal(evidence.actions.length, 2);
      assert.ok(evidence.actions.every(row => row.action === "focus-window:browser--2" && row.trusted));
      assert.deepEqual(evidence.contextClicks, [true, true]);
      await button.dispose();
    }
    assert.deepEqual(errors, []);
  } finally {
    await browser?.close(); server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
  }
});
