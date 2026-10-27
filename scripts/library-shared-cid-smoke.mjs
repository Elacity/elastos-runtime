// Browser regression: shared downloads survive invalid initial URIs and folder failures.
// Provider list mirrors library.rs clean_library_uri: non-localhost:// URIs are an error.
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";
const repo = fileURLToPath(new URL("../", import.meta.url));
const { chromium } = await import(process.env.ELASTOS_PLAYWRIGHT_MODULE);
const capsuleRoot = path.join(repo, "capsules/library/browser");
const principalRoot = "localhost://Users/repro";
const documentsUri = `${principalRoot}/Documents`;
const roots = [
  { schema: "elastos.library.root/v1", id: "home", label: "Home", uri: principalRoot, kind: "principal-root" },
  { schema: "elastos.library.root/v1", id: "documents", label: "Documents", uri: documentsUri, kind: "directory" },
];
const listed = [];
let failFolder = false;
const server = createServer(async (req, res) => {
  const url = new URL(req.url, "http://127.0.0.1");
  if (url.pathname === "/api/provider/object/events/stream") {
    res.writeHead(200, { "content-type": "text/event-stream" }); res.write("\n"); return;
  }
  const m = url.pathname.match(/^\/api\/provider\/object\/([^/]+)$/);
  if (m) {
    let body = ""; for await (const c of req) body += c;
    const payload = body ? JSON.parse(body) : {};
    res.writeHead(200, { "content-type": "application/json" });
    if (m[1] === "roots") return res.end(JSON.stringify({ status: "ok", data: { roots } }));
    if (m[1] === "list") {
      listed.push(payload.uri);
      if (failFolder || !String(payload.uri || "").startsWith("localhost://")) {
        return res.end(JSON.stringify({ status: "error", code: "library_error", message: "library object URI must be localhost://" }));
      }
      return res.end(JSON.stringify({ status: "ok", data: { uri: payload.uri, objects: [], object: { schema: "elastos.library.object/v1", uri: payload.uri, name: "Documents", kind: "directory", mime: "inode/directory", size: 0, revision: "r", metadata: {}, capabilities: ["open"] } } }));
    }
    return res.end(JSON.stringify({ status: "ok", data: {} }));
  }
  const homeFile = url.pathname.match(/^\/apps\/home\/([a-z-]+\.js)$/);
  const file = homeFile ? path.join(repo, "capsules/home/browser", homeFile[1])
    : path.join(capsuleRoot, url.pathname.replace(/^\/apps\/library\/?/, "") || "index.html");
  try {
    if (!(await stat(file)).isFile()) throw 0;
    res.writeHead(200, { "content-type": file.endsWith(".js") ? "text/javascript" : file.endsWith(".css") ? "text/css" : "text/html" });
    createReadStream(file).pipe(res);
  } catch { res.writeHead(404); res.end(); }
});
await new Promise(r => server.listen(0, "127.0.0.1", r));
const port = server.address().port;
const browser = await chromium.launch({ ...(process.env.ELASTOS_BROWSER_EXECUTABLE ? { executablePath: process.env.ELASTOS_BROWSER_EXECUTABLE } : {}), headless: true });
const cid = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3w2pz3hftb5xg3mh5g4xq7y5u";
try {
for (const [label, query] of [["legacy query {cid,uri}", `cid=${cid}&uri=${encodeURIComponent(`elastos://${cid}`)}`], ["cid only", `cid=${cid}`], ["folder failure", `cid=${cid}`], ["sender filename ignored", `cid=${cid}&fileName=report.pdf`]]) {
  listed.length = 0;
  failFolder = label === "folder failure";
  const page = await browser.newPage();
  await page.goto(`http://127.0.0.1:${port}/apps/library/?${query}&home_origin=${encodeURIComponent("https://home.example")}#home_token=t`);
  await page.locator("#dialog:not(.hidden) a[download]").waitFor();
  assert.equal(await page.locator("#dialog a[download]").getAttribute("href"), `/content/${cid}`);
  assert.equal(await page.locator("#dialog a[download]").getAttribute("download"), cid);
  assert.ok(listed.length > 0);
  assert.ok(listed.every(uri => uri.startsWith("localhost://")));
  const r = await page.evaluate(() => ({ content: document.getElementById("content")?.textContent.trim().slice(0, 80), dialog: document.getElementById("dialog")?.classList.contains("hidden") ? "(hidden)" : document.getElementById("dialog")?.textContent.replace(/\s+/g, " ").trim().slice(0, 120) }));
  console.log(`${label}: list=${JSON.stringify(listed)} content=${JSON.stringify(r.content)} dialog=${JSON.stringify(r.dialog)}`);
  await page.close();
}
} finally {
  await browser.close();
  server.closeAllConnections();
  await new Promise(resolve => server.close(resolve));
}
