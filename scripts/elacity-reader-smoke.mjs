#!/usr/bin/env node
//
// Runs Elacity Reader in a real engine, in the frame it actually runs in: a
// child frame with no standing of its own, exactly as Home gives it. Nothing
// here is a stand-in for a parser — the markup cases below are handed to the
// engine's own parser, which is the half the unit tests cannot reach.
//
// Covered:
//   * the whole session — open, read part by part, reassemble, draw, close;
//   * a session that is not a file session, refused and handed straight back;
//   * a picture, words over more than one part, a document, a comic pager and
//     a book;
//   * a file this reader has no way to show, said plainly;
//   * a book whose chapters carry every markup trick the rebuild is meant to
//     survive, checked against the engine's parser rather than a stand-in;
//   * the two ways a background thread is refused at this kind of origin, so
//     the document renderer's choice stays measured rather than remembered.

import { createServer } from "node:http";
import { createRequire } from "node:module";
import { deflateSync } from "node:zlib";
import { readFile } from "node:fs/promises";
import { extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../", import.meta.url)));
const browserRoot = join(repoRoot, "capsules/elacity-reader/browser");
const brave =
  process.env.BRAVE_BIN || "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";
const require = createRequire(
  new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url),
);
const { chromium } = require("playwright");

const { buildStoredZip } = await import(
  new URL("../capsules/elacity-reader/browser/zip-fixture.test-helper.mjs", import.meta.url)
);

const HOME_TOKEN = "elacity-reader-smoke-token";
const CHUNK_BYTES = 1024 * 1024;
const HANDLE = "e".repeat(64);
const OPEN_SCHEMA = "elastos.library.runtime-custody-viewer/v1";
const PART_SCHEMA = "elastos.library.runtime-custody-viewer-part/v1";

function assert(condition, message, details = undefined) {
  if (!condition) {
    throw new Error(`${message}${details ? `\n${JSON.stringify(details, null, 2)}` : ""}`);
  }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let index = 0; index < 256; index += 1) {
    let value = index;
    for (let bit = 0; bit < 8; bit += 1) {
      value = value & 1 ? (value >>> 1) ^ 0xedb88320 : value >>> 1;
    }
    table[index] = value >>> 0;
  }
  return table;
})();

