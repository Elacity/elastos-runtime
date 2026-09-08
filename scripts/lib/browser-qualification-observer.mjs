// Observation only: product media remains the Browser's WebRTC receiver.
import { requireEvidence, MODES, deriveMedia, TERMINAL_EFFECTS } from "../browser-qualification-audit.mjs";

export function qualificationOptions(env) {
  if (!env.HOME_VIRTUAL_AUTH_BROWSER_QUALIFICATION_MODE) return null;
  const mode = env.HOME_VIRTUAL_AUTH_BROWSER_QUALIFICATION_MODE;
  const duration_ms = Number(env.HOME_VIRTUAL_AUTH_BROWSER_QUALIFICATION_DURATION_MS || 0);
  requireEvidence(MODES.includes(mode) && Number.isInteger(duration_ms) && duration_ms >= 0 &&
    duration_ms <= 28_800_000 && (["media", "mixed"].includes(mode) ? duration_ms >= 1000 : duration_ms === 0),
  "qualification_options_invalid");
  return { mode, duration_ms };
}
export function createQualificationCancellation(enabled, messages = process) {
  if (!enabled) return null;
  const controller = new AbortController(); let context, closing, runtimeCleanup, runtimeClosing;
  const evidence = { cancelled: false, context_closed: false };
  const close = () => closing ||= observationCall(() => context?.close(), new AbortController().signal, 3000)
    .then(() => { evidence.context_closed = Boolean(context); }, e => { evidence.close_error = e.message; });
  const settleRuntime = () => {
    if (!context || !runtimeCleanup) return;
    return runtimeClosing ||= (async () => {
      await close();
      if (!evidence.context_closed || evidence.close_error) {
        evidence.runtime_cleanup = { ok: false, error: "viewer_disposal_unconfirmed" }; return;
      }
      // Keep request capture installed through disposal and queued callbacks.
      await new Promise(setImmediate);
      let result;
      try { result = await runtimeCleanup(); } catch (e) { result = { ok: false, error: e.message }; }
      evidence.runtime_cleanup = evidence.late_dispatch_after_disposal ?
        { ...result, ok: false, error: "late_dispatch_after_disposal" } : result;
    })();
  };
  const cancel = message => {
    if (message?.schema !== "elastos.browser.qualification-cancel/v1") return;
    evidence.cancelled = true; controller.abort(); if (context) void close();
    void settleRuntime();
  };
  messages.on("message", cancel);
  return { signal: controller.signal, evidence,
    dispatchObserved() {
      if (controller.signal.aborted && evidence.context_closed) {
        evidence.late_dispatch_after_disposal = true;
        evidence.runtime_cleanup = { ok: false, error: "late_dispatch_after_disposal" };
      }
    },
    async drainCancellation() { if (controller.signal.aborted) await settleRuntime(); },
    setRuntimeCleanup(action) { runtimeCleanup = action; if (controller.signal.aborted) cancel({ schema: "elastos.browser.qualification-cancel/v1" }); },
    check() { requireEvidence(!controller.signal.aborted, "qualification_cancelled"); },
    async ownContext(pending) {
      const acquired = Promise.resolve(pending).then(value => {
        context = value; if (controller.signal.aborted) void close(); return value;
      });
      return observationCall(() => acquired, controller.signal, 60_000);
    },
    fetch(url, options = {}) {
      // Revocation is an owned cleanup effect; all work requests stop on abort.
      if (options.method === "DELETE" && /^\/api\/apps\/browser\/pages\/[^/]+\/operator-requests\/[^/]+$/.test(new URL(url).pathname)) return fetch(url, options);
      return fetch(url, { ...options, signal: AbortSignal.any([controller.signal, ...(options.signal ? [options.signal] : [])]) });
    },
    async stop() {
      messages.off("message", cancel); if (context) await close();
      if (controller.signal.aborted) await settleRuntime();
      return evidence;
    },
  };
}
export async function cleanupQualificationOpens(attempts, { fetchImpl = fetch, timeoutMs = 30_000, pollMs = 250 } = {}) {
  const signal = AbortSignal.timeout(timeoutMs), results = [];
  const get = async (url, token, body) => observationCall(async () => {
    const response = await fetchImpl(url, { method: body ? "POST" : "GET", redirect: "error",
      signal: AbortSignal.any([signal, AbortSignal.timeout(3000)]),
      headers: { Origin: "null", "x-elastos-home-token": token, ...(body ? { "content-type": "application/json" } : {}) },
      ...(body ? { body: JSON.stringify(body) } : {}) });
    const chunks = []; let size = 0;
    for await (const chunk of response.body) {
      size += chunk.length; requireEvidence(size <= 65536, "cancel_cleanup_response_bound"); chunks.push(chunk);
    }
    return { status: response.status, body: JSON.parse(Buffer.concat(chunks).toString()) };
  }, signal, Math.min(3000, timeoutMs));
  try {
    for (const attempt of attempts) {
      const owner = attempt.owner;
      requireEvidence(owner?.token && owner.instance && new URL(owner.origin).hostname === "localhost",
        "cancel_cleanup_owner_unavailable");
      let settled = attempt.outcome === "completed", closed = false;
      for (;;) {
        requireEvidence(!signal.aborted, "cancel_cleanup_deadline");
        if (!settled && attempt.open_id) {
          const status = await get(owner.origin + "/api/apps/browser/open/" + encodeURIComponent(attempt.open_id), owner.token);
          settled = status.body.status === "completed" || ["terminal_pre_effect_failure", "terminal_post_effect_cleanup"]
            .includes(status.body.error?.outcome?.state);
        }
        const summary = await get(owner.origin + "/api/apps/browser/summary?browser_instance=" + encodeURIComponent(owner.instance), owner.token);
        const sessions = summary.body.sessions, page = sessions?.recoverable_page;
        requireEvidence(summary.status === 200 && sessions?.schema === "elastos.browser.session-capacity/v1", "cancel_cleanup_summary_invalid");
        if (page) {
          requireEvidence(page.schema === "elastos.browser.recoverable-page/v1" && page.page_id && page.cleanup?.id,
            "cancel_cleanup_handle_missing");
          const receipt = await get(owner.origin + "/api/apps/browser/pages/" + encodeURIComponent(page.page_id) + "/close", owner.token,
            { schema: "elastos.browser.close-request/v2", cleanup_id: page.cleanup.id, browser_instance: owner.instance });
          if (receipt.body.closed === true) {
            requireEvidence(receipt.body.page_id === page.page_id && receipt.body.cleanup_id === page.cleanup.id &&
              TERMINAL_EFFECTS.every(k => receipt.body.terminal_effects?.[k] === true), "cancel_cleanup_terminal_mismatch");
            results.push(receipt.body); closed = true;
          }
        } else if ((settled || closed) && ["active_sessions", "principal_sessions", "total_sessions", "launching_sessions",
          "engine_cleanup_obligations", "launch_reconciliation_obligations"].every(k => sessions[k] === 0)) break;
        await delay(pollMs, signal);
      }
    }
    return { ok: true, results };
  } catch (e) { return { ok: false, error: signal.aborted ? "cancel_cleanup_deadline" : e.message, results }; }
}

