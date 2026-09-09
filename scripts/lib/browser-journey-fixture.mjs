#!/usr/bin/env node
// Parent-owned fixture: node scripts/lib/browser-journey-fixture.mjs [--port 61511]
// Authorize only localhost:<port> in the test Runtime's allowed_private_targets.
import http from "node:http";
import { pathToFileURL } from "node:url";

const MAX_RUNS = 16;
const MAX_EVENTS = 128;
const TTL_MS = 10 * 60_000;
const RUN_ID = /^[a-zA-Z0-9_-]{8,64}$/;

// Serialized into the opt-in page. Only this fixture's fixed storage keys are touched.
export async function browserProfileStorageProbe({ mode, marker }) {
  if (!["write", "read"].includes(mode) || !/^[a-zA-Z0-9_-]{8,64}$/.test(marker)) throw new Error("invalid profile probe");
  const evidence = { schema: "elastos.browser.profile-storage/v1", mode, marker, measurement_ok: false, ok: false,
    cookie: { present: false, matches: false }, local_storage: { present: false, matches: false },
    indexed_db: { write_request_succeeded: false, write_committed: false,
      read_request_succeeded: false, read_completed: false, present: false, matches: false }, error: null };
  const key = "elastos_browser_profile_probe", database = "elastos-browser-profile-probe-v1";
  const observed = value => ({ present: value !== null && value !== undefined, matches: value === marker });
  let db;
  try {
    if (mode === "write") {
      document.cookie = key + "=" + marker + "; Path=/; SameSite=Strict; Max-Age=86400";
      localStorage.setItem(key, marker);
    }
    const cookie = document.cookie.split(";").map(s => s.trim()).find(s => s.startsWith(key + "="));
    evidence.cookie = observed(cookie ? cookie.slice(key.length + 1) : null);
    evidence.local_storage = observed(localStorage.getItem(key));
    db = await new Promise((resolve, reject) => {
      const request = indexedDB.open(database, 1);
      let expired = false;
      const timer = setTimeout(() => { expired = true; reject(new Error("storage_timeout")); }, 5000);
      request.onupgradeneeded = () => {
        // Opening an absent database in read mode must not create probe state.
        if (expired || mode === "read") { request.transaction.abort(); return; }
        request.result.createObjectStore("markers");
      };
      request.onsuccess = () => {
        clearTimeout(timer);
        if (expired) { request.result.close(); return; }
        resolve(request.result);
      };
      request.onerror = () => { clearTimeout(timer); reject(new Error("database_open_failed")); };
    });
    const transaction = write => new Promise((resolve, reject) => {
      const tx = db.transaction("markers", write ? "readwrite" : "readonly");
      const store = tx.objectStore("markers");
      const request = write ? store.put(marker, "value") : store.get("value");
      let expired = false;
      const timer = setTimeout(() => {
        expired = true;
        try { tx.abort(); } catch {}
        reject(new Error("storage_timeout"));
      }, 5000);
      request.onsuccess = () => {
        if (expired) return;
        if (write) evidence.indexed_db.write_request_succeeded = true;
        else {
          evidence.indexed_db.read_request_succeeded = true;
          Object.assign(evidence.indexed_db, observed(request.result));
        }
      };
      tx.oncomplete = () => {
        clearTimeout(timer); if (expired) return;
        evidence.indexed_db[write ? "write_committed" : "read_completed"] = true;
        resolve();
      };
      tx.onabort = () => { clearTimeout(timer); reject(new Error("transaction_aborted")); };
      tx.onerror = () => { /* The transaction abort is the terminal failure witness. */ };
    });
    if (mode === "write") await transaction(true);
    await transaction(false);
    evidence.measurement_ok = true;
    evidence.ok = evidence.cookie.matches && evidence.local_storage.matches && evidence.indexed_db.matches &&
      evidence.indexed_db.read_request_succeeded && evidence.indexed_db.read_completed &&
      (mode === "read" || evidence.indexed_db.write_request_succeeded && evidence.indexed_db.write_committed);
  } catch (error) {
    evidence.error = ["storage_timeout", "database_open_failed", "transaction_aborted"].includes(error.message) ?
      error.message : "storage_unavailable";
  } finally { db?.close(); }
  return evidence;
}