function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) crc = CRC_TABLE[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

function pngChunk(type, data) {
  const head = Buffer.alloc(4);
  head.writeUInt32BE(data.length, 0);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const tail = Buffer.alloc(4);
  tail.writeUInt32BE(crc32(body), 0);
  return Buffer.concat([head, body, tail]);
}

/** A real picture, written out here so the engine has real bytes to decode. */
function pngBytes(width, height, colour) {
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8;
  header[9] = 2;
  const rows = [];
  for (let row = 0; row < height; row += 1) {
    const line = [0];
    for (let column = 0; column < width; column += 1) line.push(...colour);
    rows.push(Buffer.from(line));
  }
  return Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    pngChunk("IHDR", header),
    pngChunk("IDAT", deflateSync(Buffer.concat(rows))),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

/** A real document with `pages` pages, cross-reference table and all. */
function pdfBytes(pages) {
  const objects = [];
  const kids = [];
  for (let index = 0; index < pages; index += 1) {
    kids.push(`${4 + index * 2} 0 R`);
  }
  objects.push("<< /Type /Catalog /Pages 2 0 R >>");
  objects.push(`<< /Type /Pages /Kids [${kids.join(" ")}] /Count ${pages} >>`);
  objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
  for (let index = 0; index < pages; index += 1) {
    const contentIndex = 5 + index * 2;
    objects.push(
      `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 260] /Resources << /Font << /F1 3 0 R >> >> /Contents ${contentIndex} 0 R >>`,
    );
    const stream = `BT /F1 18 Tf 20 120 Td (Page ${index + 1}) Tj ET`;
    objects.push(`<< /Length ${stream.length} >>\nstream\n${stream}\nendstream`);
  }
  let body = "%PDF-1.4\n";
  const offsets = [];
  objects.forEach((object, index) => {
    offsets.push(body.length);
    body += `${index + 1} 0 obj\n${object}\nendobj\n`;
  });
  const startxref = body.length;
  body += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  for (const offset of offsets) {
    body += `${String(offset).padStart(10, "0")} 00000 n \n`;
  }
  body += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${startxref}\n%%EOF\n`;
  return Buffer.from(body, "latin1");
}

const CONTAINER_XML = `<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>`;

function packageXml(chapters, direction = "ltr") {
  const items = chapters
    .map((name) => `<item id="${name}" href="${name}.xhtml" media-type="application/xhtml+xml"/>`)
    .join("\n    ");
  const spine = chapters.map((name) => `<itemref idref="${name}"/>`).join("\n    ");
  return `<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
  <metadata><dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">Smoke</dc:title></metadata>
  <manifest>
    ${items}
    <item id="plate" href="images/plate.png" media-type="image/png"/>
  </manifest>
  <spine page-progression-direction="${direction}">
    ${spine}
  </spine>
</package>`;
}

// Chapter one is well-formed and ordinary: a script to drop, a picture the
// book carries, and a link that leaves the book.
const CHAPTER_PLAIN = `<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>One</title></head><body>
  <h1>Chapter one</h1>
  <script>window.alert("chapter script ran")</script>
  <p id="lead" class="lead">The <em>first</em> chapter.</p>
  <img src="images/plate.png" alt="Plate"/>
  <a href="https://example.org/notes">Notes</a>
  <a href="mailto:reader@example.org">Write</a>
</body></html>`;

// Chapter two is deliberately not well-formed, so the first parse reports an
// error and the second one — the engine's markup parser — is the one that
// produces the tree. Everything in it is a trick that only that parser builds:
// a drawing with a body inside it, a script the parser lifts out of a table,
// a picture inside a no-script block, a template, an escaped script link, and
// the same source given twice.
const CHAPTER_MALFORMED_HTML = `<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body>
  <h1>Chapter two</h1>
  <p>Second chapter words.
  <svg><foreignObject><p onclick="window.alert('foreign object')">inside a drawing</p></foreignObject></svg>
  <table><script>window.alert("fostered script")</script><tr><td>cell</td></tr></table>
  <noscript><img src="x" onerror="window.alert('no-script image')"/></noscript>
  <template><p onmouseover="window.alert('template')">templated</p></template>
  <a href="javascript&colon;window.alert('escaped scheme')">escaped</a>
  <img src="images/plate.png" src="https://tracker.example/pixel.png" alt="Twice"/>
  <div>closing words
</body></html>`;

// Chapter three is well-formed, so it is the strict parse that builds it, and
// it carries what only that parse can produce: attribute names that keep their
// case, an attribute in a real namespace, and an element whose namespace is
// declared to be the drawing one while its name still reads as ordinary.
const CHAPTER_XHTML_TRICKS = `<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:xlink="http://www.w3.org/1999/xlink"><body>
  <h1>Chapter three</h1>
  <p>Third chapter words.</p>
  <a HREF="javascript:window.alert('uppercase href')">uppercase</a>
  <img SRC="https://tracker.example/pixel.png" alt="uppercase source"/>
  <a xlink:href="javascript:window.alert('namespaced href')" href="https://example.org/ok">namespaced</a>
  <p xmlns="http://www.w3.org/2000/svg">drawing namespace paragraph</p>
</body></html>`;

// A book big enough to arrive in more than one part, with the filler written
// first so every chapter's bytes land past the boundary between the parts.
// The archive checks each file against its own checksum, so a chapter that
// reads back correctly is a chapter the parts were put together correctly for.
const BOOK_FILLER = Buffer.alloc(1_200_000, 0x2e);

function bookBytes({ direction = "ltr" } = {}) {
  return buildStoredZip({
    "OEBPS/filler.txt": BOOK_FILLER,
    mimetype: "application/epub+zip",
    "META-INF/container.xml": CONTAINER_XML,
    "OEBPS/content.opf": packageXml(["one", "two", "three"], direction),
    "OEBPS/one.xhtml": CHAPTER_PLAIN,
    "OEBPS/two.xhtml": CHAPTER_MALFORMED_HTML,
    "OEBPS/three.xhtml": CHAPTER_XHTML_TRICKS,
    "OEBPS/images/plate.png": pngBytes(3, 3, [40, 90, 200]),
  });
}

// Named out of reading order on purpose: the pager has to put page 2 first.
const comicBytes = buildStoredZip({
  "pages/page10.png": pngBytes(7, 4, [200, 40, 40]),
  "pages/page2.png": pngBytes(3, 4, [40, 200, 40]),
});

// Just over one part long, and over the number of characters the reader will
// put on screen at once, so both the second part and the cap are exercised.
const wordsFixture = Buffer.from(
  `${"Elacity Reader smoke words. ".repeat(40000)}${"x".repeat(23)}TAIL-MARKER`,
  "utf8",
);

/** A real model: one solid triangle, written the way the format packs them. */
function stlBytes() {
  const triangles = [
    [[0, 0, 1], [0, 0, 0, 1, 0, 0, 0, 1, 0]],
    [[0, -1, 0], [0, 0, 0, 1, 0, 0, 0, 0, 1]],
  ];
  const bytes = Buffer.alloc(84 + triangles.length * 50);
  bytes.write("elacity reader smoke model", 0, "ascii");
  bytes.writeUInt32LE(triangles.length, 80);
  triangles.forEach(([normal, corners], index) => {
    let at = 84 + index * 50;
    for (const value of normal) {
      bytes.writeFloatLE(value, at);
      at += 4;
    }
    for (const value of corners) {
      bytes.writeFloatLE(value, at);
      at += 4;
    }
  });
  return bytes;
}

const FIXTURES = new Map([
  [
    "11".repeat(32),
    { label: "picture", contentType: "image/png", bytes: pngBytes(24, 12, [220, 180, 60]) },
  ],
  [
    "22".repeat(32),
    { label: "words", contentType: "text/plain", bytes: wordsFixture },
  ],
  [
    "33".repeat(32),
    { label: "document", contentType: "application/pdf", bytes: pdfBytes(2) },
  ],
  [
    "44".repeat(32),
    { label: "comic", contentType: "application/vnd.comicbook+zip", bytes: comicBytes },
  ],
  [
    "55".repeat(32),
    { label: "book", contentType: "application/epub+zip", bytes: bookBytes() },
  ],
  [
    "66".repeat(32),
    { label: "unknown", contentType: "application/x-unknown", bytes: Buffer.from("nothing shows this") },
  ],
  [
    "77".repeat(32),
    { label: "wrong kind", contentType: "video/mp4", bytes: Buffer.from("not for this reader"), kind: "media" },
  ],
  [
    "99".repeat(32),
    { label: "model", contentType: "model/stl", bytes: stlBytes() },
  ],
  [
    "88".repeat(32),
    { label: "book right to left", contentType: "application/epub+zip", bytes: bookBytes({ direction: "rtl" }) },
  ],
]);

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

const CONTENT_TYPES = {
  ".css": "text/css",
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".png": "image/png",
  ".svg": "image/svg+xml",
};

// The same headers the runtime puts on a capsule's files: a frame with no
// standing of its own can only read them when the answer says so outright.
const ASSET_HEADERS = {
  "access-control-allow-origin": "null",
  "cache-control": "no-store",
  "cross-origin-resource-policy": "cross-origin",
  "x-content-type-options": "nosniff",
};

function startServer() {
  const calls = [];
  const notFound = [];
  const server = createServer(async (request, response) => {
    const url = new URL(request.url, "http://localhost");
    try {
      if (request.method === "OPTIONS") {
        response
          .writeHead(204, {
            ...ASSET_HEADERS,
            "access-control-allow-headers": "content-type,x-elastos-home-token",
            "access-control-allow-methods": "GET,POST,OPTIONS",
          })
          .end();
        return;
      }
      if (url.pathname === "/favicon.ico") {
        response.writeHead(204, ASSET_HEADERS).end();
        return;
      }
      if (url.pathname === "/fixture") {
        const mintId = url.searchParams.get("mint_id") || "";
        const source = `/apps/elacity-reader/?mint_id=${encodeURIComponent(mintId)}#home_token=${encodeURIComponent(HOME_TOKEN)}`;
        // No standing of its own, exactly as the shell gives it: scripts may
        // run, and nothing else is granted.
        const body = Buffer.from(`<!doctype html>
<html><body style="margin:0">
<iframe id="reader" title="Reader" sandbox="allow-scripts allow-modals" src="${source}" style="border:0;width:100%;height:100vh"></iframe>
</body></html>`);
        response.writeHead(200, {
          "content-length": body.length,
          "content-type": "text/html; charset=utf-8",
        });
        response.end(body);
        return;
      }
      if (url.pathname.startsWith("/apps/elacity-reader/")) {
        const relative =
          url.pathname === "/apps/elacity-reader/"
            ? "index.html"
            : url.pathname.slice("/apps/elacity-reader/".length);
        const path = join(browserRoot, relative);
        assert(path === join(browserRoot, "index.html") || path.startsWith(`${browserRoot}/`), "unsafe asset path", { path });
        const body = await readFile(path);
        response.writeHead(200, {
          ...ASSET_HEADERS,
          "content-length": body.length,
          "content-type": CONTENT_TYPES[extname(path)] || "application/octet-stream",
        });
        response.end(body);
        return;
      }
      if (url.pathname.startsWith("/api/provider/object/")) {
        const operation = url.pathname.slice("/api/provider/object/".length);
        let text = "";
        for await (const piece of request) text += Buffer.from(piece).toString("utf8");
        const body = text ? JSON.parse(text) : {};
        calls.push({
          operation,
          mintId: body.mint_id || "",
          chunkIndex: body.chunk_index,
          handle: body.viewer_session_handle || "",
          token: request.headers["x-elastos-home-token"] || "",
        });
        const answer = respond(operation, body);
        const payload = Buffer.from(JSON.stringify(answer.payload));
        response.writeHead(answer.status, {
          ...ASSET_HEADERS,
          "content-length": payload.length,
          "content-type": "application/json",
        });
        response.end(payload);
        return;
      }
      notFound.push(url.pathname);
      response.writeHead(404, ASSET_HEADERS).end("not found");
    } catch (error) {
      response.writeHead(500, { "content-type": "text/plain; charset=utf-8" });
      response.end(error.stack || String(error));
    }
  });
  return { calls, notFound, server };
}

