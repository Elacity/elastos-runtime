/* Home Studio — Generate + Storyboard + Character via CREATIVE_* jobs.
   Tip: home-20260814a — P2 Character refs + library delete */

import { fetchJson, getHomeGuiLaunchToken } from "./shell-core.js?v=home-20260814a";
import { modelRunCall as modelCall } from "./agent-live.js?v=home-20260814a";
import { appendArtifactMessage } from "./agent-stream.js?v=home-20260814a";
import { showViewerRail } from "./shell-viewer-rail.js?v=home-20260814a";

const DURATIONS = [2, 3, 5, 10, 15, 30];
const SCALES = [1, 2, 4];
const POLL_MS = 2000;
const MAX_PROMPT = 12_000;
const MAX_REF_IMAGES = 6;
const MAX_REF_BYTES = 4_000_000;
const MAX_VOICE_BYTES = 8_000_000;

let bound = false;
let pollTimer = null;
let videoObjectUrl = null;
let activeJobId = null;
let pendingRefs = [];
let pendingVoice = null;
/** @type {null | { default: number, options: Array<Record<string, unknown>> }} */
let scaleCatalog = null;

function creativeUrl(path) {
  return new URL(path, window.location.href).href;
}

/** Contract-era Studio: all generation runs through the model contract
 * (runs.*). There is no legacy job path on this branch. */


function creativeErrorMessage(err, fallback) {
  const raw = String(err?.message || fallback || "request failed");
  const brace = raw.indexOf("{");
  if (brace >= 0) {
    try {
      const parsed = JSON.parse(raw.slice(brace));
      if (parsed?.message) {
        return String(parsed.message);
      }
      if (parsed?.code) {
        return String(parsed.code);
      }
    } catch {
      /* keep raw */
    }
  }
  return raw;
}

function panelEl() {
  return document.querySelector("[data-studio-page]");
}

function currentMode() {
  const raw = panelEl()?.querySelector("[data-studio-mode]")?.value;
  if (raw === "character") {
    return "character";
  }
  if (raw === "storyboard") {
    return "storyboard";
  }
  return "generate";
}

/** Storyboard uses the Generate backend (2× FL2VA). */
function backendMode(mode = currentMode()) {
  return mode === "character" ? "character" : "generate";
}

function parseStoryboardShots(shotsText) {
  return String(shotsText || "")
    .split(/\n+/)
    .map((s) => s.trim())
    .filter(Boolean)
    .slice(0, 8);
}

/** One Generate job per shot (P1b), then ffmpeg stitch on Home. */
function composeShotPrompt(shot, index, total) {
  return (
    "EDITING RULE: Single continuous shot for a multi-shot edit. Cut-ready framing. " +
    "Native stereo audio; no burned-in titles.\n" +
    "Continuity: same product world and tone as the rest of the sequence; no on-screen text or logos.\n\n" +
    `SHOT ${index + 1} of ${total}: ${shot}`
  );
}

function currentScale() {
  const n = Number(panelEl()?.querySelector("[data-studio-scale]")?.value);
  return SCALES.includes(n) ? n : 2;
}

function setStatus(text, tone = "idle") {
  const el = panelEl()?.querySelector("[data-studio-status]");
  if (!el) {
    return;
  }
  el.textContent = text;
  el.dataset.tone = tone;
  el.hidden = !text;
}

function setProgress(percent, phase, message) {
  const wrap = panelEl()?.querySelector("[data-studio-progress]");
  const bar = panelEl()?.querySelector("[data-studio-progress-bar]");
  const label = panelEl()?.querySelector("[data-studio-progress-label]");
  if (!wrap || !bar || !label) {
    return;
  }
  const pct = Math.max(0, Math.min(100, Number(percent) || 0));
  wrap.hidden = false;
  bar.style.width = `${pct}%`;
  label.textContent = [phase, message].filter(Boolean).join(" · ");
}

function hideProgress() {
  const wrap = panelEl()?.querySelector("[data-studio-progress]");
  if (wrap) {
    wrap.hidden = true;
  }
}

function clearVideo() {
  const video = panelEl()?.querySelector("[data-studio-video]");
  const dl = panelEl()?.querySelector("[data-studio-download]");
  if (video) {
    video.pause?.();
    video.removeAttribute("src");
    video.removeAttribute("poster");
    video.load();
    video.hidden = true;
  }
  if (dl) {
    dl.hidden = true;
    dl.removeAttribute("href");
  }
  if (videoObjectUrl) {
    URL.revokeObjectURL(videoObjectUrl);
    videoObjectUrl = null;
  }
}

function optionForScale(n) {
  const options = scaleCatalog?.options;
  if (!Array.isArray(options)) {
    return null;
  }
  return options.find((o) => Number(o?.n) === n) || null;
}

function scaleReadyForMode(n, mode) {
  const opt = optionForScale(n);
  if (!opt) {
    /* Before first status: Character=1×, Generate=default 2× assumed wired. */
    if (mode === "character") {
      return n === 1;
    }
    return n === 2;
  }
  if (mode === "character") {
    return Boolean(opt.character?.wired);
  }
  return Boolean(opt.generate?.wired);
}

