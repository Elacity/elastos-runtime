#!/usr/bin/env node

import {
  assert,
  brave,
  chromium,
  CORS_HEADERS,
  inertSystemApiResponse,
  jsonResponse,
  makeAppearanceRecord,
  makeSystemSummary,
  startSystemFixtureServer,
  textResponse,
} from "./system-uiux-fixture.mjs";

function deferred() {
  let resolvePromise;
  let rejectPromise;
  const promise = new Promise((resolve, reject) => {
    resolvePromise = resolve;
    rejectPromise = reject;
  });
  return {
    promise,
    resolve: resolvePromise,
    reject: rejectPromise,
  };
}

function appearanceRecord(overrides = {}) {
  return makeAppearanceRecord(overrides);
}

function systemSummary(appearance) {
  return makeSystemSummary(appearance);
}

let summaryAppearance = appearanceRecord();

const BEHAVIOR_HOST_SCRIPT = `
      (() => {
        const trace = {
          appReadyMessages: [],
          refreshMessages: [],
          clipboardRequests: [],
        };
        let clipboardMode = "auto-success";
        let pendingClipboard = null;
        const generation = "clipboard-generation-1";

        function clipboardReady(targetWindow) {
          targetWindow.postMessage({
            type: "home:clipboard-ready",
            schema: "elastos.home.clipboard.ready/v1",
            targetId: "system",
            homeToken: "system-token",
            parentOrigin: window.location.origin,
            generation,
          }, "*");
        }

        function replyClipboard(ok, error = "denied") {
          if (!pendingClipboard) {
            return false;
          }
          const { source, data } = pendingClipboard;
          pendingClipboard = null;
          source.postMessage(
            ok
              ? {
                  type: "home:clipboard-result",
                  schema: "elastos.home.clipboard.result/v1",
                  requestId: data.requestId,
                  targetId: "system",
                  homeToken: "system-token",
                  parentOrigin: window.location.origin,
                  generation,
                  operation: data.operation,
                  purpose: data.purpose,
                  ok: true,
                }
              : {
                  type: "home:clipboard-result",
                  schema: "elastos.home.clipboard.result/v1",
                  requestId: data.requestId,
                  targetId: "system",
                  homeToken: "system-token",
                  parentOrigin: window.location.origin,
                  generation,
                  operation: data.operation,
                  purpose: data.purpose,
                  ok: false,
                  error,
                },
            "*",
          );
          return true;
        }

        window.__systemHost = {
          trace,
          setClipboardMode(mode) {
            clipboardMode = mode;
          },
          replyClipboardSuccess() {
            return replyClipboard(true);
          },
          replyClipboardFailure(error = "denied") {
            return replyClipboard(false, error);
          },
        };

        window.addEventListener("message", (event) => {
          const frame = document.getElementById("system-frame");
          if (!frame || event.source !== frame.contentWindow) {
            return;
          }
          const data = event.data;
          if (!data || typeof data !== "object" || Array.isArray(data)) {
            return;
          }
          if (data.type === "home:app-ready" && data.homeToken === "system-token") {
            trace.appReadyMessages.push({ origin: event.origin, type: data.type });
            clipboardReady(event.source);
            return;
          }
          if (data.type === "home:refresh-summary" && data.homeToken === "system-token") {
            trace.refreshMessages.push({ origin: event.origin, payload: data });
            return;
          }
          if (data.type === "home:clipboard-request") {
            trace.clipboardRequests.push(data);
            pendingClipboard = { source: event.source, data };
            if (clipboardMode === "auto-success") {
              replyClipboard(true);
            } else if (clipboardMode === "auto-fail") {
              replyClipboard(false, "denied");
            }
          }
        });
      })();
`;

async function startServer() {
  return startSystemFixtureServer({
    title: "System UIUX behavior fixture",
    background: "#101216",
    hostScript: BEHAVIOR_HOST_SCRIPT,
    async onApiRequest({ response, url }) {
      if (url.pathname === "/api/apps/system/summary") {
        jsonResponse(response, systemSummary(summaryAppearance));
        return true;
      }
      return false;
    },
  });
}

async function waitForSystemFrame(page) {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    const frame = page.frames().find((candidate) =>
      candidate.url().includes("/apps/system/"),
    );
    if (frame) {
      await frame.waitForSelector("#settings-search");
      return frame;
    }
    await page.waitForTimeout(50);
  }
  throw new Error("System frame did not load");
}