function respond(operation, body) {
  const fixture = FIXTURES.get(String(body.mint_id || ""));
  if (!fixture) {
    return { status: 404, payload: { status: "error", message: "This file is unavailable." } };
  }
  if (operation === "open_viewer") {
    return {
      status: 200,
      payload: {
        status: "ok",
        data: {
          schema: OPEN_SCHEMA,
          content_kind: fixture.kind || "object",
          mint_id: body.mint_id,
          viewer_session_handle: HANDLE,
          expires_at: 1_780_000_000,
          content_type: fixture.contentType,
          plaintext_bytes: fixture.bytes.length,
          chunk_count: Math.ceil(fixture.bytes.length / CHUNK_BYTES),
          chunk_plaintext_bytes: CHUNK_BYTES,
        },
      },
    };
  }
  if (operation === "read_viewer") {
    const index = Number(body.chunk_index);
    const slice = fixture.bytes.subarray(index * CHUNK_BYTES, (index + 1) * CHUNK_BYTES);
    return {
      status: 200,
      payload: {
        status: "ok",
        data: {
          schema: PART_SCHEMA,
          content_kind: "object",
          mint_id: body.mint_id,
          viewer_session_handle: HANDLE,
          chunk_index: index,
          encoding: "base64",
          data: Buffer.from(slice).toString("base64"),
        },
      },
    };
  }
  return { status: 200, payload: { status: "ok", data: { closed: true } } };
}