function scaleReachableForMode(n, mode) {
  const opt = optionForScale(n);
  if (!opt) {
    return false;
  }
  if (mode === "character") {
    return Boolean(opt.character?.reachable);
  }
  return Boolean(opt.generate?.reachable);
}

function syncScaleUi() {
  const panel = panelEl();
  if (!panel) {
    return;
  }
  const mode = currentMode();
  const select = panel.querySelector("[data-studio-scale]");
  // Scale note line removed from markup (Occam); keep updater inert if absent.
  const note = panel.querySelector("[data-studio-scale-note]");
  if (!select) {
    return;
  }

  const scaleMode = backendMode(mode);
  const preferred =
    mode === "character" ? 1 : Number(scaleCatalog?.default) || currentScale() || 2;

  for (const option of select.options) {
    const n = Number(option.value);
    const wired = scaleReadyForMode(n, scaleMode);
    const up = scaleReachableForMode(n, scaleMode);
    const meta = optionForScale(n);
    let label = `${n}×`;
    if (n === 1) {
      label = "1× · learn";
    } else if (n === 2) {
      label = "2× · everyday";
    } else if (n === 4) {
      label = "4× · max";
    }
    if (mode === "character" && n !== 1) {
      option.disabled = true;
      option.textContent = `${label} · Character is 1×`;
    } else if (!wired) {
      option.disabled = true;
      option.textContent = `${label} · not wired`;
    } else if (!up) {
      option.disabled = false;
      option.textContent = `${label} · offline`;
    } else {
      option.disabled = false;
      option.textContent = label;
    }
    if (meta?.chat && wired) {
      option.title = String(meta.chat);
    }
  }

  if (mode === "character") {
    select.value = "1";
  } else {
    const cur = Number(select.value);
    if (!scaleReadyForMode(cur, scaleMode)) {
      const fallback = SCALES.find((n) => scaleReadyForMode(n, scaleMode)) || preferred;
      select.value = String(fallback);
    }
  }

  if (note) {
    const n = currentScale();
    const meta = optionForScale(n);
    const chat = meta?.chat || (n === 4 ? "chat off" : "chat stays available");
    const product = meta?.note || "";
    if (mode === "character") {
      const up = scaleReachableForMode(1, "character");
      note.textContent = up
        ? "1× Comfy Ref2VA · face + optional voice · chat stays available"
        : "1× Character · Comfy offline — submit will Prepare";
      note.dataset.tone = up ? "ok" : "warn";
      return;
    }
    if (mode === "storyboard") {
      const up = scaleReachableForMode(n, "generate");
      note.textContent = up
        ? `${n}× Storyboard → one Generate job · ${chat}`
        : `${n}× Storyboard · Generate offline — submit will Prepare`;
      note.dataset.tone = up ? "ok" : "warn";
      return;
    }
    const wired = scaleReadyForMode(n, "generate");
    const up = scaleReachableForMode(n, "generate");
    if (!wired) {
      note.textContent = `${n}× Generate not wired on this Home yet`;
      note.dataset.tone = "warn";
    } else if (!up) {
      note.textContent = `${n}× Generate offline · ${chat}${product ? ` · ${product}` : ""}`;
      note.dataset.tone = "warn";
    } else {
      note.textContent = `${n}× ready · ${chat}${product ? ` · ${product}` : ""}`;
      note.dataset.tone = "ok";
    }
  }
}

function syncModeUi() {
  const panel = panelEl();
  if (!panel) {
    return;
  }
  const mode = currentMode();
  const refs = panel.querySelector("[data-studio-refs]");
  const board = panel.querySelector("[data-studio-storyboard]");
  const submit = panel.querySelector("[data-studio-submit]");
  const prompt = panel.querySelector("[data-studio-prompt]");
  const promptLabel = panel.querySelector("[data-studio-prompt-label]");
  if (refs) {
    refs.hidden = mode !== "character";
  }
  if (board) {
    board.hidden = mode !== "storyboard";
  }
  if (prompt) {
    prompt.hidden = mode === "storyboard";
  }
  if (promptLabel) {
    promptLabel.hidden = mode === "storyboard";
  }
  if (submit) {
    if (mode === "character") {
      submit.textContent = "Lock character";
    } else if (mode === "storyboard") {
      submit.textContent = "Generate storyboard";
    } else {
      submit.textContent = "Generate";
    }
  }
  if (prompt && !prompt.value.trim() && mode !== "storyboard") {
    prompt.placeholder =
      mode === "character"
        ? "<Picture 1> identity · optional <Audio 1> voice. Soft window light, look at camera. Quiet room. No text."
        : "A quiet alley after rain — neon reflections, soft footsteps, stereo ambience";
  }
  const durationLabel = panel.querySelector("[data-studio-duration-label]");
  if (durationLabel) {
    /* Storyboard applies the duration PER SHOT, then stitches — say so on the
       label so "15s" can't be misread as the total output length. */
    durationLabel.textContent = mode === "storyboard" ? "Duration / shot" : "Duration";
  }
  syncStoryboardTotal();
  syncScaleUi();
}

/* Live "N shots × Xs each = Ys total" readout under the shots box — the
   storyboard multiplies duration per line, which is otherwise invisible. */
