/* The Home GUI request toast ("Hosted model access request", "Wallet
   approval request", …) reads in Light and Dark: its title and body meet
   WCAG AA (4.5:1) and its dismiss mark meets the 3:1 a control needs, on the
   toast's own fill. The real template markup and stylesheets, in Chromium. */

import assert from "node:assert/strict";
import { createServer } from "node:http";
import { once } from "node:events";
import { readFile } from "node:fs/promises";
import { extname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { chromium } from "./system-uiux-fixture.mjs";

const root = fileURLToPath(new URL("../capsules/home-gui/browser/", import.meta.url));

/* Browser-side: contrast of each toast text and of the dismiss mark's
   stroke against the toast fill (alpha layers composited over the page). */
function toastContrast() {
  const parse = (value) => {
    const m = String(value).match(/rgba?\(([^)]+)\)/);
    if (m) {
      const [r, g, b, a = 1] = m[1].split(/[ ,/]+/).filter(Boolean).map(Number);
      return { r, g, b, a };
    }
    const hex = String(value).match(/^#([0-9a-f]{6})$/i)?.[1];
    if (hex) return { r: parseInt(hex.slice(0, 2), 16), g: parseInt(hex.slice(2, 4), 16), b: parseInt(hex.slice(4), 16), a: 1 };
    if (value === "white") return { r: 255, g: 255, b: 255, a: 1 };
    throw new Error(`unparsed colour ${value}`);
  };
  const over = (top, under) => ({ r: top.r * top.a + under.r * (1 - top.a), g: top.g * top.a + under.g * (1 - top.a),
    b: top.b * top.a + under.b * (1 - top.a), a: 1 });
  const lum = ({ r, g, b }) => [r, g, b].map((v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; })
    .reduce((sum, v, i) => sum + v * [0.2126, 0.7152, 0.0722][i], 0);
  const ratio = (a, b) => { const [x, y] = [lum(a), lum(b)]; return Number(((Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05)).toFixed(2)); };
  const toast = document.querySelector("#home-notification-toast");
  const fill = over(parse(getComputedStyle(toast).backgroundColor), parse(getComputedStyle(document.body).backgroundColor));
  const text = (selector) => ratio(over(parse(getComputedStyle(document.querySelector(selector)).color), fill), fill);
  const dismiss = getComputedStyle(document.querySelector("#home-notification-dismiss"));
  const dismissFill = over(parse(dismiss.backgroundColor), fill);
  const stroke = decodeURIComponent(dismiss.backgroundImage).match(/stroke='([^']+)'/)?.[1];
  return {
    title: text("#home-notification-title"),
    body: text("#home-notification-body"),
    dismissMark: ratio(parse(stroke), dismissFill),
  };
}

test("the request toast reads in Light and Dark", async (t) => {
  const template = await readFile(join(root, "home-gui-template.html"), "utf8");
  const start = template.lastIndexOf("<section", template.indexOf('id="home-notification-toast"'));
  const toast = template.slice(start, template.indexOf("</section>", start) + "</section>".length)
    .replace(/\s(hidden|inert)(?=[\s>])/g, "");
  const server = createServer(async (req, res) => {
    const path = new URL(req.url, "http://fixture").pathname;
    if (path.startsWith("/page/")) {
      const theme = path.slice("/page/".length);
      res.writeHead(200, { "content-type": "text/html" });
      // The Home stage behind the toast: the theme's own background.
      res.end(`<!doctype html><html${theme === "light" ? ' data-el-theme="light"' : ""}><head>
        <link rel="stylesheet" href="/elastos-ui.css"><link rel="stylesheet" href="/style.css">
        <style>body { margin: 0; height: 100vh; background: var(--bg); }</style></head><body>${toast}</body></html>`);
      return;
    }
    res.writeHead(200, { "content-type": { ".css": "text/css" }[extname(path)] || "application/octet-stream" });
    res.end(await readFile(join(root, path.slice(1))));
  });
  server.listen(0, "127.0.0.1"); await once(server, "listening");
  const browser = await chromium.launch({ headless: true });
  t.after(async () => { await browser.close(); server.close(); });
  for (const theme of ["light", "dark"]) {
    const page = await browser.newPage();
    await page.goto(`http://127.0.0.1:${server.address().port}/page/${theme}`, { waitUntil: "load" });
    const seen = await page.evaluate(toastContrast);
    assert.ok(seen.title >= 4.5 && seen.body >= 4.5, `${theme}: toast text meets 4.5:1 ${JSON.stringify(seen)}`);
    assert.ok(seen.dismissMark >= 3, `${theme}: the dismiss mark meets 3:1 ${JSON.stringify(seen)}`);
    await page.close();
  }
});