// ---------------------------------------------------------------------------
// Page helpers
// ---------------------------------------------------------------------------

async function readerFrame(page) {
  for (let attempt = 0; attempt < 80; attempt += 1) {
    const frame = page.frames().find((entry) => entry.url().includes("/apps/elacity-reader/"));
    if (frame) return frame;
    await page.waitForTimeout(50);
  }
  throw new Error("the reader frame never appeared");
}

async function openCase(page, mintId, port) {
  await page.goto(`http://127.0.0.1:${port}/fixture?mint_id=${mintId}`);
  const frame = await readerFrame(page);
  await frame.waitForFunction(
    () => document.getElementById("reader-status")?.dataset.state !== "info" ||
      document.getElementById("reader-status")?.textContent === "",
    undefined,
    { timeout: 30000 },
  );
  return frame;
}

/**
 * Everything a rebuilt chapter is checked against, read off the live page
 * rather than off any markup: nothing that runs, no handler of any kind, every
 * picture source minted by this page, and every link one that leaves the book
 * by an address a person can read.
 */
const CHAPTER_AUDIT = () => {
  const article = document.querySelector(".book-sheet");
  if (!article) return { missing: true };
  const all = Array.from(article.querySelectorAll("*"));
  const handlers = [];
  const namespaced = [];
  for (const element of all) {
    for (const attribute of Array.from(element.attributes)) {
      if (attribute.name.toLowerCase().startsWith("on")) {
        handlers.push(`${element.localName}[${attribute.name}]`);
      }
      if (attribute.name.includes(":") || attribute.namespaceURI) {
        namespaced.push(`${element.localName}[${attribute.name}]`);
      }
    }
  }
  return {
    text: article.textContent || "",
    tags: Array.from(new Set(all.map((element) => element.localName))).sort(),
    namespaces: Array.from(new Set(all.map((element) => element.namespaceURI))).sort(),
    scripts: article.querySelectorAll("script").length,
    styles: article.querySelectorAll("style").length,
    templates: article.querySelectorAll("template").length,
    handlers,
    namespaced,
    sources: Array.from(article.querySelectorAll("[src]")).map((element) => element.getAttribute("src")),
    links: Array.from(article.querySelectorAll("[href]")).map((element) => element.getAttribute("href")),
    identifiers: all.filter((element) => element.hasAttribute("id") || element.hasAttribute("class")).length,
    direction: article.getAttribute("dir"),
  };
};

