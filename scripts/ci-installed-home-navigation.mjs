import assert from "node:assert/strict";
import { join } from "node:path";

export async function openInstalledApp(page, target, timeout = 30000) {
  const shell = page.frameLocator("#active-shell-frame");
  const [control, surface] = {
    marketplace: ['.taskbar-item[data-target="marketplace"]', 'section.window[data-target="marketplace"] iframe.window-frame'],
    assistant: ["#assistant-toggle", "#assistant-space-frame"],
  }[target];
  await shell.locator('body[data-home-status="ready"][data-home-authority="signed"]').waitFor({ state: "visible", timeout });
  const iframe = shell.locator(surface).last();
  // The dock hides an active window and brings an inactive window forward.
  const activeMarketplace = shell.locator('section.window.window-active[data-target="marketplace"] iframe.window-frame').last();
  if (!await iframe.isVisible() || (target === "marketplace" && !await activeMarketplace.isVisible())) {
    await shell.locator(control).click({ timeout });
  }
  await iframe.waitFor({ state: "visible", timeout });
  const frame = await (await iframe.elementHandle()).contentFrame();
  await frame.waitForURL(url => url.pathname.startsWith(`/apps/${target}/`), { timeout });
  return frame;
}

export async function openInstalledAssistant(page, homeUrl, timeout = 60000) {
  assert((await page.goto(homeUrl, { waitUntil: "domcontentloaded", timeout })).ok(), "installed Home frontdoor responds successfully");
  await page.waitForFunction(() =>
    (document.body.dataset.homeStatus === "ready" && document.body.dataset.homeAuthority === "signed") ||
    document.querySelector("#home-unlock")?.hidden === false,
  null, { timeout });
  assert.equal(await page.locator("#home-unlock").isVisible(), false, "second Home requires a refreshed signed session");
  await page.waitForFunction(() =>
    document.body.dataset.homeStatus === "ready" && document.body.dataset.homeAuthority === "signed" &&
    document.querySelector("#home-unlock")?.hidden !== false,
  null, { timeout });
  const shell = page.frameLocator("#active-shell-frame");
  // Shell readiness includes saved-window restoration; a restore still running keeps an opened Assistant.
  await shell.locator('body[data-home-status="ready"][data-home-authority="signed"]').waitFor({ state: "visible", timeout });
  await shell.locator("#desktop").waitFor({ state: "visible", timeout });
  if (await shell.locator("#setup-sheet").isVisible()) {
    await shell.locator("#setup-sheet-close").click({ timeout });
    await shell.locator("#setup-sheet").waitFor({ state: "hidden", timeout });
  }
  await shell.locator("#assistant-toggle").click({ timeout });
  const iframe = shell.locator("#assistant-space-frame");
  await iframe.waitFor({ state: "visible", timeout });
  const assistant = await (await iframe.elementHandle()).contentFrame();
  await assistant.waitForURL(url => url.pathname.startsWith("/apps/assistant/"), { timeout });
  await assistant.locator('#agent-harness.is-visible[aria-hidden="false"]').waitFor({ state: "visible", timeout });
  return assistant;
}

export async function returnInstalledHome(page, assistant, evidence, timeout = 10000) {
  // Assistant owns the visible Home control while Home's dock is hidden.
  await assistant.locator("#agent-shelf-flip-back").click({ timeout });
  const shell = page.frameLocator("#active-shell-frame");
  await shell.locator("#assistant-space").waitFor({ state: "hidden", timeout });
  await shell.locator("#desktop").waitFor({ state: "visible", timeout });
  assert.equal(await page.getAttribute("body", "data-home-status"), "ready");
  await page.screenshot({ path: join(evidence, "finite-home-recovery.png"), fullPage: true });
}
