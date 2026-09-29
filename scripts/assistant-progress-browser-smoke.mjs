#!/usr/bin/env node
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { once } from "node:events";
import { readFile, mkdtemp, mkdir, writeFile } from "node:fs/promises";
import { resolve, extname, join } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { chromium, brave } from "./system-uiux-fixture.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const errors = [];
const server = createServer(async (req, res) => {
  try {
    const pathname = new URL(req.url, "http://fixture").pathname;
    if (pathname === "/favicon.ico") { res.writeHead(204); res.end(); return; }
    if (pathname === "/") {
      res.setHeader("content-type", "text/html; charset=utf-8");
      res.end(`<!doctype html><html><head><link rel="stylesheet" href="/apps/assistant/agent-harness.css">
        <style>:root{--text:#222;--prose-faint:#666;--font-ui:Arial;--text-small-size:14px;--text-small-line:21px}body{padding:24px}#stream{width:600px}</style></head>
        <body><div id="stream"></div><script type="module">
        import { bindAgentStream, appendProgressBlock, paintProgressView } from '/apps/assistant/agent-stream.js';
        bindAgentStream({}, { streamEl: () => document.querySelector('#stream'), clearEmptyState() {} });
        const block = appendProgressBlock();
        window.paint = (progress, options) => paintProgressView(block, progress, options);
        window.fixtureReady = true;
        </script></body></html>`);
      return;
    }
    const match = pathname.match(/^\/apps\/([^/]+)\/(.+)$/);
    assert.ok(match, `Unexpected fixture request: ${pathname}`);
    const capsuleRoot = resolve(root, "capsules", match[1], "browser");
    const path = resolve(capsuleRoot, match[2]);
    assert.ok(path.startsWith(`${capsuleRoot}/`));
    const bytes = await readFile(path);
    res.setHeader("content-type", ({ ".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css" })[extname(path)] || "application/octet-stream");
    res.end(bytes);
  } catch (error) {
    errors.push(String(error));
    res.writeHead(500); res.end("Fixture asset error");
  }
});
server.listen(0, "127.0.0.1");
await once(server, "listening");
let browser, page;
try {
  browser = await chromium.launch({ executablePath: brave, headless: true });
  page = await browser.newPage({ viewport: { width: 900, height: 700 } });
  page.on("pageerror", error => errors.push(String(error)));
  await page.goto(`http://127.0.0.1:${server.address().port}/`);
  await page.waitForFunction(() => window.fixtureReady === true);
  const block = page.locator('.agent-progress');
  const summary = block.locator('summary');
  const rows = block.locator('.agent-progress-milestones li');
  const raw = block.locator('.agent-progress-raw');
  const label = "Analyzing the findings…";
  const current = { key: "current", text: label, phase: "analyzing" };
  let progress = { revealed: true, phase: "analyzing", currentText: label, current, milestones: [] };
  const paint = (options = {}) => page.evaluate(({ progress, options }) => window.paint(progress, options), { progress, options });
  const mouseClickSummary = async () => {
    const box = await summary.boundingBox();
    assert.ok(box, "Current status is rendered");
    await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
  };

  await paint();
  assert.equal(await summary.innerText(), label);
  assert.equal(await rows.count(), 0, "Current activity is not repeated in the trace");
  assert.equal(await summary.getAttribute('aria-disabled'), 'true');
  assert.equal(await block.locator('.agent-thinking-chevron').isVisible(), false);
  await mouseClickSummary();
  assert.equal(await block.evaluate(el => el.open), false, "A status alone has no disclosure");
  assert.equal(await raw.isVisible(), false, "Generic activity is not presented as reasoning");

  // An identical historical label also gives the disclosure nothing new to show.
  progress.milestones = [{ key: "same", text: label, phase: "analyzing" }];
  await paint();
  assert.equal(await rows.count(), 0);
  assert.equal(await block.getAttribute('data-disclosure'), 'false');

  progress.milestones.push({ key: "read", text: "Read the source file", phase: "reading" });
  await paint();
  await summary.click();
  assert.equal(await block.evaluate(el => el.open), true);
  assert.deepEqual(await rows.allTextContents(), ["Read the source file"]);
  assert.equal((await block.innerText()).split(label).length - 1, 1, "Expanded activity shows the current label once");
  // A corrected milestone with the same key must repaint, without losing open state.
  progress.milestones[1].text = "Read both source files";
  await paint();
  assert.deepEqual(await rows.allTextContents(), ["Read both source files"]);
  assert.equal(await block.evaluate(el => el.open), true);

  progress.milestones = [];
  const reasoning = 'Compare α and β.\n<script>literal, not markup</script> 🧪';
  await paint({ reasoning, reasoningVisible: true });
  assert.equal(await block.getAttribute('data-disclosure'), 'true');
  assert.equal(await raw.isVisible(), true);
  assert.equal(await raw.textContent(), reasoning);
  assert.equal(await raw.locator('script').count(), 0, "Model reasoning stays plain text");
  await paint({ reasoning: `${reasoning}\nThen check the result.`, reasoningVisible: true });
  assert.equal(await raw.textContent(), `${reasoning}\nThen check the result.`, "Further reasoning deltas repaint at the same phase");
  await paint({ reasoning, reasoningVisible: false });
  assert.equal(await block.evaluate(el => el.open), false);
  assert.equal(await raw.isVisible(), false);
  assert.equal(await block.getAttribute('data-disclosure'), 'false', "Hidden reasoning cannot enable an empty disclosure");
  await paint({ reasoning: ' \n ', reasoningVisible: true });
  assert.equal(await block.getAttribute('data-disclosure'), 'false');

  for (const phase of ['done', 'stopped', 'error']) {
    progress = { revealed: true, phase, secondary: true, current: null, currentText: phase === 'done' ? '✓ 2 steps' : phase,
      milestones: [{ key: 'read', text: 'Read both source files', phase: 'reading' },
        { key: 'finding', text: 'The result matches', kind: 'finding', phase: 'verifying' }] };
    await paint();
    if (!(await block.evaluate(el => el.open))) await summary.click();
    assert.deepEqual(await rows.allTextContents(), ['Read both source files', 'The result matches']);
    assert.equal(await block.locator('.is-finding').count(), 1);
    assert.equal(await block.evaluate(el => el.classList.contains('is-streaming')), false);
    assert.equal(await block.evaluate(el => el.classList.contains('is-complete')), true);
    assert.equal(await summary.innerText(), progress.currentText);
  }
  assert.deepEqual(errors, []);
  console.log('PASS Assistant rendered progress: single status, distinct history, raw reasoning, terminal milestones');
} catch (error) {
  const proofDir = process.env.ASSISTANT_PROGRESS_PROOF_DIR || await mkdtemp(join(tmpdir(), 'assistant-progress-failure-'));
  await mkdir(proofDir, { recursive: true });
  await writeFile(join(proofDir, 'report.json'), JSON.stringify({ error: String(error), errors }, null, 2));
  if (page) await page.screenshot({ path: join(proofDir, 'first-failure.png'), fullPage: true }).catch(() => {});
  console.error(`Assistant progress failure evidence: ${proofDir}`);
  throw error;
} finally {
  await browser?.close();
  server.close();
  await once(server, 'close');
}