// Self-contained because Playwright installs this function in each new document.
export function installQualificationReceiver() {
  if (!new URL(location.href).searchParams.get("browser_instance")) return;
  const audioElements = [], NativeAudio = window.Audio;
  window.Audio = new Proxy(NativeAudio, { construct(target, args) {
    const audio = Reflect.construct(target, args); audioElements.push(audio);
    if (audioElements.length > 4) audioElements.shift(); return audio;
  } });
  let detected = false, firstCallback;
  const detect = setInterval(() => {
    const video = document.querySelector("#browser-remote-display");
    if (detected || !video || !video.requestVideoFrameCallback) return;
    detected = true;
    const first = () => {
      if (!window.__elastosBrowserCurrentPageId || !video.videoWidth || video.paused ||
        document.querySelector("#browser-url")?.disabled) {
        firstCallback = video.requestVideoFrameCallback(first); return;
      }
      clearInterval(detect);
      void window.__reportBrowserQualificationFrame?.({
        page_id: window.__elastosBrowserCurrentPageId,
        width: video.videoWidth, height: video.videoHeight,
      });
    };
    firstCallback = video.requestVideoFrameCallback(first);
  }, 50);
  let state;
  window.__browserQualification = {
    async start() {
      if (state) throw new Error("qualification_probe_already_started");
      const active = audioElements.filter(a => a.srcObject?.getAudioTracks().some(t => t.readyState === "live"));
      const video = document.querySelector("#browser-remote-display");
      if (active.length !== 1 || !video?.requestVideoFrameCallback) throw new Error("qualification_product_receiver_missing");
      const audio = active[0], stream = audio.srcObject, track = stream.getAudioTracks()[0];
      const context = new AudioContext();
      state = { audio, stream, track, video, video_stream: video.srcObject, context, start: 0, active_ms: 0, interaction_ms: 0,
        phase: "active", phase_start: 0, frames: 0, max_frame_gap_ms: 0, callback: null,
        last_frame: null, audio_stats: null, worklet: null, source: null, stopped: false,
        snapshot_id: 0, snapshot_pending: null };
      try {
        await context.resume();
        if (state.stopped) throw new Error("qualification_probe_stopped");
        const source = `
          class QualificationMeter extends AudioWorkletProcessor {
            constructor() {
              super(); this.enabled = true; this.reset();
              this.port.onmessage = e => { if (e.data?.snapshot) this.snapshot(e.data.snapshot);
                else if (e.data === "reset") this.reset();
                else { this.enabled = e.data === "active"; this.silence = 0; } };
            }
            reset() { this.frames=0; this.crossings=0; this.previous=0; this.silence=0; this.maxSilence=0; this.invalid=0; this.ticks=0; this.maxBlock=0; }
            snapshot(request_id=0) { this.port.postMessage({ request_id,
              frames:this.frames, sample_rate:sampleRate, max_silence_ms:this.maxSilence,
              crossings:this.crossings, invalid:this.invalid, max_block_frames:this.maxBlock }); }
            process(inputs) {
              const data = inputs[0]?.[0];
              if (this.enabled) {
                const length = data?.length || 128; let energy=0;
                this.maxBlock = Math.max(this.maxBlock,length);
                if (data) for (const value of data) {
                  if (!Number.isFinite(value)) this.invalid++;
                  energy += value*value;
                  if (this.previous <= 0 && value > 0) this.crossings++;
                  this.previous=value;
                }
                this.frames += length;
                this.silence = Math.sqrt(energy/length) > 0.003 ? 0 : this.silence + length*1000/sampleRate;
                this.maxSilence = Math.max(this.maxSilence,this.silence);
              }
              if (++this.ticks % 32 === 0) this.snapshot();
              return true;
            }
          }
          registerProcessor("qualification-meter",QualificationMeter);
        `;
        const url = URL.createObjectURL(new Blob([source], { type: "text/javascript" }));
        try { await context.audioWorklet.addModule(url); } finally { URL.revokeObjectURL(url); }
        if (state.stopped) throw new Error("qualification_probe_stopped");
        state.worklet = new AudioWorkletNode(context, "qualification-meter");
        state.worklet.port.onmessage = event => {
          state.audio_stats = event.data;
          if (state.snapshot_pending && event.data.request_id === state.snapshot_id) {
            clearTimeout(state.snapshot_pending.timer); state.snapshot_pending.resolve(); state.snapshot_pending = null;
          }
        };
        state.source = context.createMediaStreamSource(stream);
        // Worklet output stays silent. The product Audio element owns playback.
        state.source.connect(state.worklet).connect(context.destination);
        state.start = state.phase_start = performance.now();
        state.last_frame = state.start;
        state.active_total = state.active_dropped = 0;
        state.initial_quality = video.getVideoPlaybackQuality();
        const frame = (now, metadata) => {
          if (state.stopped) return;
          if (state.phase !== "hidden") {
            if (state.last_frame != null) state.max_frame_gap_ms = Math.max(state.max_frame_gap_ms, now - state.last_frame);
            state.last_frame = now; state.frames++;
            state.presented = metadata.presentedFrames;
          }
          state.callback = video.requestVideoFrameCallback(frame);
        };
        state.callback = video.requestVideoFrameCallback(frame);
        return { started: true, page_id: window.__elastosBrowserCurrentPageId };
      } catch (e) { await window.__browserQualification.stop(); throw e; }
    },
    phase(value) {
      if (!state || !["active", "idle", "hidden"].includes(value)) throw new Error("qualification_phase_invalid");
      const now = performance.now();
      const quality = state.video.getVideoPlaybackQuality();
      if (state.phase !== "hidden") {
        state.active_total += quality.totalVideoFrames - state.initial_quality.totalVideoFrames;
        state.active_dropped += quality.droppedVideoFrames - state.initial_quality.droppedVideoFrames;
      }
      state.initial_quality = quality;
      if (state.phase !== "hidden") state.active_ms += now - state.phase_start;
      if (state.phase === "active") state.interaction_ms += now - state.phase_start;
      state.phase = value; state.phase_start = now; state.last_frame = value === "hidden" ? null : now;
      state.worklet.port.postMessage(value === "hidden" ? "hidden" : "active");
    },
    async read() {
      if (!state || state.stopped) throw new Error("qualification_probe_inactive");
      if (state.snapshot_pending) throw new Error("qualification_snapshot_already_pending");
      await new Promise((resolve, reject) => {
        const timer = setTimeout(() => { state.snapshot_pending = null; reject(new Error("qualification_audio_snapshot_deadline")); }, 1000);
        state.snapshot_pending = { resolve, reject, timer };
        state.worklet.port.postMessage({ snapshot: ++state.snapshot_id });
      });
      const s = state, now = performance.now(), a = s.audio_stats;
      const active_ms = s.active_ms + (s.phase !== "hidden" ? now - s.phase_start : 0);
      const interaction_ms = s.interaction_ms + (s.phase === "active" ? now - s.phase_start : 0);
      const quality = s.video.getVideoPlaybackQuality();
      const total = s.active_total + (s.phase === "hidden" ? 0 : quality.totalVideoFrames - s.initial_quality.totalVideoFrames);
      const dropped = s.active_dropped + (s.phase === "hidden" ? 0 : quality.droppedVideoFrames - s.initial_quality.droppedVideoFrames);
      const unchanged = s.audio.srcObject === s.stream && s.track.readyState === "live" &&
        document.querySelector("#browser-remote-display") === s.video && s.video.srcObject === s.video_stream &&
        s.video_stream?.getVideoTracks().every(t => t.readyState === "live");
      return { duration_ms: now - s.start, active_ms, interaction_ms, phase: s.phase, phase_started_ms: s.phase_start - s.start,
        page_id: window.__elastosBrowserCurrentPageId, document_id: performance.timeOrigin,
        hidden: document.hidden, width: s.video.videoWidth, height: s.video.videoHeight,
        engine_id: document.querySelector("#browser-engine")?.value,
        exit_id: document.querySelector("#browser-exit")?.value,
        receiver_unchanged: unchanged && !s.audio.muted && !s.audio.paused && s.context.state === "running",
        video: { frames: s.frames, fps: active_ms > 0 ? s.frames * 1000 / active_ms : 0,
          max_frame_gap_ms: Math.max(s.max_frame_gap_ms, s.phase !== "hidden" && s.last_frame != null ? now - s.last_frame : 0),
          drop_ratio: total > 0 ? dropped / total : 0, dropped, total,
          width: s.video.videoWidth, height: s.video.videoHeight },
        audio: { observed_ms: a ? a.frames * 1000 / a.sample_rate : 0,
          max_silence_ms: a ? a.max_silence_ms + (a.max_silence_ms > 0 ? 2 * a.max_block_frames * 1000 / a.sample_rate : 0) : null,
          resolution_ms: a ? 2 * a.max_block_frames * 1000 / a.sample_rate : null, sample_rate: a?.sample_rate ?? null,
          tone_hz: a?.frames > 0 ? a.crossings * a.sample_rate / a.frames : null,
          invalid_samples: a?.invalid ?? null, receiver_unchanged: unchanged } };
    },
    async stop() {
      clearInterval(detect);
      const video = document.querySelector("#browser-remote-display");
      if (firstCallback !== undefined) video?.cancelVideoFrameCallback(firstCallback);
      if (!state) return { observer_stopped: true, probe_closed: true };
      if (state.stop_promise) return state.stop_promise;
      state.stop_promise = (async () => {
      state.stopped = true;
      if (state.snapshot_pending) {
        clearTimeout(state.snapshot_pending.timer); state.snapshot_pending.reject(new Error("qualification_probe_stopped"));
        state.snapshot_pending = null;
      }
      if (state.callback !== null) state.video.cancelVideoFrameCallback(state.callback);
      state.source?.disconnect(); state.worklet?.disconnect();
      if (state.worklet) { state.worklet.port.onmessage = null; state.worklet.port.close(); }
      await state.context.close();
      return { observer_stopped: true, probe_closed: state.context.state === "closed" };
      })();
      return state.stop_promise;
    },
  };
}
const delay = (ms, signal) => new Promise((resolve, reject) => {
  const finish = () => { clearTimeout(timer); signal.removeEventListener("abort", abort); resolve(); };
  const abort = () => { clearTimeout(timer); signal.removeEventListener("abort", abort); reject(new Error("qualification_cancelled")); };
  const timer = setTimeout(finish, ms);
  signal.addEventListener("abort", abort, { once: true }); if (signal.aborted) abort();
});
export async function observationCall(action, signal, ms = 3000) {
  let timer, abort;
  try {
    requireEvidence(!signal.aborted, "qualification_cancelled");
    return await Promise.race([Promise.resolve().then(action), new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error("qualification_observation_deadline")), ms);
      abort = () => reject(new Error("qualification_cancelled"));
      signal.addEventListener("abort", abort, { once: true });
    })]);
  } finally { clearTimeout(timer); if (abort) signal.removeEventListener("abort", abort); }
}
export async function qualificationInteraction({ index, pageId, run, text, readReceipt, key, wheel,
  signal, timeoutMs, now = () => performance.now() }) {
  const deadline = now() + timeoutMs;
  const budget = () => {
    requireEvidence(!signal.aborted && now() < deadline, "qualification_input_deadline");
    return { timeout: Math.max(1, Math.ceil(deadline - now())) };
  };
  return observationCall(async () => {
    const initial = await readReceipt({ signal });
    requireEvidence(initial.run === run, "qualification_input_run_changed");
    const sequence = initial.events.at(-1)?.sequence || 0;
    const before = [...initial.events].reverse().find(e => e.type === "scroll");
    requireEvidence(before && Number.isFinite(before.scroll_y), "qualification_scroll_baseline_missing");
    const expected = text.slice(0, -1) + (index % 2 ? "B" : "A"), delta = index % 2 ? 320 : -320;
    const waitEvent = async predicate => {
      for (;;) {
        budget(); const receipt = await readReceipt({ signal });
        requireEvidence(receipt.run === run, "qualification_input_run_changed");
        const event = receipt.events.find(e => e.page === "nav" && predicate(e));
        if (event) return event;
        await delay(Math.min(50, budget().timeout), signal);
      }
    };
    const started = now();
    await key("press", "End", budget()); await key("press", "Backspace", budget());
    await key("pressSequentially", expected.at(-1), budget());
    const input = await waitEvent(e => e.sequence > sequence && e.type === "input" && e.value === expected);
    const inputAt = now();
    await wheel(delta, budget);
    const scroll = await waitEvent(e => e.sequence > input.sequence && e.type === "scroll" &&
      e.value === expected && (e.scroll_y - before.scroll_y) * Math.sign(delta) >= 100);
    return { index, page_id: pageId, run, expected,
      input: { sequence: input.sequence, started_ms: started, receipt_ms: inputAt },
      scroll: { sequence: scroll.sequence, before_y: before.scroll_y, delta, started_ms: inputAt, receipt_ms: now() },
      visible_response: null, pending: "input_to_visible_latency_measurement" };
  }, signal, timeoutMs);
}
export async function createQualificationHarness(context, page, options, now = () => performance.now(), cancellation = null) {
  if (!options) return null;
  const controller = new AbortController(), opens = new Map(), frames = new Map(), launches = new Map(), attempts = [];
  const cancel = message => { if (message?.schema === "elastos.browser.qualification-cancel/v1") controller.abort(); };
  const match = frame => {
    const opened = opens.get(frame), observed = frames.get(frame);
    if (opened?.page_id && observed?.page_id === opened.page_id && observed.at > opened.start) {
      launches.set(opened.page_id, { page_id: opened.page_id, clock: "host_monotonic",
        usable_frame_ms: observed.at - attempts[0].start, width: observed.width, height: observed.height,
        method: "Runtime open request to first usable product video callback; host observation upper bound" });
    }
  };
  const request = req => {
    if (req.method() !== "POST" || !/\/api\/apps\/browser\/open$/.test(new URL(req.url()).pathname)) return;
    cancellation?.dispatchObserved();
    if (!opens.has(req.frame()) && opens.size >= 16) { controller.abort(); return; }
    if (attempts.length >= 16) { controller.abort(); return; }
    const previous = opens.get(req.frame());
    if (previous?.outcome === "pending") previous.outcome = "interrupted_by_retry";
    const opened = { start: now(), request: req, page_id: null, open_id: null, outcome: "pending" };
    if (cancellation) {
      try { opened.owner = { origin: new URL(req.url()).origin, token: req.headers()["x-elastos-home-token"],
        instance: req.postDataJSON()?.browser_instance }; } catch {}
    }
    attempts.push(opened); opens.set(req.frame(), opened);
  };
  const observeResponse = async res => {
    const path = new URL(res.url()).pathname;
    if (!/\/api\/apps\/browser\/open(?:\/[^/]+)?$/.test(path)) return;
    const frame = res.request().frame(), opened = path.endsWith("/open") ?
      attempts.find(a => a.request === res.request()) :
      attempts.find(a => a.open_id === decodeURIComponent(path.split("/").at(-1)));
    if (!opened) return;
    if (path.endsWith("/open") ? res.request() !== opened.request :
      !opened.open_id || decodeURIComponent(path.split("/").at(-1)) !== opened.open_id) return;
    const body = await res.json().catch(() => null);
    if (res.request() === opened.request && typeof body?.open_id === "string") opened.open_id = body.open_id;
    const result = body?.schema === "elastos.browser.open-result/v1" ? body :
      body?.schema === "elastos.browser.open-status/v1" && body.status === "completed" ? body.result : null;
    if (body?.status === "failed" || res.status?.() >= 400) opened.outcome = "failed";
    if (result?.engine_page?.page_id) {
      opened.outcome = "completed"; opened.page_id = result.engine_page.page_id; match(frame);
    }
  };
  const response = res => { void observeResponse(res).catch(() => controller.abort()); };
  await context.exposeBinding("__reportBrowserQualificationFrame", ({ frame }, value) => {
    requireEvidence(value && typeof value.page_id === "string" && frames.size < 16, "frame_observation_invalid");
    frames.set(frame, { ...value, at: now() }); match(frame);
  });
  await context.addInitScript(installQualificationReceiver);
  process.on("message", cancel);
  page.on("request", request); page.on("response", response);
  cancellation?.setRuntimeCleanup(() => cleanupQualificationOpens(attempts));
  const openSnapshot = pageId => ({ schema: "elastos.browser.qualification-observation/v1", mode: options.mode,
    launch: launches.get(pageId) || null, observation: null,
    open_attempts: attempts.map(a => ({ open_id: a.open_id, page_id: a.page_id, outcome: a.outcome,
      started_ms: a.start - attempts[0].start })) });
  return {
    snapshot: openSnapshot,
    async observe({ appFrame, pageId, readReceipt, readStatus, interact }) {
      const evidence = openSnapshot(pageId);
      if (attempts.length !== 1 || attempts[0].outcome !== "completed") {
        throw Object.assign(new Error("qualification_open_retry_or_failure"), { qualification: evidence });
      }
      if (["cold", "warm"].includes(options.mode)) requireEvidence(evidence.launch, "qualification_launch_not_observed");
      evidence.services = await observationCall(() => appFrame.evaluate(() => ({
        engine_id: document.querySelector("#browser-engine")?.value,
        exit_id: document.querySelector("#browser-exit")?.value,
      })), controller.signal);
      if (!options.duration_ms) return evidence;
      const o = { ok: false, samples: 0, interactions: 0, hidden_cycles: 0, idle_cycles: 0, max_sample_gap_ms: 0 };
      evidence.observation = o;
      let cover, phase = "active", cursor = 0, snapshot, nextInteraction = 0;
      const trace = [];
      const call = (action, ms) => observationCall(action, controller.signal, ms);
      const initialReceipt = await call(() => readReceipt({ signal: controller.signal }));
      requireEvidence(initialReceipt.observation === "bounded-v1", "qualification_fixture_contract_required");
      cursor = initialReceipt.events.at(-1)?.sequence || 0;
      try {
        const result = await call(() => appFrame.evaluate(() => window.__browserQualification.start()));
        requireEvidence(result.page_id === pageId, "qualification_page_changed");
        const started = performance.now(); let nextSample = started, priorCapture = started;
        let documentId;
        const { engine_id: engineId, exit_id: exitId } = evidence.services;
        const initialCursor = cursor;
        const capture = async interaction => {
          snapshot = await call(() => appFrame.evaluate(() => window.__browserQualification.read()));
          documentId ??= snapshot.document_id;
          requireEvidence(snapshot.page_id === pageId && snapshot.document_id === documentId &&
            snapshot.engine_id === engineId && snapshot.exit_id === exitId && snapshot.receiver_unchanged &&
            (phase !== "hidden" || snapshot.hidden), "qualification_receiver_or_binding_changed");
          const status = await call(readStatus);
          requireEvidence(status.ok && status.body?.direct_network === false &&
            status.body?.display_session?.media_transport === "runtime_relay", "qualification_runtime_route_changed");
          const receipt = await call(() => readReceipt({ signal: controller.signal }));
          const events = receipt.events.filter(e => e.sequence > cursor);
          requireEvidence(receipt.run === initialReceipt.run && (!events.length || events[0].sequence === cursor + 1),
            "qualification_fixture_event_gap");
          if (events.length) cursor = events.at(-1).sequence;
          const row = { schema: "elastos.browser.qualification-sample/v1", page_id: pageId, run: initialReceipt.run,
            snapshot, events, fixture_cursor: cursor, initial_cursor: initialCursor,
            action_window: { started_ms: priorCapture, observed_ms: performance.now() }, ...(interaction ? { interaction } : {}) };
          priorCapture = row.action_window.observed_ms;
          requireEvidence(trace.length < 6000, "media_trace_bound"); trace.push(row);
          if (process.connected) process.send(row);
        };
        for (;;) {
          requireEvidence(!controller.signal.aborted, "qualification_cancelled");
          const elapsed = performance.now() - started;
          if (elapsed >= options.duration_ms) break;
          const cycle = elapsed % 300_000;
          const nextPhase = options.mode !== "mixed" ? "active" : cycle >= 270_000 ? "hidden" : cycle >= 240_000 ? "idle" : "active";
          if (nextPhase !== phase) {
            await call(() => appFrame.evaluate(value => window.__browserQualification.phase(value), nextPhase));
            if (nextPhase === "hidden") {
              cover = await call(() => context.newPage()); await cover.goto("about:blank", { timeout: 3000 });
              await call(() => cover.bringToFront());
            } else if (cover) { await call(() => page.bringToFront()); await call(() => cover.close()); cover = null; }
            phase = nextPhase;
          }
          let interaction;
          if (phase === "active" && elapsed >= nextInteraction) {
            interaction = await call(() => interact(o.interactions, { signal: controller.signal, timeoutMs: 5000 }), 5000);
            o.interactions++; nextInteraction = elapsed + 60_000;
          }
          await capture(interaction);
          nextSample += 5000;
          await delay(Math.max(0, Math.min(nextSample, started + options.duration_ms) - performance.now()), controller.signal);
        }
        await capture();
        Object.assign(o, deriveMedia(trace, { page_id: pageId, run: initialReceipt.run, engine_id: engineId, exit_id: exitId, mode: options.mode }));
        o.ok = true;
      } catch (error) { error.qualification = evidence; throw error; }
      finally {
        const cleanup = action => observationCall(action, new AbortController().signal);
        if (cover) { await cleanup(() => page.bringToFront()).catch(() => {}); await cleanup(() => cover.close()).catch(() => {}); }
        o.cleanup = await cleanup(() => appFrame.evaluate(() => window.__browserQualification.stop())).catch(() =>
          ({ observer_stopped: false, probe_closed: false }));
      }
      return evidence;
    },
    async stop() {
      await cancellation?.drainCancellation();
      page.off("request", request); page.off("response", response); process.removeListener("message", cancel);
      controller.abort();
    },
  };
}