function syncStoryboardTotal() {
  const panel = panelEl();
  const el = panel?.querySelector("[data-studio-storyboard-total]");
  if (!el) {
    return;
  }
  if (currentMode() !== "storyboard") {
    el.hidden = true;
    return;
  }
  const shots = parseStoryboardShots(panel.querySelector("[data-studio-shots]")?.value || "");
  const duration = Number(panel.querySelector("[data-studio-duration]")?.value) || 10;
  if (!shots.length) {
    el.hidden = true;
    return;
  }
  el.hidden = false;
  const total = shots.length * duration;
  el.textContent = `${shots.length} shot${shots.length === 1 ? "" : "s"} × ${duration}s each = ${total}s total output`;
}

function readFileAsDataUrl(file, { kind, maxBytes }) {
  return new Promise((resolve, reject) => {
    if (!file) {
      reject(new Error(`missing ${kind} file`));
      return;
    }
    if (kind === "image" && !file.type.startsWith("image/")) {
      reject(new Error("face refs must be image files"));
      return;
    }
    if (kind === "audio" && !file.type.startsWith("audio/")) {
      reject(new Error("voice ref must be an audio file (wav/mp3/m4a)"));
      return;
    }
    if (file.size > maxBytes) {
      reject(new Error(`${kind} max ${maxBytes} bytes`));
      return;
    }
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result || ""));
    reader.onerror = () => reject(new Error(`could not read ${kind}`));
    reader.readAsDataURL(file);
  });
}

async function loadVideo(jobId) {
  const token = getHomeGuiLaunchToken();
  if (!token) {
    throw new Error("missing home launch token");
  }
  const response = await fetch(creativeUrl(`/api/apps/home/creative/jobs/${jobId}/video`), {
    headers: { "x-elastos-home-token": token },
  });
  if (!response.ok) {
    throw new Error(`video fetch failed (${response.status})`);
  }
  const buf = await response.arrayBuffer();
  /* Force MIME — opaque sandbox + wrong blob type = blank <video> while QuickLook works. */
  const blob = new Blob([buf], { type: "video/mp4" });
  clearVideo();
  videoObjectUrl = URL.createObjectURL(blob);
  const video = panelEl()?.querySelector("[data-studio-video]");
  const dl = panelEl()?.querySelector("[data-studio-download]");
  if (video) {
    video.preload = "auto";
    video.controls = true;
    video.playsInline = true;
    video.src = videoObjectUrl;
    /* The side panel is the player now — keep this element as a hidden buffer
       (it holds the blob URL) but never surface it inline above the list. */
    video.hidden = true;
    const paint = () => {
      try {
        if (video.readyState >= 1 && Number.isFinite(video.duration) && video.duration > 0.05) {
          video.currentTime = Math.min(0.12, video.duration * 0.05);
        }
      } catch {
        /* seek optional */
      }
    };
    video.addEventListener("loadeddata", paint, { once: true });
    video.addEventListener("loadedmetadata", paint, { once: true });
    video.load();
  }
  if (dl) {
    dl.href = videoObjectUrl;
    dl.download = `studio-${jobId.slice(0, 8)}.mp4`;
    /* Keep the inline "Save clip" link hidden — Save is a hover action on the
       clip row now. */
    dl.hidden = true;
  }
}

