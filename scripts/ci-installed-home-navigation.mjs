import assert from "node:assert/strict";
import { join } from "node:path";

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
  // Shell readiness includes saved-window restoration, which can close an opening Assistant.
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