function auditChapter(audit, where) {
  assert(!audit.missing, `${where}: the book never reached the page`);
  assert(audit.scripts === 0, `${where}: something that runs survived the rebuild`, audit);
  assert(audit.styles === 0, `${where}: a style block survived the rebuild`, audit);
  assert(audit.templates === 0, `${where}: a template survived the rebuild`, audit);
  assert(audit.handlers.length === 0, `${where}: a handler attribute survived the rebuild`, audit);
  assert(audit.namespaced.length === 0, `${where}: a namespaced attribute survived the rebuild`, audit);
  assert(
    audit.sources.every((source) => source.startsWith("blob:")),
    `${where}: a picture source the page did not mint survived`,
    audit,
  );
  assert(
    audit.links.every((link) => /^(https?:|mailto:)/.test(link)),
    `${where}: a link this reader does not allow survived`,
    audit,
  );
  assert(
    audit.namespaces.every((namespace) => namespace === "http://www.w3.org/1999/xhtml"),
    `${where}: an element outside the page's own namespace survived`,
    audit,
  );
  assert(audit.identifiers === 0, `${where}: an author-chosen handle survived`, audit);
  assert(!/alert|javascript:/i.test(audit.text), `${where}: script text survived as words`, audit);
}

// ---------------------------------------------------------------------------
// Run
// ---------------------------------------------------------------------------

