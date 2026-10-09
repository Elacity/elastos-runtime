#!/usr/bin/env node

import {
  assert,
  brave,
  chromium,
  jsonResponse,
  makeAppearanceRecord,
  makeSystemSummary,
  startSystemFixtureServer,
} from "./system-uiux-fixture.mjs";

function appearanceRecord() {
  return makeAppearanceRecord({
    revision: 11,
    theme: "light",
    accent: "custom",
    accent_custom: "#336699",
    dock_auto_hide: false,
    sounds: true,
  });
}

function systemSummary() {
  return makeSystemSummary(appearanceRecord(), {
    proofBindingId: "proof:passkey:system-layout",
    deviceDid: "did:key:z6MkSystemLayoutDeviceDid111111111111111111111",
  });
}

const LAYOUT_HOST_SCRIPT = `
      window.addEventListener("message", (event) => {
        const frame = document.getElementById("system-frame");
        if (!frame || event.source !== frame.contentWindow) {
          return;
        }
        const data = event.data;
        if (
          data &&
          typeof data === "object" &&
          !Array.isArray(data) &&
          data.type === "home:app-ready" &&
          data.homeToken === "system-token"
        ) {
          event.source.postMessage({
            type: "home:clipboard-ready",
            schema: "elastos.home.clipboard.ready/v1",
            targetId: "system",
            homeToken: "system-token",
            parentOrigin: window.location.origin,
            generation: "clipboard-generation-1",
          }, "*");
        }
      });
`;