async function activateTab(frame, tab) {
  await frame.click(`.settings-sidebar-item[data-settings="${tab}"]`);
  await frame.waitForFunction(
    (nextTab) =>
      document
        .querySelector(`.settings-content[data-settings="${nextTab}"]`)
        ?.classList.contains("active") === true,
    tab,
  );
}

async function waitForAppearance(frame, matcher) {
  await frame.waitForFunction(matcher);
}

async function readAppearanceState(frame) {
  return frame.evaluate(() => ({
    errorHidden: document.querySelector(".system-error")?.hidden ?? null,
    errorText: document.querySelector(".system-error")?.textContent?.trim() ?? "",
    theme: document.querySelector('#theme-segment [data-theme-option].active')?.dataset.themeOption ?? "",
    accent: document.querySelector('#accent-picker [data-accent-option].active')?.dataset.accentOption ?? "",
    accentCustom: document.querySelector("#accent-custom-hex")?.value ?? "",
    dockAutoHide: document.querySelector("#dock-autohide")?.checked ?? null,
    sounds: document.querySelector("#ui-sounds")?.checked ?? null,
    copied: document.querySelector("#device-did-copy")?.dataset.copied ?? "",
    copyTitle: document.querySelector("#device-did-copy")?.title ?? "",
    copyLabel: document.querySelector("#device-did-copy")?.getAttribute("aria-label") ?? "",
    copyCheckHidden:
      document.querySelector("#device-did-copy .el-copy-check")?.hasAttribute("hidden") ?? null,
    copyIconHidden:
      document.querySelector("#device-did-copy .el-copy-icon")?.hasAttribute("hidden") ?? null,
  }));
}

async function collectStartupDiagnostics(frame, page, summaryTrace, pageErrors, consoleErrors, requestFailures) {
  const deadline = Date.now() + 3_000;
  let snapshot = null;
  while (Date.now() < deadline) {
    snapshot = await frame.evaluate(() => ({
      activeTab:
        document.querySelector(".settings-sidebar-item.active")?.dataset.settings ??
        "",
      deviceDidText:
        document.querySelector("#device-did-value")?.textContent?.trim() ?? "",
      errorText:
        document.querySelector(".system-error")?.hidden === false
          ? document.querySelector(".system-error")?.textContent?.trim() ?? ""
          : "",
      frameUrl: window.location.href,
      readyState: document.readyState,
      theme:
        document.querySelector('#theme-segment [data-theme-option].active')
          ?.dataset.themeOption ?? "",
      accent:
        document.querySelector('#accent-picker [data-accent-option].active')
          ?.dataset.accentOption ?? "",
      accentCustom:
        document.querySelector("#accent-custom-hex")?.value ?? "",
      dockAutoHide: document.querySelector("#dock-autohide")?.checked ?? null,
      sounds: document.querySelector("#ui-sounds")?.checked ?? null,
    }));
    if (
      summaryTrace.length > 0 ||
      snapshot.deviceDidText.length > 0 ||
      snapshot.errorText.length > 0
    ) {
      break;
    }
    await page.waitForTimeout(100);
  }
  return {
    summaryRequests: summaryTrace,
    pageErrors,
    consoleErrors,
    requestFailures,
    ...snapshot,
  };
}

function nextAppearancePlan(queue, plan) {
  queue.push(plan);
}