async function run() {
  const { calls, notFound, server } = startServer();
  const port = await new Promise((resolvePort, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => resolvePort(server.address().port));
  });
  const browser = await chromium.launch({ executablePath: brave, headless: true });
  const page = await browser.newPage();
  const pageErrors = [];
  const consoleErrors = [];
  const dialogs = [];
  page.on("pageerror", (error) => pageErrors.push(String(error)));
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  // The page's own rules refusing something is the rules working, so those are
  // counted rather than treated as breakage — and each one is asserted below,
  // so a refusal that stops happening is a failure too.
  const REFUSALS = [
    {
      name: "an inline style the engine's parser tried to apply while building an adversarial chapter",
      match: (text) => /style-src/.test(text),
    },
    {
      name: "a background thread from a minted address",
      match: (text) => /worker-src/.test(text),
    },
  ];
  page.on("dialog", async (dialog) => {
    dialogs.push(`${dialog.type()}: ${dialog.message()}`);
    await dialog.dismiss();
  });

  try {
    // --- a picture -------------------------------------------------------
    let frame = await openCase(page, "11".repeat(32), port);
    let state = await frame.evaluate(() => ({
      kind: document.getElementById("reader-kind").textContent,
      status: document.getElementById("reader-status").textContent,
      controlsHidden: document.getElementById("reader-controls").hidden,
      source: document.querySelector(".image-sheet")?.getAttribute("src") || "",
      width: document.querySelector(".image-sheet")?.naturalWidth || 0,
    }));
    assert(state.kind === "Picture", "a picture must be named a picture", state);
    assert(state.source.startsWith("blob:"), "a picture must be drawn from a source this page minted", state);
    assert(state.width === 24, "the picture the reader drew must be the picture that was sent", state);
    assert(state.controlsHidden, "a picture has no pages to step through", state);

    // --- words over more than one part ------------------------------------
    frame = await openCase(page, "22".repeat(32), port);
    state = await frame.evaluate(() => ({
      kind: document.getElementById("reader-kind").textContent,
      length: (document.querySelector(".text-sheet")?.textContent || "").length,
      note: document.querySelector(".view-note")?.textContent || "",
      code: document.querySelector(".text-sheet")?.dataset.code,
    }));
    const wordsCalls = calls.filter(
      (call) => call.mintId === "22".repeat(32) && call.operation === "read_viewer",
    );
    assert(state.kind === "Text", "words must be named words", state);
    assert(wordsCalls.length === 2, "a file over one part long must be read part by part", wordsCalls);
    assert(
      wordsCalls.map((call) => call.chunkIndex).join(",") === "0,1",
      "the parts must be asked for in order",
      wordsCalls,
    );
    assert(state.length === 1_000_000, "a long file is shown up to the reader's own limit", state);
    assert(
      state.note.includes("first 1,000,000 characters"),
      "a file cut short must say so on the screen",
      state,
    );
    assert(state.code === "false", "ordinary words are not shown as source", state);

    // --- a document, drawn without a background thread ---------------------
    frame = await openCase(page, "33".repeat(32), port);
    state = await frame.evaluate(() => ({
      kind: document.getElementById("reader-kind").textContent,
      position: document.getElementById("reader-position").textContent,
      width: document.querySelector(".page-sheet")?.width || 0,
      height: document.querySelector(".page-sheet")?.height || 0,
      previousDisabled: document.getElementById("reader-previous").disabled,
      nextDisabled: document.getElementById("reader-next").disabled,
      drawn: (() => {
        const canvas = document.querySelector("canvas.page-sheet");
        if (!canvas) return 0;
        const pixels = canvas.getContext("2d").getImageData(0, 0, canvas.width, canvas.height).data;
        let ink = 0;
        for (let at = 0; at < pixels.length; at += 4) {
          if (pixels[at] !== 255 || pixels[at + 1] !== 255 || pixels[at + 2] !== 255) ink += 1;
        }
        return ink;
      })(),
    }));
    assert(state.kind === "Document", "a document must be named a document", state);
    assert(state.position === "Page 1 of 2", "a two-page document opens on its first page", state);
    assert(state.width > 0 && state.height > 0, "a document page must be drawn at a real size", state);
    assert(state.drawn > 0, "a document page must actually have something drawn on it", state);
    assert(state.previousDisabled && !state.nextDisabled, "the first page steps forward only", state);
    await frame.click("#reader-next");
    await frame.waitForFunction(
      () => document.getElementById("reader-position").textContent === "Page 2 of 2",
    );
    state = await frame.evaluate(() => ({
      position: document.getElementById("reader-position").textContent,
      previousDisabled: document.getElementById("reader-previous").disabled,
      nextDisabled: document.getElementById("reader-next").disabled,
    }));
    assert(!state.previousDisabled && state.nextDisabled, "the last page steps back only", state);

    // --- a comic, in reading order ----------------------------------------
    frame = await openCase(page, "44".repeat(32), port);
    state = await frame.evaluate(() => ({
      kind: document.getElementById("reader-kind").textContent,
      position: document.getElementById("reader-position").textContent,
      width: document.querySelector(".page-sheet")?.naturalWidth || 0,
      source: document.querySelector(".page-sheet")?.getAttribute("src") || "",
    }));
    assert(state.kind === "Comic", "a comic must be named a comic", state);
    assert(state.position === "Page 1 of 2", "a two-page comic opens on its first page", state);
    assert(state.source.startsWith("blob:"), "a comic page is drawn from a source this page minted", state);
    // page2 is three across and page10 is seven: reading order, not file order.
    assert(state.width === 3, "page 2 must come before page 10", state);
    await frame.click("#reader-next");
    await frame.waitForFunction(
      () => document.querySelector(".page-sheet")?.naturalWidth === 7,
      undefined,
      { timeout: 15000 },
    );
    state = await frame.evaluate(() => ({
      position: document.getElementById("reader-position").textContent,
      width: document.querySelector(".page-sheet").naturalWidth,
    }));
    assert(state.position === "Page 2 of 2" && state.width === 7, "the pager must reach page 10", state);

    // --- a book, chapter by chapter ---------------------------------------
    frame = await openCase(page, "55".repeat(32), port);
    state = await frame.evaluate(() => ({
      kind: document.getElementById("reader-kind").textContent,
      position: document.getElementById("reader-position").textContent,
      controlsDirection: document.getElementById("reader-controls").getAttribute("dir"),
    }));
    const bookCalls = calls.filter(
      (call) => call.mintId === "55".repeat(32) && call.operation === "read_viewer",
    );
    assert(state.kind === "Book", "a book must be named a book", state);
    assert(state.position === "Chapter 1 of 3", "a book opens on its first chapter", state);
    assert(
      bookCalls.length === 2 && bookCalls.map((call) => call.chunkIndex).join(",") === "0,1",
      "a book over one part long must be read part by part, in order",
      bookCalls,
    );
    assert(state.controlsDirection === "ltr", "a left-to-right book steps left to right", state);

    let audit = await frame.evaluate(CHAPTER_AUDIT);
    auditChapter(audit, "chapter one");
    assert(audit.text.includes("The first chapter."), "the chapter's own words must survive", audit);
    assert(!audit.tags.includes("script"), "nothing that runs may survive", audit);
    assert(audit.sources.length === 1, "the book's own picture must survive", audit);
    assert(
      audit.links.length === 2 && audit.links.some((link) => link.startsWith("mailto:")),
      "an ordinary link must survive",
      audit,
    );

    // Chapter two: not well-formed, so the engine's markup parser is the one
    // that built the tree the rebuild walked.
    await frame.click("#reader-next");
    await frame.waitForFunction(
      () => document.getElementById("reader-position").textContent === "Chapter 2 of 3",
    );
    audit = await frame.evaluate(CHAPTER_AUDIT);
    auditChapter(audit, "chapter two");
    assert(
      audit.text.includes("Second chapter words."),
      "a chapter only the second parse can read must still reach the page",
      audit,
    );
    assert(
      !audit.tags.includes("svg") && !audit.tags.includes("foreignobject"),
      "a drawing subtree must not survive",
      audit,
    );
    assert(!audit.tags.includes("noscript"), "a no-script block must not survive", audit);
    assert(!audit.text.includes("inside a drawing"), "words inside a drawing must not survive", audit);
    assert(!audit.text.includes("templated"), "words inside a template must not survive", audit);
    assert(audit.text.includes("closing words"), "the chapter's last words must survive", audit);
    // The escaped script link is written `javascript&colon;…`, which only the
    // engine's markup parser turns back into a scheme; there is no link left
    // in this chapter, so none of it survived.
    assert(audit.links.length === 0, "an escaped script link must not survive", audit);
    // The same picture is given two sources. The parser keeps the first and
    // drops the second, and the one that is kept is the book's own file.
    assert(audit.sources.length === 1, "the picture with two sources must keep one", audit);

    // Chapter three: well-formed, so the strict parse built it, keeping the
    // attribute case and the namespaces the rebuild has to refuse.
    await frame.click("#reader-next");
    await frame.waitForFunction(
      () => document.getElementById("reader-position").textContent === "Chapter 3 of 3",
    );
    audit = await frame.evaluate(CHAPTER_AUDIT);
    auditChapter(audit, "chapter three");
    assert(audit.text.includes("Third chapter words."), "the third chapter must reach the page", audit);
    assert(
      audit.links.length === 1 && audit.links[0] === "https://example.org/ok",
      "only the link that leaves the book by a readable address survives",
      audit,
    );
    assert(audit.sources.length === 0, "a picture from somewhere else must not survive", audit);
    assert(
      audit.text.includes("drawing namespace paragraph"),
      "words in an element the rebuild unwraps must survive",
      audit,
    );

    // --- a book that reads the other way ----------------------------------
    frame = await openCase(page, "88".repeat(32), port);
    state = await frame.evaluate(() => ({
      controlsDirection: document.getElementById("reader-controls").getAttribute("dir"),
      articleDirection: document.querySelector(".book-sheet")?.getAttribute("dir"),
      computed: getComputedStyle(document.querySelector(".book-sheet")).direction,
    }));
    assert(
      state.articleDirection === "rtl" && state.computed === "rtl",
      "a book that reads right to left must be laid out that way",
      state,
    );
    assert(state.controlsDirection === "rtl", "its steps must turn with it", state);

    // --- a 3D model -------------------------------------------------------
    // The 3D library is named, not addressed, in the code that loads it, so
    // this case is also what proves the page's one inline name map is admitted
    // by its digest and that a drawing surface is available at this origin.
    frame = await openCase(page, "99".repeat(32), port);
    state = await frame.evaluate(() => {
      const surface = document.querySelector("canvas.model-sheet");
      return {
        kind: document.getElementById("reader-kind").textContent,
        hasSurface: Boolean(surface),
        width: surface?.width || 0,
        height: surface?.height || 0,
        controlsHidden: document.getElementById("reader-controls").hidden,
      };
    });
    assert(state.kind === "3D model", "a model must be named a model", state);
    assert(state.hasSurface, "a model must get a drawing surface", state);
    assert(state.width > 0 && state.height > 0, "the drawing surface must have a real size", state);
    assert(state.controlsHidden, "a model has no pages to step through", state);

    // --- a file with no renderer ------------------------------------------
    frame = await openCase(page, "66".repeat(32), port);
    state = await frame.evaluate(() => ({
      kind: document.getElementById("reader-kind").textContent,
      lines: Array.from(document.querySelectorAll(".view-empty p")).map((line) => line.textContent),
      controlsHidden: document.getElementById("reader-controls").hidden,
    }));
    assert(state.kind === "File", "a file with no renderer is named plainly", state);
    assert(
      state.lines.some((line) => line.includes("no way to show this file")) &&
        state.lines.some((line) => line.includes("application/x-unknown")),
      "the empty state must say what it cannot show, and name it",
      state,
    );
    assert(state.controlsHidden, "there is nothing to step through", state);

    // --- a session that is not a file session -----------------------------
    frame = await openCase(page, "77".repeat(32), port);
    state = await frame.evaluate(() => ({
      status: document.getElementById("reader-status").textContent,
      lines: Array.from(document.querySelectorAll(".view-empty p")).map((line) => line.textContent),
    }));
    assert(
      state.status === "This file plays in Elacity Player." &&
        state.lines.includes("This file plays in Elacity Player."),
      "a session that is not a file session must be refused by name",
      state,
    );
    const wrongKindCalls = calls.filter((call) => call.mintId === "77".repeat(32));
    assert(
      wrongKindCalls.some((call) => call.operation === "close_viewer"),
      "a session this reader refuses must still be handed back",
      wrongKindCalls,
    );
    assert(
      !wrongKindCalls.some((call) => call.operation === "read_viewer"),
      "a session this reader refuses must not be read from",
      wrongKindCalls,
    );

    // --- how a background thread is refused at this origin ------------------
    const workers = await frame.evaluate(async () => {
      const answer = {};
      try {
        const worker = new Worker("./vendor/pdfjs/pdf.worker.min.mjs?v=probe", { type: "module" });
        worker.terminate();
        answer.fromAFileAddress = "started";
      } catch (error) {
        answer.fromAFileAddress = `refused: ${error.name}`;
      }
      try {
        const url = URL.createObjectURL(
          new Blob(["self.postMessage('ready')"], { type: "text/javascript" }),
        );
        const worker = new Worker(url, { type: "module" });
        answer.fromAMintedAddress = await new Promise((settle) => {
          worker.onmessage = () => settle("started");
          worker.onerror = () => settle("refused: error");
          setTimeout(() => settle("refused: silence"), 2000);
        });
        worker.terminate();
      } catch (error) {
        answer.fromAMintedAddress = `refused: ${error.name}`;
      }
      return answer;
    });
    assert(
      workers.fromAFileAddress === "refused: SecurityError",
      "a background thread from a file address must stay refused at this origin",
      workers,
    );
    assert(
      workers.fromAMintedAddress.startsWith("refused"),
      "a background thread from a minted address must stay refused at this origin",
      workers,
    );

    // --- both shapes a parse failure comes in -------------------------------
    const parseFailures = await frame.evaluate(async () => {
      const { parseContainer } = await import("./epub.js");
      const answer = {};
      // The shape this engine produces: an error element inside the document.
      const broken = '<?xml version="1.0"?><container><rootfiles></container>';
      answer.engineShape = new DOMParser()
        .parseFromString(broken, "application/xml")
        .getElementsByTagName("parsererror").length;
      try {
        parseContainer(broken, (markup, type) => new DOMParser().parseFromString(markup, type));
        answer.engineDetected = false;
      } catch (error) {
        answer.engineDetected = /damaged/i.test(error.message);
      }
      // The other shape a parser can report it in: the error as the whole
      // document, under a namespace of the parser's own.
      const other = document.implementation.createDocument(
        "http://www.mozilla.org/newlayout/xml/parsererror.xml",
        "parsererror",
        null,
      );
      other.documentElement.appendChild(other.createTextNode("XML Parsing Error"));
      answer.otherShapeRoot = other.documentElement.localName;
      try {
        parseContainer("<container/>", () => other);
        answer.otherDetected = false;
      } catch (error) {
        answer.otherDetected = /damaged/i.test(error.message);
      }
      return answer;
    });
    assert(parseFailures.engineShape > 0, "this engine reports a parse failure inside the document", parseFailures);
    assert(parseFailures.engineDetected, "a parse failure this engine reports must be caught", parseFailures);
    assert(
      parseFailures.otherShapeRoot === "parsererror" && parseFailures.otherDetected,
      "a parse failure reported as the whole document must be caught too",
      parseFailures,
    );

    // --- nothing went wrong along the way ----------------------------------
    assert(dialogs.length === 0, "the reader must never put a dialog on the screen", dialogs);
    assert(pageErrors.length === 0, "the reader must run without a page error", pageErrors);
    for (const refusal of REFUSALS) {
      assert(
        consoleErrors.some((text) => refusal.match(text)),
        `the page's own rules must still be refusing ${refusal.name}`,
        consoleErrors,
      );
    }
    const unexpected = consoleErrors.filter((text) => !REFUSALS.some((refusal) => refusal.match(text)));
    assert(unexpected.length === 0, "the reader must run without a console error of its own", unexpected);
    assert(notFound.length === 0, "the reader must not ask for anything that is not there", notFound);
    assert(
      calls.every((call) => call.token === HOME_TOKEN),
      "every call must carry the token the page was launched with",
      calls.filter((call) => call.token !== HOME_TOKEN),
    );
    console.log(`elacity-reader-smoke: OK (${calls.length} provider calls across ${FIXTURES.size} files)`);
  } finally {
    await browser.close().catch(() => {});
    await new Promise((done) => server.close(() => done()));
  }
}

run().catch((error) => {
  console.error(error.stack || String(error));
  process.exitCode = 1;
});