async function startServer() {
  return startSystemFixtureServer({
    title: "System UIUX layout fixture",
    background: "#0f1217",
    hostScript: LAYOUT_HOST_SCRIPT,
    async onApiRequest({ response, url }) {
      if (url.pathname === "/api/apps/system/summary") {
        jsonResponse(response, systemSummary());
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
      await frame.waitForSelector("#settings-search", { state: "attached" });
      return frame;
    }
    await page.waitForTimeout(50);
  }
  throw new Error("System frame did not load");
}

async function ensureSidebarVisible(frame) {
  const sidebar = frame.locator(".settings-sidebar");
  if (await sidebar.isVisible()) {
    return;
  }
  await frame.click(".sidebar-toggle");
  await frame.waitForFunction(
    () =>
      document.querySelector(".settings-sidebar")?.classList.contains("active") ===
      true,
  );
}

async function activateTab(frame, tab) {
  const tabSelector = `.settings-sidebar-item[data-settings="${tab}"]`;
  const tabButton = frame.locator(tabSelector);
  if (!(await tabButton.isVisible())) {
    await frame.click(".sidebar-toggle");
    await frame.waitForFunction(
      () =>
        document.querySelector(".settings-sidebar")?.classList.contains("active") ===
        true,
    );
  }
  await frame.click(tabSelector);
  await frame.waitForFunction(
    (nextTab) =>
      document
        .querySelector(`.settings-content[data-settings="${nextTab}"]`)
        ?.classList.contains("active") === true,
    tab,
  );
}

async function readLayoutMetrics(frame) {
  return frame.evaluate(() => {
    const rect = (selector) => {
      const element = document.querySelector(selector);
      if (!(element instanceof HTMLElement)) {
        throw new Error(`missing ${selector}`);
      }
      const box = element.getBoundingClientRect();
      return {
        left: box.left,
        top: box.top,
        right: box.right,
        bottom: box.bottom,
        width: box.width,
        height: box.height,
      };
    };
    const contentElement = document.querySelector(".settings-content-container");
    if (!(contentElement instanceof HTMLElement)) {
      throw new Error("missing .settings-content-container");
    }
    const contentStyle = window.getComputedStyle(contentElement);
    const systemFrame = {
      innerWidth: window.innerWidth,
      innerHeight: window.innerHeight,
      scrollWidth: document.scrollingElement?.scrollWidth ?? 0,
      scrollHeight: document.scrollingElement?.scrollHeight ?? 0,
      sidebar: rect(".settings-sidebar"),
      search: rect("#settings-search"),
      content: {
        ...rect(".settings-content-container"),
        clientWidth: contentElement.clientWidth,
        clientHeight: contentElement.clientHeight,
        scrollWidth: contentElement.scrollWidth,
        scrollHeight: contentElement.scrollHeight,
        scrollTop: contentElement.scrollTop,
        overflowY: contentStyle.overflowY,
      },
    };
    return systemFrame;
  });
}

function assertRectWithinViewport(name, rect, metrics) {
  assert(rect.left >= -0.5, `${name} escapes the left viewport edge`, metrics);
  assert(rect.top >= -0.5, `${name} escapes the top viewport edge`, metrics);
  assert(
    rect.right <= metrics.innerWidth + 0.5,
    `${name} escapes the right viewport edge`,
    metrics,
  );
  assert(
    rect.bottom <= metrics.innerHeight + 0.5,
    `${name} escapes the bottom viewport edge`,
    metrics,
  );
}

function assertRectWithinRect(name, rect, bounds, metrics) {
  assert(rect.left >= bounds.left - 0.5, `${name} escapes the left container edge`, metrics);
  assert(rect.top >= bounds.top - 0.5, `${name} escapes the top container edge`, metrics);
  assert(rect.right <= bounds.right + 0.5, `${name} escapes the right container edge`, metrics);
  assert(rect.bottom <= bounds.bottom + 0.5, `${name} escapes the bottom container edge`, metrics);
}

async function scrollControlIntoView(frame, selector) {
  await frame.evaluate((targetSelector) => {
    const content = document.querySelector(".settings-content-container");
    const target = document.querySelector(targetSelector);
    if (!(content instanceof HTMLElement)) {
      throw new Error("missing .settings-content-container");
    }
    if (!(target instanceof HTMLElement)) {
      throw new Error(`missing ${targetSelector}`);
    }
    target.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, selector);
}

async function readControlMetrics(frame, selector) {
  return frame.evaluate((targetSelector) => {
    const target = document.querySelector(targetSelector);
    const content = document.querySelector(".settings-content-container");
    if (!(target instanceof HTMLElement)) {
      throw new Error(`missing ${targetSelector}`);
    }
    if (!(content instanceof HTMLElement)) {
      throw new Error("missing .settings-content-container");
    }
    const targetBox = target.getBoundingClientRect();
    const contentBox = content.getBoundingClientRect();
    return {
      innerWidth: window.innerWidth,
      innerHeight: window.innerHeight,
      content: {
        left: contentBox.left,
        top: contentBox.top,
        right: contentBox.right,
        bottom: contentBox.bottom,
        width: contentBox.width,
        height: contentBox.height,
      },
      target: {
        left: targetBox.left,
        top: targetBox.top,
        right: targetBox.right,
        bottom: targetBox.bottom,
        width: targetBox.width,
        height: targetBox.height,
      },
    };
  }, selector);
}

async function assertScenario(page, width, height, screenshotPath) {
  await page.setViewportSize({ width, height });
  await page.goto(page.url(), { waitUntil: "networkidle" });
  const frame = await waitForSystemFrame(page);

  if (width <= 640) {
    await ensureSidebarVisible(frame);
  }

  for (const tab of ["account", "personalization", "shell", "security", "catalog", "about"]) {
    await activateTab(frame, tab);
  }
  await ensureSidebarVisible(frame);
  await frame.fill("#settings-search", "dock");
  await frame.waitForFunction(
    () =>
      document
        .querySelector('.settings-sidebar-item[data-settings="personalization"]')
        ?.classList.contains("search-hidden") === false,
  );
  await activateTab(frame, "personalization");
  await ensureSidebarVisible(frame);
  await frame.fill("#settings-search", "");
  await frame.waitForFunction(
    () =>
      document.querySelector("#accent-custom-popover")?.hidden === false &&
      document.querySelector('#accent-picker [data-accent-option="custom"]')
        ?.classList.contains("active") === true,
  );

  const metrics = await readLayoutMetrics(frame);
  assert(
    metrics.scrollWidth <= metrics.innerWidth,
    "System page scrolls sideways",
    metrics,
  );
  assert(
    metrics.content.scrollWidth <= metrics.content.clientWidth,
    "System content container scrolls sideways",
    metrics,
  );
  assertRectWithinViewport("System sidebar", metrics.sidebar, metrics);
  assertRectWithinViewport("System search", metrics.search, metrics);
  assertRectWithinViewport("System content", metrics.content, metrics);
  assert(
    metrics.search.bottom <= metrics.sidebar.bottom + 0.5,
    "System search clips outside the sidebar",
    metrics,
  );
  assert(
    metrics.content.overflowY === "auto" &&
      metrics.content.scrollHeight >= metrics.content.clientHeight,
    "System content container does not own vertical scrolling",
    metrics,
  );

  for (const [name, selector] of [
    ["System theme segment", "#theme-segment"],
    ["System accent controls", "#accent-picker"],
    ["System custom accent popover", "#accent-custom-popover"],
  ]) {
    await scrollControlIntoView(frame, selector);
    const controlMetrics = await readControlMetrics(frame, selector);
    assertRectWithinViewport(name, controlMetrics.target, controlMetrics);
    assertRectWithinRect(name, controlMetrics.target, controlMetrics.content, controlMetrics);
    assert(controlMetrics.target.height > 0, `${name} is not visible`, controlMetrics);
  }

  await activateTab(frame, "about");
  await scrollControlIntoView(frame, ".device-did-inline");
  const didMetrics = await readControlMetrics(frame, ".device-did-inline");
  assertRectWithinViewport("System device DID row", didMetrics.target, didMetrics);
  assertRectWithinRect(
    "System device DID row",
    didMetrics.target,
    didMetrics.content,
    didMetrics,
  );
  assert(
    didMetrics.target.height > 0,
    "System device DID row is not visible",
    didMetrics,
  );

  await frame.evaluate(() => {
    const content = document.querySelector(".settings-content-container");
    if (content instanceof HTMLElement) {
      content.scrollTo({ top: 0, left: 0 });
    }
  });

  await frame.locator("body").screenshot({ path: screenshotPath });
}

async function main() {
  const server = await startServer();
  const pageErrors = [];
  const consoleErrors = [];
  const failedRequests = [];
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
    page.on("pageerror", (error) => {
      pageErrors.push(error.message);
    });
    page.on("console", (message) => {
      if (message.type() === "error") {
        consoleErrors.push(message.text());
      }
    });
    page.on("requestfailed", (request) => {
      failedRequests.push(
        `${request.url()} ${request.failure()?.errorText ?? "request failed"}`,
      );
    });

    await page.goto(`${server.baseUrl}/fixture`, { waitUntil: "networkidle" });
    const desktopScreenshot = "/tmp/system-uiux-desktop-1280x900.png";
    const narrowScreenshot = "/tmp/system-uiux-narrow-640x900.png";
    await assertScenario(page, 1280, 900, desktopScreenshot);
    await assertScenario(page, 640, 900, narrowScreenshot);

    assert(
      server.requestFailures.length === 0,
      "System layout fixture returned 500 while serving static assets",
      server.requestFailures,
    );
    assert(
      pageErrors.length === 0,
      "System layout smoke hit page errors",
      pageErrors,
    );
    assert(
      consoleErrors.length === 0,
      "System layout smoke hit console errors",
      consoleErrors,
    );
    assert(
      failedRequests.length === 0,
      "System layout smoke hit failed requests",
      failedRequests,
    );
    console.log(
      JSON.stringify(
        {
          screenshots: [desktopScreenshot, narrowScreenshot],
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