// Serialized into the opt-in page; only the selected File's bytes are read.
export async function browserFileInputProbe(input, { expected_sha256 }) {
  if (!/^[a-f0-9]{64}$/.test(expected_sha256)) throw new Error("invalid file probe");
  const started = performance.now(), deadline = started + 5000;
  const result = { schema: "elastos.browser.file-input/v1", expected_sha256, size_bytes: null, sha256: null,
    read_completed: false, hash_completed: false, measurement_ok: false, matches: false, ok: false, error: null, elapsed_ms: 0 };
  let stopped = false, timer;
  const current = () => { if (stopped || performance.now() >= deadline) throw new Error("file_timeout"); };
  try {
    const files = input.files;
    if (!files || files.length !== 1) throw new Error("invalid_selection");
    const file = files[0];
    if (Number.isSafeInteger(file.size) && file.size >= 0) result.size_bytes = file.size;
    if (result.size_bytes !== 65536) throw new Error("invalid_size");
    if (globalThis.isSecureContext !== true || !globalThis.crypto?.subtle) throw new Error("hash_unavailable");
    await Promise.race([
      (async () => {
        const bytes = await file.arrayBuffer().catch(() => { throw new Error("read_failed"); });
        current();
        result.read_completed = true;
        if (bytes.byteLength !== 65536) throw new Error("invalid_size");
        const digest = await crypto.subtle.digest("SHA-256", bytes).catch(() => { throw new Error("hash_failed"); });
        current();
        result.sha256 = Array.from(new Uint8Array(digest), byte => byte.toString(16).padStart(2, "0")).join("");
        result.hash_completed = result.measurement_ok = true;
        result.matches = result.ok = result.sha256 === expected_sha256;
      })(),
      new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("file_timeout")), 5000); }),
    ]);
  } catch (error) {
    result.error = ["invalid_selection", "invalid_size", "hash_unavailable", "read_failed", "hash_failed", "file_timeout"]
      .includes(error.message) ? error.message : "read_failed";
  } finally {
    stopped = true;
    clearTimeout(timer);
    result.elapsed_ms = Math.max(0, Math.floor(performance.now() - started));
  }
  return result;
}