async function main() {
  const server = await startServer();
  const hostOrigin = server.baseUrl;
  const pageErrors = [];
  const requestFailures = [];
  const appearanceRequests = [];
  const appearancePlans = [];
  summaryAppearance = appearanceRecord();
  let appearanceInFlight = 0;
  let maxAppearanceInFlight = 0;
  const summaryTrace = [];
  let browser;
  try {
    browser = await chromium.launch({
      headless: true,
      executablePath: brave,
    });
    const page = await browser.newPage({
      viewport: { width: 1280, height: 900 },
    });
    page.setDefaultNavigationTimeout(5_000);
    page.setDefaultTimeout(5_000);
    const consoleErrors = [];
    page.on("pageerror", (error) => {
      pageErrors.push(error.message);
    });
    page.on("console", (message) => {
      if (message.type() === "error") {
        consoleErrors.push(message.text());
      }
    });
    page.on("requestfailed", (request) => {
      requestFailures.push(
        `${request.url()} ${request.failure()?.errorText ?? "request failed"}`,
      );
    });
    await page.route("**/api/**", async (route) => {
      const request = route.request();
      if (request.method() === "OPTIONS") {
        await route.fulfill({
          status: 204,
          headers: CORS_HEADERS,
          body: "",
        });
        return;
      }
      const url = new URL(request.url());
      if (url.pathname === "/api/apps/system/summary") {
        summaryTrace.push({
          count: summaryTrace.length + 1,
          status: 200,
          body: systemSummary(summaryAppearance),
        });
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          headers: CORS_HEADERS,
          body: JSON.stringify(systemSummary(summaryAppearance)),
        });
        return;
      }
      if (url.pathname === "/api/apps/system/appearance/preferences") {
        const body = request.postDataJSON();
        appearanceRequests.push(body);
        const plan = appearancePlans.shift();
        assert(plan, "Unexpected System appearance write", { body });
        assert(
          JSON.stringify(body) === JSON.stringify(plan.expectBody),
          "System appearance write did not use the expected exact one-field body",
          { actual: body, expected: plan.expectBody },
        );
        plan.seen?.resolve(body);
        appearanceInFlight += 1;
        maxAppearanceInFlight = Math.max(maxAppearanceInFlight, appearanceInFlight);
        try {
          if (plan.hold) {
            await plan.hold.promise;
          }
          if (plan.errorText) {
            await route.fulfill({
              status: plan.status || 500,
              contentType: "text/plain; charset=utf-8",
              headers: CORS_HEADERS,
              body: plan.errorText,
            });
            return;
          }
          summaryAppearance = plan.response;
          await route.fulfill({
            status: 200,
            contentType: "application/json",
            headers: CORS_HEADERS,
            body: JSON.stringify(plan.response),
          });
        } finally {
          appearanceInFlight -= 1;
        }
        return;
      }
      const inert = inertSystemApiResponse(url.pathname);
      if (inert !== null) {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          headers: CORS_HEADERS,
          body: JSON.stringify(inert),
        });
        return;
      }
      await route.fulfill({
        status: 500,
        contentType: "text/plain; charset=utf-8",
        headers: CORS_HEADERS,
        body: `Unhandled fixture API route: ${url.pathname}`,
      });
    });

    await page.goto(`${server.baseUrl}/fixture`, { waitUntil: "networkidle" });
    const frame = await waitForSystemFrame(page);
    const startup = await collectStartupDiagnostics(
      frame,
      page,
      summaryTrace,
      pageErrors,
      consoleErrors,
      requestFailures,
    );
    assert(
      startup.summaryRequests.length > 0 &&
        startup.summaryRequests[0].status === 200 &&
        startup.readyState === "complete" &&
        startup.deviceDidText.length > 0 &&
        startup.errorText.length === 0,
      "System startup diagnostic failed",
      startup,
    );
    await activateTab(frame, "personalization");

    const initialState = await readAppearanceState(frame);
    assert(
      initialState.theme === "dark" &&
        initialState.accent === "orange" &&
        initialState.accentCustom === "#4f7fff" &&
        initialState.dockAutoHide === true &&
        initialState.sounds === false,
      "System did not load authoritative appearance values from summary",
      initialState,
    );

    const holdTheme = deferred();
    const seenTheme = deferred();
    nextAppearancePlan(appearancePlans, {
      expectBody: { theme: "light" },
      response: appearanceRecord({ revision: 6, theme: "light" }),
      hold: holdTheme,
      seen: seenTheme,
    });
    const refreshCountBeforeTheme = await page.evaluate(
      () => window.__systemHost.trace.refreshMessages.length,
    );
    await frame.click('#theme-segment [data-theme-option="light"]');
    await seenTheme.promise;
    const pendingThemeState = {
      ...(await frame.evaluate(() => ({
        themeDisabled: document.querySelector('#theme-segment [data-theme-option="dark"]')?.disabled ?? null,
        dockDisabled: document.querySelector("#dock-autohide")?.disabled ?? null,
        soundsDisabled: document.querySelector("#ui-sounds")?.disabled ?? null,
      }))),
      requestCount: await page.evaluate(
        () => window.__systemHost.trace.refreshMessages.length,
      ),
    };
    assert(
      pendingThemeState.themeDisabled === true &&
        pendingThemeState.dockDisabled === true &&
        pendingThemeState.soundsDisabled === true &&
        appearanceRequests.length === 1,
      "System did not serialize appearance writes behind one busy boundary",
      pendingThemeState,
    );
    holdTheme.resolve();
    await waitForAppearance(
      frame,
      () =>
        document.querySelector('#theme-segment [data-theme-option].active')
          ?.dataset.themeOption === "light",
    );
    assert(
      (await page.evaluate(() => window.__systemHost.trace.refreshMessages.length)) ===
        refreshCountBeforeTheme + 1,
      "Accepted System theme write did not emit verified home:refresh-summary",
    );

    nextAppearancePlan(appearancePlans, {
      expectBody: { dock_auto_hide: false },
      response: appearanceRecord({ revision: 7, theme: "light", dock_auto_hide: false }),
    });
    await frame.click('label:has(#dock-autohide)');
    await waitForAppearance(
      frame,
      () => document.querySelector("#dock-autohide")?.checked === false,
    );

    nextAppearancePlan(appearancePlans, {
      expectBody: { accent: "custom" },
      response: appearanceRecord({
        revision: 8,
        theme: "light",
        accent: "custom",
        dock_auto_hide: false,
      }),
    });
    await frame.click('#accent-picker [data-accent-option="custom"]');
    await waitForAppearance(
      frame,
      () =>
        document.querySelector('#accent-picker [data-accent-option].active')
          ?.dataset.accentOption === "custom" &&
        document.querySelector("#accent-custom-popover")?.hidden === false,
    );

    nextAppearancePlan(appearancePlans, {
      expectBody: { accent_custom: "#336699" },
      response: appearanceRecord({
        revision: 9,
        theme: "light",
        accent: "custom",
        accent_custom: "#336699",
        dock_auto_hide: false,
      }),
    });
    await frame.fill("#accent-custom-hex", "#336699");
    await frame.press("#accent-custom-hex", "Enter");
    await waitForAppearance(
      frame,
      () => document.querySelector("#accent-custom-hex")?.value === "#336699",
    );

    const refreshCountBeforeStale = await page.evaluate(
      () => window.__systemHost.trace.refreshMessages.length,
    );
    nextAppearancePlan(appearancePlans, {
      expectBody: { sounds: true },
      response: appearanceRecord({
        revision: 8,
        theme: "dark",
        accent: "orange",
        accent_custom: "#ff0000",
        dock_auto_hide: true,
        sounds: true,
      }),
    });
    await frame.click('label:has(#ui-sounds)');
    await waitForAppearance(
      frame,
      () =>
        document.querySelector("#ui-sounds")?.checked === false &&
        document.querySelector('#theme-segment [data-theme-option].active')
          ?.dataset.themeOption === "light" &&
        document.querySelector('#accent-picker [data-accent-option].active')
          ?.dataset.accentOption === "custom" &&
        document.querySelector("#accent-custom-hex")?.value === "#336699" &&
        document.querySelector("#dock-autohide")?.checked === false,
    );
    const staleState = await readAppearanceState(frame);
    assert(
      staleState.theme === "light" &&
        staleState.accent === "custom" &&
        staleState.accentCustom === "#336699" &&
        staleState.dockAutoHide === false &&
        staleState.sounds === false &&
        (await page.evaluate(() => window.__systemHost.trace.refreshMessages.length)) ===
          refreshCountBeforeStale,
      "Lower revision appearance response changed accepted System appearance state",
      staleState,
    );

    nextAppearancePlan(appearancePlans, {
      expectBody: { dock_auto_hide: true },
      errorText: "appearance request failed",
      status: 500,
    });
    await frame.click('label:has(#dock-autohide)');
    await waitForAppearance(
      frame,
      () =>
        document.querySelector("#dock-autohide")?.checked === false &&
        document.querySelector("#ui-sounds")?.checked === false &&
        !document.querySelector(".system-error")?.hidden,
    );
    const failedWriteState = await readAppearanceState(frame);
    assert(
      failedWriteState.dockAutoHide === false &&
        failedWriteState.sounds === false &&
        failedWriteState.errorHidden === false &&
        failedWriteState.errorText.length > 0,
      "Failed System appearance write did not restore authoritative controls",
      failedWriteState,
    );

    await page.evaluate(() => window.__systemHost.setClipboardMode("hold"));
    await activateTab(frame, "about");
    const copyBefore = await readAppearanceState(frame);
    assert(copyBefore.copied === "false", "System DID copy button started in copied state", copyBefore);
    await frame.click("#device-did-copy");
    await page.waitForFunction(
      () => window.__systemHost.trace.clipboardRequests.length === 1,
    );
    const copyPending = await readAppearanceState(frame);
    assert(
      copyPending.copied === "false" &&
        copyPending.copyCheckHidden === true &&
        copyPending.copyIconHidden === false,
      "System DID copy showed success before the trusted Home Clipboard host replied",
      copyPending,
    );
    const clipboardRequest = await page.evaluate(
      () => window.__systemHost.trace.clipboardRequests[0],
    );
    assert(
      clipboardRequest.homeToken === "system-token" &&
        clipboardRequest.parentOrigin === hostOrigin &&
        clipboardRequest.generation === "clipboard-generation-1" &&
        clipboardRequest.purpose === "identity.did" &&
        clipboardRequest.operation === "write" &&
        clipboardRequest.text ===
          "did:key:z6Mkr7x1SystemSmokeDeviceDid111111111111111111",
      "System DID copy did not use the exact bounded Home Clipboard request",
      clipboardRequest,
    );
    await page.evaluate(() => window.__systemHost.replyClipboardSuccess());
    await waitForAppearance(
      frame,
      () => document.querySelector("#device-did-copy")?.dataset.copied === "true",
    );
    const copyAfter = await readAppearanceState(frame);
    assert(
      copyAfter.copied === "true" &&
        copyAfter.copyCheckHidden === false &&
        copyAfter.copyIconHidden === true,
      "System DID copy did not wait for trusted Home success before showing copied state",
      copyAfter,
    );

    summaryAppearance = {
      schema: "elastos.home.appearance/v1 ",
      revision: 10,
      theme: "light",
      accent: "orange",
      accent_custom: "#336699",
      dock_auto_hide: false,
      sounds: false,
      focus_mode: false,
      background_image_url: null,
      background_overlay_enabled: true,
      background_overlay_opacity: 0.55,
    };
    const malformedPage = await browser.newPage({
      viewport: { width: 1280, height: 900 },
    });
    await malformedPage.route("**/api/**", async (route) => {
      const request = route.request();
      if (request.method() === "OPTIONS") {
        await route.fulfill({
          status: 204,
          headers: CORS_HEADERS,
          body: "",
        });
        return;
      }
      const url = new URL(request.url());
      if (url.pathname === "/api/apps/system/summary") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          headers: CORS_HEADERS,
          body: JSON.stringify(systemSummary(summaryAppearance)),
        });
        return;
      }
      const inert = inertSystemApiResponse(url.pathname);
      if (inert !== null) {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          headers: CORS_HEADERS,
          body: JSON.stringify(inert),
        });
        return;
      }
      await route.fulfill({
        status: 500,
        contentType: "text/plain; charset=utf-8",
        headers: CORS_HEADERS,
        body: `Unhandled fixture API route: ${url.pathname}`,
      });
    });
    await malformedPage.goto(`${server.baseUrl}/fixture`, {
      waitUntil: "networkidle",
    });
    const malformedFrame = await waitForSystemFrame(malformedPage);
    await malformedFrame.waitForFunction(
      () => !document.querySelector(".system-error")?.hidden,
    );
    const malformedState = await readAppearanceState(malformedFrame);
    assert(
      malformedState.errorHidden === false &&
        malformedState.errorText.length > 0 &&
        malformedState.theme === "",
      "System accepted a malformed authoritative appearance summary",
      malformedState,
    );
    await malformedPage.close();

    assert(
      appearancePlans.length === 0,
      "System behavior smoke left planned appearance replies unused",
      appearancePlans.length,
    );
    assert(
      maxAppearanceInFlight === 1,
      "System appearance writes overlapped instead of serializing",
      { maxAppearanceInFlight },
    );
    assert(
      pageErrors.length === 0,
      "System behavior smoke hit unexpected page errors",
      pageErrors,
    );
    assert(
      requestFailures.length === 0,
      "System behavior smoke hit failed browser requests",
      requestFailures,
    );
    console.log(
      JSON.stringify(
        {
          appearanceWrites: appearanceRequests,
          refreshMessages: await page.evaluate(
            () => window.__systemHost.trace.refreshMessages.length,
          ),
          clipboardRequests: await page.evaluate(
            () => window.__systemHost.trace.clipboardRequests.length,
          ),
          maxAppearanceInFlight,
        },
        null,
        2,
      ),
    );
  } finally {
    await browser?.close();
    await server.close();
  }
}

await main();
