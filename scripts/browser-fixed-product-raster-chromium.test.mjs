import assert from "node:assert/strict";
import { createRequire } from "node:module";
import http from "node:http";
import test from "node:test";

import {
  browserDisplayMetrics,
  projectRuntimeProxyOnlineState,
} from "./browser-selkies-control-service.mjs";

const require = createRequire(
  new URL(
    "../elastos/tools/browser-playwright-engine/package.json",
    import.meta.url,
  ),
);
const { chromium } = require("playwright");

function listen(server) {
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      server.off("error", reject);
      resolve(server.address().port);
    });
  });
}

function close(server) {
  return new Promise((resolve) => server.close(resolve));
}

test("installed-shaped Chromium fills the DPR-1 raster through a loopback proxy", async () => {
  const proxyRequests = [];
  const proxy = http.createServer((request, response) => {
    proxyRequests.push({
      method: request.method,
      url: request.url,
    });
    let target;
    try {
      target = new URL(request.url);
    } catch {
      response.writeHead(400).end();
      return;
    }
    if (target.hostname !== "runtime-only.invalid") {
      response.writeHead(502).end();
      return;
    }
    response.writeHead(200, {
      "cache-control": "no-store",
      "content-type": "text/html; charset=utf-8",
    });
    response.end(`<!doctype html>
      <meta charset="utf-8">
      <style>
        html, body { margin: 0; width: 100%; height: 100%; overflow: hidden; }
        #tl, #tr, #bl, #br { position: fixed; width: 50vw; height: 50vh; }
        #tl { inset: 0 auto auto 0; background: #f00; }
        #tr { inset: 0 0 auto auto; background: #0f0; }
        #bl { inset: auto auto 0 0; background: #00f; }
        #br { inset: auto 0 0 auto; background: #fff; }
      </style>
      <div id="tl"></div><div id="tr"></div><div id="bl"></div><div id="br"></div>
      <script>
        globalThis.clickedCorners = [];
        for (const id of ["tl", "tr", "bl", "br"]) {
          document.getElementById(id).addEventListener("click", () => {
            globalThis.clickedCorners.push(id);
          });
        }
      </script>`);
  });
  proxy.on("connect", (_request, socket) => {
    socket.end("HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n");
  });

  const proxyPort = await listen(proxy);
  const browser = await chromium.launch({
    headless: true,
    proxy: { server: `http://127.0.0.1:${proxyPort}` },
    args: [
      "--disable-background-networking",
      "--disable-component-update",
      "--disable-default-apps",
      "--disable-quic",
      "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1",
      "--no-first-run",
    ],
  });
  try {
    const metrics = browserDisplayMetrics({
      displaySurface: {
        stream: { width: 1920, height: 1080 },
      },
    });
    const context = await browser.newContext({
      viewport: { width: metrics.width, height: metrics.height },
      deviceScaleFactor: metrics.deviceScaleFactor,
    });
    const page = await context.newPage();
    const cdp = await context.newCDPSession(page);
    const runtimeProxyUrl = new URL(`http://127.0.0.1:${proxyPort}/`);
    const cdpAdapter = {
      request(method, params) {
        return cdp.send(method, params);
      },
    };

    await projectRuntimeProxyOnlineState(cdpAdapter, runtimeProxyUrl);
    await page.goto("http://runtime-only.invalid/", {
      waitUntil: "domcontentloaded",
    });
    const projected = await projectRuntimeProxyOnlineState(
      cdpAdapter,
      runtimeProxyUrl,
    );
    assert.equal(projected.online, true);

    const geometry = await page.evaluate(() => ({
      width: innerWidth,
      height: innerHeight,
      dpr: devicePixelRatio,
      online: navigator.onLine,
      corners: [
        document.elementFromPoint(1, 1)?.id,
        document.elementFromPoint(innerWidth - 1, 1)?.id,
        document.elementFromPoint(1, innerHeight - 1)?.id,
        document.elementFromPoint(innerWidth - 1, innerHeight - 1)?.id,
      ],
    }));
    assert.deepEqual(geometry, {
      width: 1920,
      height: 1080,
      dpr: 1,
      online: true,
      corners: ["tl", "tr", "bl", "br"],
    });

    for (const [x, y] of [
      [1, 1],
      [1919, 1],
      [1, 1079],
      [1919, 1079],
    ]) {
      await page.mouse.click(x, y);
    }
    assert.deepEqual(
      await page.evaluate(() => globalThis.clickedCorners),
      ["tl", "tr", "bl", "br"],
    );

    const screenshot = await page.screenshot();
    assert.equal(screenshot.readUInt32BE(16), 1920);
    assert.equal(screenshot.readUInt32BE(20), 1080);
    assert.ok(
      proxyRequests.some(
        ({ method, url }) =>
          method === "GET" &&
          new URL(url).hostname === "runtime-only.invalid",
      ),
      "fixture navigation did not traverse the loopback Runtime-shaped proxy",
    );
    await context.close();
  } finally {
    await browser.close();
    await close(proxy);
  }
});