function fixturePage(page, run, media = false, qualification = false, profile = null, fileProbe = null) {
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
  <a id="journey-next" href="/nav?run=${run}${profile ? `&profile=${profile.mode}&marker=${profile.marker}` : ""}${fileProbe ? `&file=upload&sha256=${fileProbe.expected_sha256}` : ""}">Open navigation page</a>
  ${fileProbe ? '<label for="journey-file">Select the 64KiB test file from Library</label><input id="journey-file" type="file">' : ""}
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
  function report(type, profileStorage, fileInput) {
    if (sent >= ${qualification ? 8192 : MAX_EVENTS}) return;
    sent++;
    const rect = input.getBoundingClientRect();
    const event = { type, page, value: input.value, scroll_x: scrollX, scroll_y: scrollY,
      ...(type === "profile_storage" ? { profile_storage: profileStorage } : {}),
      ...(type === "file_input" ? { file_input: fileInput } : {}),
      ...(type === "audio" ? { audio_state: toneContext.state, frequency_hz: 440 } : {}),
      input_rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height } };
    document.querySelector('#journey-observation').textContent = type === "profile_storage" ?
      'Profile ' + profileStorage.mode + ': ' + (profileStorage.ok ? 'marker matches' : 'incomplete') +
        ' | IndexedDB write committed: ' + profileStorage.indexed_db.write_committed :
      type === "file_input" ? 'File SHA-256: ' + (fileInput.ok ? 'matches' : 'incomplete or different') :
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
  ${profile ? `(${browserProfileStorageProbe.toString()})(${JSON.stringify(profile)}).then(value => report('profile_storage', value));` : ""}
  ${fileProbe ? `const fileInput = document.querySelector('#journey-file');
  let fileProbeStarted = false;
  const measureFile = () => {
    if (fileProbeStarted) return;
    fileProbeStarted = true;
    fileInput.removeEventListener('input', measureFile);
    fileInput.removeEventListener('change', measureFile);
    (${browserFileInputProbe.toString()})(fileInput, ${JSON.stringify(fileProbe)}).then(value => report('file_input', null, value));
  };
  fileInput.addEventListener('input', measureFile);
  fileInput.addEventListener('change', measureFile);` : ""}
</script></html>`;
}

export function createBrowserJourneyFixture({ now = Date.now } = {}) {
  const runs = new Map();
  return http.createServer({ requestTimeout: 5_000, headersTimeout: 5_000 }, async (req, res) => {
    const json = (status, body) => {
      res.writeHead(status, { "content-type": "application/json", "cache-control": "no-store" });
      res.end(JSON.stringify(body));
    };
    try {
      const url = new URL(req.url, "http://localhost");
      if (req.method === "GET" && url.pathname === "/health") {
        json(200, { schema: "elastos.browser.journey-fixture/v1", ok: true, qualification: "bounded-v1" });
        return;
      }
      const run = url.searchParams.get("run") || "";
      if (!RUN_ID.test(run)) { json(400, { error: "invalid run id" }); return; }
      for (const [id, record] of runs) {
        if (now() - (record.qualification ? record.last_seen : record.created_at) >= TTL_MS) runs.delete(id);
      }
      if (req.method === "GET" && ["/main", "/nav"].includes(url.pathname)) {
        const mode = url.searchParams.get("profile"), marker = url.searchParams.get("marker");
        const profile = mode === null && marker === null ? null : { mode, marker };
        if (profile && (!["write", "read"].includes(mode) || !RUN_ID.test(marker || "") ||
            url.searchParams.getAll("profile").length !== 1 || url.searchParams.getAll("marker").length !== 1)) {
          json(400, { error: "invalid profile probe" }); return;
        }
        const fileMode = url.searchParams.get("file"), expected = url.searchParams.get("sha256");
        const fileProbe = fileMode === null && expected === null ? null : { mode: fileMode, expected_sha256: expected };
        if (fileProbe && (fileMode !== "upload" || !/^[a-f0-9]{64}$/.test(expected || "") ||
            url.searchParams.getAll("file").length !== 1 || url.searchParams.getAll("sha256").length !== 1)) {
          json(400, { error: "invalid file probe" }); return;
        }
        if (!runs.has(run)) {
          if (runs.size >= MAX_RUNS) { json(429, { error: "fixture run capacity" }); return; }
          runs.set(run, { created_at: now(), last_seen: now(), events: [], sequence: 0,
            qualification: url.searchParams.get("qualification") === "1", profile, fileProbe });
        }
        const record = runs.get(run);
        if (record.qualification !== (url.searchParams.get("qualification") === "1")) {
          json(409, { error: "fixture observation contract changed" }); return;
        }
        if (JSON.stringify(record.profile) !== JSON.stringify(profile)) {
          json(409, { error: "fixture profile contract changed" }); return;
        }
        if (JSON.stringify(record.fileProbe) !== JSON.stringify(fileProbe)) {
          json(409, { error: "fixture file contract changed" }); return;
        }
        record.last_seen = now();
        res.writeHead(200, { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" });
        res.end(fixturePage(url.pathname.slice(1), run, url.searchParams.get("media") === "1", record.qualification, profile, fileProbe));
        return;
      }
      const record = runs.get(run);
      if (!record) { json(404, { error: "unknown run" }); return; }
      record.last_seen = now();
      if (req.method === "GET" && url.pathname === "/receipt") {
        json(200, { schema: "elastos.browser.journey-receipt/v1", run, events: record.events,
          ...(record.profile ? { profile_probe: record.profile } : {}),
          ...(record.fileProbe ? { file_probe: record.fileProbe } : {}),
          ...(record.qualification ? { observation: "bounded-v1", total_events: record.sequence,
            dropped_events: record.sequence - record.events.length } : {}) });
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
      if (!event || !["load", "input", "scroll", "audio", "profile_storage", "file_input"].includes(event.type) ||
          !["main", "nav"].includes(event.page) || typeof event.value !== "string" ||
          event.value.length > 96 || !rect ||
          ![event.scroll_x, event.scroll_y, rect.x, rect.y, rect.width, rect.height]
            .every(value => Number.isFinite(value) && Math.abs(value) <= 100_000)) {
        json(400, { error: "invalid event" }); return;
      }
      if (event.type === "audio" && (event.audio_state !== "running" || event.frequency_hz !== 440)) {
        json(400, { error: "invalid audio event" }); return;
      }
      let profileStorage, fileInput;
      if (event.type === "file_input") {
        const p = event.file_input;
        const digest = typeof p?.sha256 === "string" && /^[a-f0-9]{64}$/.test(p.sha256);
        if (!record.fileProbe || p?.schema !== "elastos.browser.file-input/v1" ||
            p.expected_sha256 !== record.fileProbe.expected_sha256 ||
            !(p.size_bytes === null || Number.isSafeInteger(p.size_bytes) && p.size_bytes >= 0) ||
            !Number.isSafeInteger(p.elapsed_ms) || p.elapsed_ms < 0 ||
            ![p.read_completed, p.hash_completed, p.measurement_ok, p.matches, p.ok].every(v => typeof v === "boolean") ||
            ![null, "invalid_selection", "invalid_size", "hash_unavailable", "read_failed", "hash_failed", "file_timeout"].includes(p.error) ||
            p.measurement_ok !== p.hash_completed || p.measurement_ok !== (p.error === null) ||
            (p.hash_completed ? !digest || !p.read_completed || p.size_bytes !== 65536 || p.elapsed_ms >= 5000 : p.sha256 !== null) ||
            p.matches !== (p.hash_completed && p.sha256 === p.expected_sha256) || p.ok !== p.matches) {
          json(400, { error: "invalid file input event" }); return;
        }
        fileInput = { schema: p.schema, expected_sha256: p.expected_sha256, size_bytes: p.size_bytes, sha256: p.sha256,
          read_completed: p.read_completed, hash_completed: p.hash_completed, measurement_ok: p.measurement_ok,
          matches: p.matches, ok: p.ok, error: p.error, elapsed_ms: p.elapsed_ms };
      }
      if (event.type === "profile_storage") {
        const p = event.profile_storage, idb = p?.indexed_db;
        const matches = value => value && typeof value.present === "boolean" && typeof value.matches === "boolean" &&
          (!value.matches || value.present);
        if (!record.profile || p?.schema !== "elastos.browser.profile-storage/v1" ||
            p.mode !== record.profile.mode || p.marker !== record.profile.marker ||
            typeof p.measurement_ok !== "boolean" || typeof p.ok !== "boolean" ||
            !matches(p.cookie) || !matches(p.local_storage) || !matches(idb) ||
            ![idb.write_request_succeeded, idb.write_committed, idb.read_request_succeeded, idb.read_completed]
              .every(v => typeof v === "boolean") ||
            ![null, "storage_timeout", "database_open_failed", "transaction_aborted", "storage_unavailable"].includes(p.error) ||
            p.measurement_ok !== (p.error === null) ||
            (idb.write_committed && !idb.write_request_succeeded) || (idb.read_completed && !idb.read_request_succeeded) ||
            (p.mode === "read" && (idb.write_request_succeeded || idb.write_committed)) ||
            p.ok !== (p.measurement_ok && p.cookie.matches && p.local_storage.matches && idb.matches &&
              idb.read_request_succeeded && idb.read_completed && (p.mode === "read" || idb.write_committed))) {
          json(400, { error: "invalid profile storage event" }); return;
        }
        const copyMatches = v => ({ present: v.present, matches: v.matches });
        profileStorage = { schema: p.schema, mode: p.mode, marker: p.marker, measurement_ok: p.measurement_ok, ok: p.ok,
          cookie: copyMatches(p.cookie), local_storage: copyMatches(p.local_storage), indexed_db: {
            ...copyMatches(idb), write_request_succeeded: idb.write_request_succeeded, write_committed: idb.write_committed,
            read_request_succeeded: idb.read_request_succeeded, read_completed: idb.read_completed }, error: p.error };
      }
      if (record.events.length >= MAX_EVENTS && !record.qualification || record.sequence >= 8192) {
        json(429, { error: "fixture event capacity" }); return;
      }
      if (record.events.length >= MAX_EVENTS) record.events.shift();
      record.events.push({ sequence: ++record.sequence, received_at: now(),
        type: event.type, page: event.page, value: event.value,
        ...(event.type === "audio" ? { audio_state: event.audio_state, frequency_hz: event.frequency_hz } : {}),
        ...(profileStorage ? { profile_storage: profileStorage } : {}),
        ...(fileInput ? { file_input: fileInput } : {}),
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
