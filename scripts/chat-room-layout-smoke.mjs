#!/usr/bin/env node

import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = fileURLToPath(new URL("../", import.meta.url));
const chatRoot = join(repoRoot, "capsules/chat-room/browser");
const indexSource = readFileSync(join(chatRoot, "index.html"), "utf8").replace(
  /<script type="module">[\s\S]*?<\/script>/,
  "",
);

function assert(condition, message, details = undefined) {
  if (condition) return;
  const suffix = details === undefined ? "" : `\n${JSON.stringify(details, null, 2)}`;
  throw new Error(`${message}${suffix}`);
}

function send(response, status, contentType, body) {
  const bytes = Buffer.from(body);
  response.writeHead(status, {
    "cache-control": "no-store",
    "content-length": bytes.length,
    "content-type": contentType,
  });
  response.end(bytes);
}

const server = createServer((request, response) => {
  const url = new URL(request.url || "/", "http://127.0.0.1");
  if (url.pathname === "/") {
    send(response, 200, "text/html; charset=utf-8", indexSource);
    return;
  }
  if (url.pathname === "/style.css") {
    send(
      response,
      200,
      "text/css; charset=utf-8",
      readFileSync(join(chatRoot, "style.css")),
    );
    return;
  }
  send(response, 404, "text/plain; charset=utf-8", "not found");
});

async function listen() {
  await new Promise((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  const address = server.address();
  assert(address && typeof address === "object", "Chat layout fixture did not bind");
  return `http://127.0.0.1:${address.port}`;
}

function playwrightSpecifier() {
  const configured = process.env.ELASTOS_PLAYWRIGHT_MODULE || "";
  if (configured) {
    return configured.startsWith("file:")
      ? configured
      : pathToFileURL(resolve(configured)).href;
  }
  return pathToFileURL(
    join(
      repoRoot,
      "elastos/tools/browser-playwright-engine/node_modules/playwright/index.js",
    ),
  ).href;
}

const origin = await listen();
let browser = null;
try {
  const imported = await import(playwrightSpecifier());
  const { chromium } = imported.default || imported;
  browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  const results = [];

  for (const width of [1400, 1000, 760, 420]) {
    await page.setViewportSize({ width, height: 900 });
    await page.goto(
      `${origin}/?home_origin=${encodeURIComponent(origin)}#home_token=layout-proof`,
      { waitUntil: "load" },
    );
    const result = await page.evaluate(() => {
      document.body.dataset.roomAccessMode = "shell";
      document.body.dataset.roomSessionActive = "true";
      document.body.dataset.roomJoinVisible = "true";
      const chatCard = document.querySelector("#chat-card");
      chatCard.dataset.rosterOpen = "true";
      for (const id of [
        "room-access-toggle",
        "room-access-section",
        "conversation-join-section",
        "conversation-invite-output-row",
      ]) {
        document.querySelector(`#${id}`).hidden = false;
      }
      document.querySelector("#conversation-join-input").value =
        "elastos://peer/invite?token=" + "j".repeat(180);
      document.querySelector("#conversation-invite-output").value =
        "elastos://peer/invite?token=" + "a".repeat(180);
      document.querySelector("#room-policy-list").innerHTML = `
        <div class="policy-row">
          <span class="policy-row-name">Allow trusted ElastOS participants with a deliberately long label</span>
          <button type="button" aria-pressed="true">On</button>
        </div>`;
      document.querySelector("#node-list").innerHTML = `
        <li class="node-row">
          <div class="node-row-head">
            <span class="node-row-name">did:elastos:${"c".repeat(120)}</span>
            <span class="node-row-detail">Trusted participant</span>
          </div>
        </li>`;

      for (const selector of [
        "#conversation-join-section",
        "#conversation-join-input",
        "#conversation-join-submit",
      ]) {
        if (document.querySelector(selector).getClientRects().length === 0) {
          throw new Error(`Chat join control is not visible: ${selector}`);
        }
      }
      if (
        !document
          .querySelector("#conversation-join-section p")
          .textContent.includes("replaces this unused local conversation")
      ) {
        throw new Error(
          "Chat join copy does not explain unused-conversation replacement",
        );
      }
      for (const selector of ["#message-list", "#composer-form"]) {
        if (getComputedStyle(document.querySelector(selector)).display !== "none") {
          throw new Error(`Chat surface remains visible behind join UI: ${selector}`);
        }
      }

      const selectors = [
        "html",
        "body",
        "#chat-card",
        "#conversation-join-section",
        "#conversation-join-form",
        "#conversation-join-input",
        "#conversation-join-submit",
        ".presence-card",
        "#room-access-section",
        ".join-link-card",
        "#conversation-invite-output-row",
        ".policy-row",
        ".node-row",
      ];
      return selectors.map((selector) => {
        const element = document.querySelector(selector);
        const bounds = element.getBoundingClientRect();
        return {
          selector,
          clientWidth: element.clientWidth,
          scrollWidth: element.scrollWidth,
          left: bounds.left,
          right: bounds.right,
          viewportWidth: innerWidth,
          checkInternalOverflow: element.tagName !== "INPUT",
        };
      });
    });
    const failures = result.filter((entry) =>
      entry.checkInternalOverflow && entry.scrollWidth > entry.clientWidth + 1 ||
      entry.left < -1 ||
      entry.right > entry.viewportWidth + 1
    );
    assert(failures.length === 0, `Chat settings overflow at ${width}px`, failures);
    results.push({ width, checked: result.length });
  }

  process.stdout.write(`${JSON.stringify({
    schema: "elastos.chat-room.layout-smoke/v1",
    ok: true,
    viewports: results,
  })}\n`);
} finally {
  await browser?.close().catch(() => {});
  await new Promise((resolveClose) => server.close(resolveClose));
}