function stopPoll() {
  if (pollTimer) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

function setBusy(busy) {
  const form = panelEl()?.querySelector("[data-studio-form]");
  if (!form) {
    return;
  }
  form.querySelectorAll("textarea, select, button, input").forEach((el) => {
    if (el.matches("[data-studio-submit]")) {
      el.disabled = busy;
      const mode = currentMode();
      if (!busy) {
        if (mode === "character") {
          el.textContent = "Lock character";
        } else if (mode === "storyboard") {
          el.textContent = "Generate storyboard";
        } else {
          el.textContent = "Generate";
        }
      } else if (mode === "character") {
        el.textContent = "Locking…";
      } else {
        el.textContent = "Generating…";
      }
    } else if (el.matches("[data-studio-scale]")) {
      el.disabled = busy;
      if (!busy) {
        syncScaleUi();
      }
    } else {
      el.disabled = busy;
    }
  });
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function ensurePrepared(mode) {
  const initial = await fetchJson(creativeUrl("/api/apps/home/creative/status"));
  const readyNow =
    mode === "character"
      ? Boolean(initial?.character?.upstream_reachable)
      : Boolean(initial?.generate?.upstream_reachable);
  if (readyNow) {
    return;
  }
  if (!initial?.allocator?.configured) {
    throw new Error(
      mode === "character"
        ? "Character offline — prepare not configured (CREATIVE_PREPARE_CMD)"
        : "Generate offline — prepare not configured (CREATIVE_PREPARE_CMD)",
    );
  }

  const alloc0 = initial?.allocator;
  if (alloc0?.status !== "preparing") {
    await fetchJson(creativeUrl("/api/apps/home/creative/prepare"), {
      method: "POST",
      body: JSON.stringify({ target: mode }),
    });
  }

  setStatus(
    mode === "character"
      ? "Preparing 1× Character on the configured backend… (can take several minutes)"
      : "Preparing 2× Generate on the configured backend… (can take several minutes)",
    "busy",
  );
  setProgress(5, "Preparing", "allocator");

  const deadline = Date.now() + 20 * 60 * 1000;
  while (Date.now() < deadline) {
    await sleep(3000);
    const status = await fetchJson(creativeUrl("/api/apps/home/creative/status"));
    const alloc = status?.allocator || {};
    setProgress(
      alloc.percent ?? 10,
      alloc.phase || "Preparing",
      alloc.message || `${alloc.elapsed_s ?? 0}s`,
    );
    const up =
      mode === "character"
        ? Boolean(status?.character?.upstream_reachable)
        : Boolean(status?.generate?.upstream_reachable);
    if (up || alloc.status === "ready") {
      setStatus(
        mode === "character" ? "Character ready — locking…" : "Generate ready — starting…",
        "busy",
      );
      return;
    }
    if (alloc.status === "error") {
      throw new Error(alloc.error || alloc.message || "Prepare failed");
    }
  }
  throw new Error("Prepare timed out — check the configured video backend");
}

/** Run lifecycle via the model contract (runs.*). */
async function waitForRun(runId) {
  let cursor = 0;
  for (;;) {
    const data = await modelCall("runs_events", { run_id: runId, cursor });
    cursor = data?.cursor ?? cursor;
    const state = String(data?.state || "");
    const events = Array.isArray(data?.events) ? data.events : [];
    const lastProgress = [...events].reverse().find((e) => e?.type === "progress");
    if (lastProgress) {
      const pct = lastProgress.total
        ? Math.round((lastProgress.completed / lastProgress.total) * 100)
        : undefined;
      setProgress(pct, lastProgress.phase, "");
    }
    const errEvent = events.find((e) => e?.type === "error");
    if (state === "succeeded") {
      const result = events.find((e) => e?.type === "result");
      const artifactId = result?.objects?.[0]?.id;
      return { artifactId };
    }
    if (state === "failed") {
      throw new Error(errEvent?.message || "run failed");
    }
    if (state === "cancelled") {
      throw new Error("run cancelled");
    }
    await sleep(POLL_MS);
  }
}

/** P4.5: contract path for generate mode (offer:h3-video:2x). */
async function startJobViaContract(payload) {
  stopPoll();
  clearVideo();
  setBusy(true);
  try {
    await ensurePrepared("generate");
  } catch (err) {
    setBusy(false);
    hideProgress();
    setStatus(creativeErrorMessage(err, "Could not prepare Studio"), "error");
    return;
  }

  setStatus("Queued (contract)…", "busy");
  setProgress(0, "Queued", `${payload.duration}s`);

  let created;
  try {
    created = await modelCall("runs_create", {
      offer_id: "offer:h3-video:2x",
      operation: "generate",
      inputs: {
        prompt: payload.prompt,
        duration_seconds: payload.duration,
      },
    });
  } catch (err) {
    setBusy(false);
    hideProgress();
    setStatus(creativeErrorMessage(err, "Could not start run"), "error");
    return;
  }

  const runId = String(created?.run_id || "");
  if (!runId) {
    setBusy(false);
    hideProgress();
    setStatus("Contract returned no run id", "error");
    return;
  }

  activeJobId = runId;
  setStatus(`Run ${runId.slice(0, 14)}…`, "busy");
  try {
    const { artifactId } = await waitForRun(runId);
    activeJobId = null;
    setBusy(false);
    setStatus("Done — clip ready on this Home. (contract)", "ok");
    setProgress(100, "Done", "");
    if (artifactId) {
      await loadVideo(artifactId);
      /* File a clickable artifact card into the active chat (the Studio player
         already shows the clip, so we don't yank the rail open here). */
      try {
        const artifact = clipArtifact(artifactId, payload?.prompt, payload?.duration);
        const promptText = String(payload?.prompt || "").trim();
        appendArtifactMessage(promptText ? `Clip — ${promptText}` : "Clip on this Home.", artifact);
      } catch {
        /* chat card is best-effort */
      }
    }
    refreshLibrary();
  } catch (err) {
    activeJobId = null;
    setBusy(false);
    hideProgress();
    setStatus(creativeErrorMessage(err, "Run failed"), "error");
  }
}

async function startJob(payload) {
  return startJobViaContract(payload);
}

/** Storyboard: one contract run per shot → POST /creative/stitch. */
async function startStoryboardJobs({ shots, duration }) {
  stopPoll();
  clearVideo();
  setBusy(true);
  const total = shots.length;

  try {
    await ensurePrepared("generate");
  } catch (err) {
    setBusy(false);
    hideProgress();
    setStatus(creativeErrorMessage(err, "Could not prepare Studio"), "error");
    return;
  }

  const ids = [];
  try {
    for (let i = 0; i < total; i++) {
      const prompt = composeShotPrompt(shots[i], i, total);
      setStatus(`Storyboard shot ${i + 1}/${total}…`, "busy");
      setProgress((i / total) * 85, "Generating", `shot ${i + 1}/${total}`);
      const created = await modelCall("runs_create", {
        offer_id: "offer:h3-video:2x",
        operation: "generate",
        inputs: {
          prompt,
          duration_seconds: duration,
        },
      });
      const runId = String(created?.run_id || "");
      if (!runId) {
        throw new Error("Contract returned no run id");
      }
      activeJobId = runId;
      const { artifactId } = await waitForRun(runId);
      if (!artifactId) {
        throw new Error(`Shot ${i + 1} finished without an artifact`);
      }
      ids.push(artifactId);
    }
    activeJobId = null;
    setStatus(`Stitching ${ids.length} shots…`, "busy");
    setProgress(92, "Stitching", "ffmpeg");
    const stitched = await fetchJson(creativeUrl("/api/apps/home/creative/stitch"), {
      method: "POST",
      /* Raw shot lines ride along → stored on the stitch sidecar, so the
         finished video keeps its readable provenance (prompt drop-down). */
      body: JSON.stringify({ job_ids: ids, shots }),
    });
    const stitchId = String(stitched?.id || "");
    if (!stitchId) {
      throw new Error("Stitch returned no job id");
    }
    setBusy(false);
    setStatus("Done — storyboard stitched on this Home.", "ok");
    setProgress(100, "Done", `${ids.length} shots`);
    await loadVideo(stitchId);
    refreshLibrary();
  } catch (err) {
    activeJobId = null;
    setBusy(false);
    hideProgress();
    setStatus(creativeErrorMessage(err, "Storyboard failed"), "error");
  }
}

async function refreshCreativeStatus() {
  const el = panelEl()?.querySelector("[data-studio-profile]");
  if (!el) {
    return;
  }
  try {
    const status = await fetchJson(creativeUrl("/api/apps/home/creative/status"));
    const profile = status?.profile || "unknown";
    scaleCatalog = status?.scale || null;
    const genUp = Boolean(status?.generate?.upstream_reachable);
    const genWired = Boolean(status?.generate?.wired);
    const gen = genUp ? "Generate ready" : genWired ? "Generate offline" : "Generate not wired";
    const charWired = Boolean(status?.character?.wired);
    const charUp = Boolean(status?.character?.upstream_reachable);
    let char = "Character not wired";
    if (charWired && charUp) {
      char = "Character / Ref2VA ready";
    } else if (charWired) {
      char = "Character wired · Comfy down (park 2× first)";
    }
    const def = Number(status?.scale?.default) || 2;
    const alloc = status?.allocator;
    let allocNote = "";
    if (alloc?.status === "preparing") {
      allocNote = ` · preparing ${alloc.target || ""}`.trimEnd();
    } else if (alloc?.configured) {
      allocNote = " · allocator on";
    }
    el.textContent = `${profile} · default ${def}× · ${gen} · ${char}${allocNote}`;
    el.dataset.state = genUp || charUp || alloc?.status === "preparing" ? "ready" : "offline";
    syncScaleUi();
  } catch (err) {
    el.textContent = err?.message || "Creative status unavailable";
    el.dataset.state = "offline";
  }
}

function formatLibraryWhen(mtimeMs) {
  const n = Number(mtimeMs) || 0;
  if (!n) {
    return "";
  }
  try {
    return new Intl.DateTimeFormat(undefined, {
      month: "short",
      day: "numeric",
      hour: "numeric",
      minute: "2-digit",
    }).format(new Date(n));
  } catch {
    return "";
  }
}

function formatLibraryBytes(bytes) {
  const n = Number(bytes) || 0;
  if (n >= 1_000_000) {
    return `${(n / 1_000_000).toFixed(1)} MB`;
  }
  if (n >= 1_000) {
    return `${Math.round(n / 1_000)} KB`;
  }
  return n ? `${n} B` : "";
}

function libraryPromptPreview(job) {
  const mode = String(job?.mode || "generate");
  /* Storyboard stitches: preview the first raw shot line (+ how many more)
     instead of the internal "stitch of N shots" summary. */
  const shots = Array.isArray(job?.shots)
    ? job.shots.map((s) => String(s || "").trim()).filter(Boolean)
    : [];
  if (shots.length) {
    const first = shots[0].replace(/\s+/g, " ");
    const base = first.length > 110 ? `${first.slice(0, 107)}…` : first;
    return shots.length > 1 ? `${base} · +${shots.length - 1} more` : base;
  }
  const prompt = String(job?.prompt || "").trim().replace(/\s+/g, " ");
  if (mode === "storyboard" && /^storyboard stitch/i.test(prompt)) {
    return prompt.length > 120 ? `${prompt.slice(0, 117)}…` : prompt;
  }
  if (prompt) {
    return prompt.length > 120 ? `${prompt.slice(0, 117)}…` : prompt;
  }
  const id = String(job?.id || "").slice(0, 8);
  if (mode === "character") {
    return id ? `Character ${id}` : "Character clip";
  }
  if (mode === "storyboard") {
    return id ? `Storyboard ${id}` : "Storyboard";
  }
  return id ? `Clip ${id}` : "Clip";
}

function currentRefImageSize() {
  const raw = panelEl()?.querySelector("[data-studio-ref-size]")?.value;
  return raw === "max" ? "max" : "match";
}

function setLibraryActive(jobId) {
  const list = panelEl()?.querySelector("[data-studio-library-list]");
  if (!list) {
    return;
  }
  list.querySelectorAll("[data-studio-library-item]").forEach((btn) => {
    btn.dataset.active = btn.dataset.jobId === jobId ? "true" : "false";
  });
}

async function openLibraryClip(jobId) {
  if (!jobId || activeJobId) {
    return;
  }
  setLibraryActive(jobId);
  setStatus("Loading clip…", "busy");
  try {
    await loadVideo(jobId);
    setStatus("Playing clip from this Home.", "ok");
  } catch (err) {
    setStatus(creativeErrorMessage(err, "Could not load clip"), "error");
  }
}

/* Build the canonical artifact object for a clip. */
function clipArtifact(jobId, prompt, duration) {
  const promptText = String(prompt || "").trim();
  return {
    id: String(jobId),
    kind: "video",
    title: promptText ? `Clip · ${duration || 5}s` : "Video clip",
    subtitle: promptText || "Generated clip",
    mediaUrl: creativeUrl(`/api/apps/home/creative/jobs/${jobId}/video`),
    mediaType: "video",
  };
}

/* "→" on a library row: open the clip in the side panel right now (the feedback
   the user expects), AND file a card into the active chat so it's saved. */
function openClipInViewer(jobId, prompt, duration) {
  const artifact = clipArtifact(jobId, prompt, duration);
  /* Open the side panel immediately — this is the visible result of the click. */
  try {
    showViewerRail({
      mediaUrl: artifact.mediaUrl,
      mediaType: "video",
      title: artifact.title,
      kind: "video",
    });
  } catch {
    /* rail open is best-effort */
  }
  /* Also drop a card into the chat so the clip is a first-class saved result. */
  try {
    const promptText = String(prompt || "").trim();
    appendArtifactMessage(promptText ? `Clip — ${promptText}` : "Clip on this Home.", artifact);
  } catch {
    /* chat card is best-effort */
  }
}

/* Save the clip to disk (Downloads) — fetches the bytes and triggers a save. */
async function saveClipToDisk(jobId) {
  try {
    const token = getHomeGuiLaunchToken();
    const response = await fetch(creativeUrl(`/api/apps/home/creative/jobs/${jobId}/video`), {
      headers: token ? { "x-elastos-home-token": token } : {},
    });
    if (!response.ok) {
      throw new Error(`fetch failed (${response.status})`);
    }
    const buf = await response.arrayBuffer();
    const url = URL.createObjectURL(new Blob([buf], { type: "video/mp4" }));
    const a = document.createElement("a");
    a.href = url;
    a.download = `clip-${jobId.slice(0, 8)}.mp4`;
    document.body.append(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 5000);
    setStatus("Clip saved.", "ok");
  } catch (err) {
    setStatus(creativeErrorMessage(err, "Could not save clip"), "error");
  }
}

async function deleteLibraryClip(jobId) {
  if (!jobId || activeJobId) {
    return;
  }
  if (!window.confirm("Remove this clip from this Home?")) {
    return;
  }
  try {
    await fetchJson(creativeUrl(`/api/apps/home/creative/jobs/${jobId}`), {
      method: "DELETE",
    });
    const video = panelEl()?.querySelector("[data-studio-video]");
    const dl = panelEl()?.querySelector("[data-studio-download]");
    if (video?.src && String(dl?.download || "").includes(jobId.slice(0, 8))) {
      clearVideo();
    }
    setStatus("Clip removed from this Home.", "ok");
    await refreshLibrary();
  } catch (err) {
    setStatus(creativeErrorMessage(err, "Could not remove clip"), "error");
  }
}

async function refreshLibrary() {
  const panel = panelEl();
  const list = panel?.querySelector("[data-studio-library-list]");
  const empty = panel?.querySelector("[data-studio-library-empty]");
  if (!list || !empty) {
    return;
  }
  try {
    const data = await fetchJson(creativeUrl("/api/apps/home/creative/jobs"));
    const jobs = Array.isArray(data?.jobs) ? data.jobs : [];
    const playable = jobs.filter((j) => j?.has_video !== false && String(j?.status) === "done");
    list.replaceChildren();
    if (!playable.length) {
      empty.hidden = false;
      empty.textContent = "No clips yet — Generate one above.";
      return;
    }
    empty.hidden = true;
    for (const job of playable) {
      const id = String(job.id || "");
      if (!id) {
        continue;
      }
      const row = document.createElement("div");
      row.className = "agent-studio-library-row";
      /* Card body: the visible chip + hover icons. Prompt expand lives outside
         this so absolute-positioned actions stay inside the chip, not under it. */
      const main = document.createElement("div");
      main.className = "agent-studio-library-main";
      /* Main zone: click → open the clip in the side panel (the player lives
         there now — no big inline preview above the list). */
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "agent-studio-library-item";
      btn.dataset.studioLibraryItem = "";
      btn.dataset.jobId = id;
      const prompt = document.createElement("span");
      prompt.className = "agent-studio-library-prompt";
      prompt.textContent = libraryPromptPreview(job);
      const meta = document.createElement("span");
      meta.className = "agent-studio-library-meta";
      const mode = String(job.mode || "generate");
      const scale =
        job.scale != null && Number(job.scale) > 0 ? `${job.scale}×` : "";
      const when = formatLibraryWhen(job.mtime_ms);
      const size = formatLibraryBytes(job.bytes);
      meta.textContent = [mode, scale, when, size].filter(Boolean).join(" · ");
      btn.append(prompt, meta);
      btn.addEventListener("click", () => {
        openClipInViewer(id, job.prompt, job.duration_seconds ?? job.duration);
      });

      /* Hover actions: Prompt · Save · Delete. */
      const actions = document.createElement("span");
      actions.className = "agent-studio-library-actions";

      /* Prompt drop-down: every skill surfaces whatever provenance the asset
         carries. Storyboard stitches prefer the numbered shot list (from the
         sidecar, or rebuilt from source clips by the list API); everything
         else shows the exact generation prompt. */
      const shots = Array.isArray(job.shots)
        ? job.shots.map((s) => String(s || "").trim()).filter(Boolean)
        : [];
      const fullPrompt = String(job.prompt || "").trim();
      const promptDisplay = shots.length
        ? shots.map((s, i) => `${i + 1}. ${s}`).join("\n")
        : fullPrompt;
      let promptBtn = null;
      let promptPanel = null;
      if (promptDisplay) {
        promptBtn = document.createElement("button");
        promptBtn.type = "button";
        promptBtn.className = "agent-studio-library-action";
        promptBtn.dataset.studioPromptToggle = "";
        promptBtn.title = shots.length ? "Show shot list" : "Show prompt";
        promptBtn.setAttribute(
          "aria-label",
          shots.length
            ? `Show shot list for clip ${id.slice(0, 8)}`
            : `Show prompt for clip ${id.slice(0, 8)}`,
        );
        promptBtn.innerHTML =
          '<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M2.75 4.25h10.5M2.75 8h10.5M2.75 11.75h6.5"/></svg>';

        promptPanel = document.createElement("div");
        promptPanel.className = "agent-studio-library-prompt-full";
        const promptInner = document.createElement("div");
        promptInner.className = "agent-studio-library-prompt-inner";
        const promptText = document.createElement("p");
        promptText.textContent = promptDisplay;
        const copyBtn = document.createElement("button");
        copyBtn.type = "button";
        copyBtn.className = "agent-studio-library-prompt-copy";
        copyBtn.textContent = "Copy";
        copyBtn.addEventListener("click", (event) => {
          event.preventDefault();
          event.stopPropagation();
          navigator.clipboard?.writeText(promptDisplay).then(() => {
            copyBtn.textContent = "Copied";
            window.setTimeout(() => {
              copyBtn.textContent = "Copy";
            }, 1500);
          }).catch(() => {});
        });
        promptInner.append(promptText, copyBtn);
        promptPanel.append(promptInner);

        promptBtn.setAttribute("aria-expanded", "false");
        promptBtn.addEventListener("click", (event) => {
          event.preventDefault();
          event.stopPropagation();
          const opening = !promptPanel.classList.contains("open");
          list.querySelectorAll(".agent-studio-library-prompt-full.open").forEach((p) => {
            p.classList.remove("open");
          });
          list.querySelectorAll("[data-studio-prompt-toggle][aria-expanded='true']").forEach((b) => {
            b.setAttribute("aria-expanded", "false");
          });
          if (opening) {
            promptPanel.classList.add("open");
            promptBtn.setAttribute("aria-expanded", "true");
          }
        });
      }

      const save = document.createElement("button");
      save.type = "button";
      save.className = "agent-studio-library-action";
      save.title = "Save clip";
      save.setAttribute("aria-label", `Save clip ${id.slice(0, 8)}`);
      save.innerHTML =
        '<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M8 2.5v8M4.75 7.25 8 10.5l3.25-3.25M2.75 13.5h10.5"/></svg>';
      save.addEventListener("click", (event) => {
        event.preventDefault();
        event.stopPropagation();
        saveClipToDisk(id);
      });

      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "agent-studio-library-action agent-studio-library-action-danger";
      remove.title = "Delete clip";
      remove.setAttribute("aria-label", `Remove clip ${id.slice(0, 8)}`);
      remove.innerHTML =
        '<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M2.5 4h11M6.5 4V2.75h3V4M3.5 4l.7 9.5h7.6l.7-9.5"/></svg>';
      remove.addEventListener("click", (event) => {
        event.preventDefault();
        event.stopPropagation();
        deleteLibraryClip(id);
      });

      if (promptBtn) {
        actions.append(promptBtn);
      }
      actions.append(save, remove);
      main.append(btn, actions);
      row.append(main);
      if (promptPanel) {
        row.append(promptPanel);
      }
      list.append(row);
    }
  } catch (err) {
    empty.hidden = false;
    empty.textContent = creativeErrorMessage(err, "Could not load clip library");
  }
}

export function bindAgentStudio() {
  if (bound) {
    return;
  }
  const panel = panelEl();
  if (!panel) {
    return;
  }
  bound = true;

  panel.querySelector("[data-studio-library-refresh]")?.addEventListener("click", () => {
    refreshLibrary();
  });

  /* Live shot-count × duration math while typing the storyboard. */
  panel.addEventListener("input", (event) => {
    if (event.target?.matches?.("[data-studio-shots]")) {
      syncStoryboardTotal();
    }
  });

  panel.addEventListener("change", (event) => {
    if (event.target?.matches?.("[data-studio-mode]")) {
      syncModeUi();
      return;
    }
    if (event.target?.matches?.("[data-studio-duration]")) {
      syncStoryboardTotal();
      return;
    }
    if (event.target?.matches?.("[data-studio-scale]")) {
      syncScaleUi();
      return;
    }
    if (event.target?.matches?.("[data-studio-ref-files]")) {
      const picked = Array.from(event.target.files || []);
      if (picked.length > MAX_REF_IMAGES) {
        setStatus(`Using first ${MAX_REF_IMAGES} face stills (max ${MAX_REF_IMAGES})`, "busy");
      }
      const files = picked.slice(0, MAX_REF_IMAGES);
      pendingRefs = [];
      Promise.all(
        files.map((f) => readFileAsDataUrl(f, { kind: "image", maxBytes: MAX_REF_BYTES })),
      )
        .then((urls) => {
          pendingRefs = urls;
          const note = panel.querySelector("[data-studio-ref-note]");
          if (note) {
            note.textContent = urls.length
              ? `${urls.length} face still(s) ready (max ${MAX_REF_IMAGES})`
              : "Face stills + optional voice clip lock the person.";
          }
        })
        .catch((err) => {
          pendingRefs = [];
          setStatus(err?.message || "Could not read face stills", "error");
        });
      return;
    }
    if (event.target?.matches?.("[data-studio-voice-file]")) {
      const file = event.target.files?.[0];
      pendingVoice = null;
      const note = panel.querySelector("[data-studio-voice-note]");
      if (!file) {
        if (note) {
          note.textContent = "No voice clip (optional)";
        }
        return;
      }
      readFileAsDataUrl(file, { kind: "audio", maxBytes: MAX_VOICE_BYTES })
        .then((url) => {
          pendingVoice = url;
          if (note) {
            note.textContent = `Voice ready · ${file.name}`;
          }
        })
        .catch((err) => {
          pendingVoice = null;
          if (note) {
            note.textContent = "No voice clip (optional)";
          }
          setStatus(err?.message || "Could not read voice clip", "error");
        });
    }
  });

  panel.addEventListener("submit", (event) => {
    const form = event.target?.closest?.("[data-studio-form]");
    if (!form) {
      return;
    }
    event.preventDefault();
    if (activeJobId) {
      return;
    }
    let duration = Number(form.querySelector("[data-studio-duration]")?.value);
    if (!duration || !DURATIONS.includes(duration)) {
      duration = 10;
    }
    const mode = currentMode();
    const scale = mode === "character" ? 1 : currentScale();
    let prompt = String(form.querySelector("[data-studio-prompt]")?.value || "").trim();
    if (mode === "storyboard") {
      const shots = parseStoryboardShots(form.querySelector("[data-studio-shots]")?.value || "");
      if (!shots.length) {
        setStatus("Add at least one shot (one line each)", "error");
        return;
      }
      if (!SCALES.includes(scale)) {
        setStatus("Scale must be 1×, 2×, or 4×", "error");
        return;
      }
      if (!scaleReadyForMode(scale, "generate")) {
        setStatus(`${scale}× Generate is not wired on this Home`, "error");
        return;
      }
      if (shots.length === 1) {
        startJob({
          prompt: composeShotPrompt(shots[0], 0, 1),
          duration,
          mode: "generate",
          scale,
        });
        return;
      }
      startStoryboardJobs({ shots, duration, scale });
      return;
    }
    if (!prompt) {
      setStatus("Enter a prompt", "error");
      return;
    }
    if (prompt.length > MAX_PROMPT) {
      setStatus(`Prompt too long (max ${MAX_PROMPT})`, "error");
      return;
    }
    if (!DURATIONS.includes(duration)) {
      setStatus("Duration must be 2, 3, 5, 10, 15, or 30", "error");
      return;
    }
    if (!SCALES.includes(scale)) {
      setStatus("Scale must be 1×, 2×, or 4×", "error");
      return;
    }
    if (backendMode(mode) === "generate" && !scaleReadyForMode(scale, "generate")) {
      setStatus(`${scale}× Generate is not wired on this Home`, "error");
      return;
    }
    if (mode === "character") {
      /* Character/Ref2VA is not a model offer yet — fail honest, not silently
         through the video offer. */
      setStatus("Character runs as a model offer in a later phase", "error");
      return;
    }
    startJob({ prompt, duration, mode: "generate", scale });
  });

  syncModeUi();
}

export function renderStudioPage() {
  bindAgentStudio();
  const panel = panelEl();
  if (!panel) {
    return;
  }
  refreshCreativeStatus();
  refreshLibrary();
  syncModeUi();
  if (!activeJobId) {
    const status = panel.querySelector("[data-studio-status]");
    if (status && !status.textContent) {
      setStatus("", "idle");
    }
  }
}
