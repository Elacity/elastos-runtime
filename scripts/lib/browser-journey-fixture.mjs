#!/usr/bin/env node
// Parent-owned fixture: node scripts/lib/browser-journey-fixture.mjs [--port 61511]
// Authorize only localhost:<port> in the test Runtime's allowed_private_targets.
import http from "node:http";
import { pathToFileURL } from "node:url";

const MAX_RUNS = 16;
const MAX_EVENTS = 128;
const TTL_MS = 10 * 60_000;
const RUN_ID = /^[a-zA-Z0-9_-]{8,64}$/;

function fixturePage(page, run, media = false) {
  return `<!doctype html><html lang="en"><meta charset="utf-8">
<title>Browser journey ${page}</title>
<style>
  body { margin: 0; font: 24px system-ui; color: #13243b; background: #edf5ff; }
  main { padding: 32px; min-height: 3600px; background: linear-gradient(#edf5ff, #76acd8); }
  input { display: block; width: 480px; max-width: 80vw; margin: 12px 0; padding: 12px; font: inherit; }
  #journey-observation { position: fixed; bottom: 16px; left: 24px; background: white; padding: 12px; }
  #journey-motion { width: 48px; height: 24px; background: #d74b28; animation: motion 1s linear infinite alternate; }
  @keyframes motion { to { transform: translateX(200px); } }
</style>
<main>
  <h1>Browser journey ${page}</h1>
  <label for="journey-input">Test text</label>
  <input id="journey-input" maxlength="96" autocomplete="off">
  <a id="journey-next" href="/nav?run=${run}">Open navigation page</a>
  <div id="journey-motion" aria-label="Continuous test motion"></div>
  ${media ? "<p>Click the text field to start a quiet 440 Hz audio test tone.</p>" : ""}
  <p>Scroll down to move this page.</p>
  <p style="margin-top: 2400px">End of controlled scroll content</p>
</main>
<output id="journey-observation"></output>
<script>
  const run = ${JSON.stringify(run)};
  const page = ${JSON.stringify(page)};
  const input = document.querySelector('#journey-input');
  let toneContext = null;
  if (${media}) input.addEventListener('pointerdown', async () => {
    toneContext = new AudioContext();
    const tone = toneContext.createOscillator();
    const gain = toneContext.createGain();
    tone.frequency.value = 440;
    gain.gain.value = 0.05;
    tone.connect(gain).connect(toneContext.destination);
    tone.start();
    await toneContext.resume();
    report('audio');
  }, { once: true });
  let sent = 0;
  let pending = Promise.resolve();
  function report(type) {
    if (sent >= ${MAX_EVENTS}) return;
    sent++;
    const rect = input.getBoundingClientRect();
    const event = { type, page, value: input.value, scroll_x: scrollX, scroll_y: scrollY,
      ...(type === "audio" ? { audio_state: toneContext.state, frequency_hz: 440 } : {}),
      input_rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height } };
    document.querySelector('#journey-observation').textContent =
      'Text: ' + input.value + ' | Scroll: ' + Math.round(scrollY);
    pending = pending.then(async () => {
      const response = await fetch('/events?run=' + run, {
        method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(event)
      });
      if (!response.ok) throw new Error('Fixture event rejected: ' + response.status);
    }).catch(error => { document.querySelector('#journey-observation').textContent = error.message; });
  }
  input.addEventListener('input', () => report('input'));
  let scrollPending = false;
  addEventListener('scroll', () => {
    if (scrollPending) return;
    scrollPending = true;
    requestAnimationFrame(() => { scrollPending = false; report('scroll'); });
  });
  report('load');
</script></html>`;
}

export function createBrowserJourneyFixture() {
  const runs = new Map();
  return http.createServer({ requestTimeout: 5_000, headersTimeout: 5_000 }, async (req, res) => {
    const json = (status, body) => {
      res.writeHead(status, { "content-type": "application/json", "cache-control": "no-store" });
      res.end(JSON.stringify(body));
    };
    try {
      const url = new URL(req.url, "http://localhost");
      if (req.method === "GET" && url.pathname === "/health") {
        json(200, { schema: "elastos.browser.journey-fixture/v1", ok: true });
        return;
      }
      const run = url.searchParams.get("run") || "";
      if (!RUN_ID.test(run)) { json(400, { error: "invalid run id" }); return; }
      for (const [id, record] of runs) {
        if (Date.now() - record.created_at >= TTL_MS) runs.delete(id);
      }
      if (req.method === "GET" && ["/main", "/nav"].includes(url.pathname)) {
        if (!runs.has(run)) {
          if (runs.size >= MAX_RUNS) { json(429, { error: "fixture run capacity" }); return; }
          runs.set(run, { created_at: Date.now(), events: [] });
        }
        res.writeHead(200, { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" });
        res.end(fixturePage(url.pathname.slice(1), run, url.searchParams.get("media") === "1"));
        return;
      }
      const record = runs.get(run);
      if (!record) { json(404, { error: "unknown run" }); return; }
      if (req.method === "GET" && url.pathname === "/receipt") {
        json(200, { schema: "elastos.browser.journey-receipt/v1", run, events: record.events });
        return;
      }
      if (req.method !== "POST" || url.pathname !== "/events") {
        json(404, { error: "unknown route" }); return;
      }
      const chunks = [];
      let bytes = 0;
      for await (const chunk of req) {
        bytes += chunk.length;
        if (bytes > 4096) { json(413, { error: "event too large" }); return; }
        chunks.push(chunk);
      }
      const event = JSON.parse(Buffer.concat(chunks).toString("utf8"));
      const rect = event?.input_rect;
      if (!event || !["load", "input", "scroll", "audio"].includes(event.type) ||
          !["main", "nav"].includes(event.page) || typeof event.value !== "string" ||
          event.value.length > 96 || !rect ||
          ![event.scroll_x, event.scroll_y, rect.x, rect.y, rect.width, rect.height]
            .every(value => Number.isFinite(value) && Math.abs(value) <= 100_000)) {
        json(400, { error: "invalid event" }); return;
      }
      if (event.type === "audio" && (event.audio_state !== "running" || event.frequency_hz !== 440)) {
        json(400, { error: "invalid audio event" }); return;
      }
      if (record.events.length >= MAX_EVENTS) { json(429, { error: "fixture event capacity" }); return; }
      record.events.push({ sequence: record.events.length + 1, received_at: Date.now(),
        type: event.type, page: event.page, value: event.value,
        ...(event.type === "audio" ? { audio_state: event.audio_state, frequency_hz: event.frequency_hz } : {}),
        scroll_x: event.scroll_x, scroll_y: event.scroll_y,
        input_rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height } });
      json(200, { ok: true });
    } catch {
      if (!res.headersSent) json(400, { error: "invalid fixture request" });
    }
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const args = process.argv.slice(2);
  const port = args.length === 0 ? 61511 : args.length === 2 && args[0] === "--port" ? Number(args[1]) : NaN;
  if (!Number.isInteger(port) || port < 1024 || port > 65535) {
    throw new Error("Usage: node scripts/lib/browser-journey-fixture.mjs [--port 61511]");
  }
  const server = createBrowserJourneyFixture();
  server.listen(port, "127.0.0.1", () => console.log(JSON.stringify({
    schema: "elastos.browser.journey-fixture/v1", origin: `http://localhost:${port}`,
    retention: { max_runs: MAX_RUNS, max_events_per_run: MAX_EVENTS, ttl_ms: TTL_MS },
  })));
  for (const signal of ["SIGINT", "SIGTERM"]) process.once(signal, () => {
    server.close();
    server.closeAllConnections();
  });
}
