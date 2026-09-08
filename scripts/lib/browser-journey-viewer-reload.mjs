import { createHash } from "node:crypto";
import { browserJourneyBinding, browserJourneyVideo } from "./browser-journey-recovery.mjs";

const WINDOW_MS = 5000, POLL_MS = 250, SUFFIX = "-reload";
const hash = value => `sha256:${createHash("sha256").update(String(value)).digest("hex").slice(0, 16)}`;
const text = value => typeof value === "string" && value.length > 0 && value.length <= 512;
const count = value => Number.isSafeInteger(value) && value >= 0;
const documentId = value => Number.isFinite(value) && value > 0;
class ReloadFailure extends Error {}
function requireEvidence(ok, code) { if (!ok) throw new ReloadFailure(code); }
function binding(raw) {
  try { return browserJourneyBinding(raw); }
  catch { throw new ReloadFailure("binding_unavailable_or_cleanup"); }
}
function video(raw, bytesRequired) {
  try { return browserJourneyVideo(raw, bytesRequired); }
  catch { throw new ReloadFailure("video_metrics_unavailable"); }
}
function receipt(raw) {
  requireEvidence(raw?.schema === "elastos.browser.journey-receipt/v1" && text(raw.run) &&
    Array.isArray(raw.events) && raw.events.length <= 128 && raw.events.every((event, i, events) =>
      count(event?.sequence) && event.sequence > 0 && (i === 0 || event.sequence > events[i - 1].sequence)),
  "receipt_invalid");
  return raw;
}

/**
 * Opt-in existing-iframe reload proof. Runtime owns the retained page throughout.
 * readState returns { sessions, page_status, viewer: null | { page_id,
 * browser_instance, actual_url, engine_id, exit_id, document_id: performance.timeOrigin },
 * video: null | browserRemoteVideoMetrics }. Read fresh Runtime status independently
 * of viewer readiness; read document_id and video together from the same document.
 * observeRequests(record, budget) returns a listener removal/flush function. Events
 * use the recovery observer shape, plus { kind: "navigation", phase: "commit",
 * source_matches: true, document_generation }. Only hashes and bounded metrics survive.
 * Callbacks receive { signal, deadlineMs, timeoutMs } and must honor cancellation.
 * Observer setup owns removal if it fails before returning its stop function.
 */
export async function diagnoseBrowserViewerReload({ expectedUrl, readReceipt, readState,
  reloadViewer, extendInput, observeRequests,
  clock = { now: () => performance.now(), setTimeout, clearTimeout },
}) {
  const started = clock.now();
  const evidence = { schema: "elastos.browser.journey-viewer-reload/v1", ok: false,
    requests: [], samples: [], dropped_requests: 0, observer: { stopped: false } };
  let phase = "baseline", stop, failure, forbidden = false, original, initialViewer;
  let newDocument, newVideo, bytesRequired, initialReceipt, input, receiptSequence;
  const interruptions = new Set();
  const elapsed = () => Math.round(clock.now() - started);
  async function within(deadlineMs, code, action, finishing = false) {
    if (!finishing) requireEvidence(!forbidden, "page_open_or_close_observed");
    const timeoutMs = deadlineMs - clock.now();
    requireEvidence(timeoutMs > 0, code);
    const controller = new AbortController();
    let timer, interrupt;
    try {
      const limit = new Promise((_, reject) => {
        interrupt = () => { controller.abort(); reject(new ReloadFailure("page_open_or_close_observed")); };
        if (!finishing) interruptions.add(interrupt);
        timer = clock.setTimeout(() => { controller.abort(); reject(new ReloadFailure(code)); }, timeoutMs);
      });
      const result = await Promise.race([limit, Promise.resolve().then(() =>
        action({ signal: controller.signal, deadlineMs, timeoutMs }))]);
      requireEvidence(clock.now() <= deadlineMs, code);
      return result;
    } catch (error) {
      throw error instanceof ReloadFailure ? error : new ReloadFailure(code.replace(/_deadline$/, "_failed"));
    } finally {
      clock.clearTimeout(timer);
      interruptions.delete(interrupt);
    }
  }
  function record(event) {
    if (event?.source_matches !== true) return;
    const navigation = event.kind === "navigation" && event.phase === "commit";
    if (!navigation && (!["opening", "closing", "renewal", "probe", "status", "heartbeat", "signaling"].includes(event.kind) ||
      !["request", "response", "failed"].includes(event.phase))) return;
    if (["opening", "closing"].includes(event.kind)) {
      forbidden = true;
      for (const interrupt of interruptions) interrupt();
    }
    if (evidence.requests.length === 64) { evidence.dropped_requests++; return; }
    evidence.requests.push({ at_ms: elapsed(), phase, kind: event.kind, event: event.phase,
      ...browserViewerSignalMetadata(event),
      ...(text(event.request_id) ? { request_hash: hash(event.request_id) } : {}),
      ...(count(event.document_generation) ? { document_generation: event.document_generation } : {}),
      ...(count(event.status) && event.status <= 599 ? { status: event.status } : {}) });
  }
  const advanced = (after, before) => after?.visible && after.decoded_frames > before.decoded_frames &&
    (!bytesRequired || after.video_bytes_received > before.video_bytes_received);
  const matches = value => requireEvidence(value.every((item, i) => item === original[i]), "binding_changed");
  async function pause(deadline) {
    await within(deadline, phase === "baseline" ? "baseline_deadline" : "reload_deadline", ({ signal, timeoutMs }) =>
      new Promise(resolve => {
        const onAbort = () => { clock.clearTimeout(timer); resolve(); };
        const timer = clock.setTimeout(() => { signal.removeEventListener("abort", onAbort); resolve(); },
          Math.min(POLL_MS, timeoutMs));
        signal.addEventListener("abort", onAbort, { once: true });
      }));
  }
  async function sample(deadline) {
    const raw = await within(deadline, "state_deadline", readState);
    // The launch snapshot URL can be stale; the fresh status URL must stay exact.
    matches(binding({ ...initialViewer, sessions: raw?.sessions, page_status: raw?.page_status,
      page_id: raw?.sessions?.recoverable_page?.page_id, actual_url: raw?.page_status?.actual_url }));
    const viewer = raw?.viewer;
    let media = null;
    if (viewer) {
      requireEvidence(documentId(viewer.document_id), "viewer_document_invalid");
      if (phase === "baseline") requireEvidence(viewer.document_id === initialViewer.document_id, "baseline_document_changed");
      else if (viewer.document_id !== initialViewer.document_id) {
        if (newDocument === undefined) newDocument = viewer.document_id;
        requireEvidence(newDocument === viewer.document_id, "viewer_document_changed_again");
      } else requireEvidence(newDocument === undefined, "viewer_document_reverted");
      if (viewer.page_id) {
        matches(binding({ ...viewer, sessions: raw.sessions, page_status: raw.page_status }));
        if (raw.video && (phase === "baseline" || viewer.document_id === newDocument)) {
          media = video(raw.video, phase === "baseline" && bytesRequired);
          // A new peer can expose its video element before its first getStats result.
          // Validate reported counts now, but await missing stats within the reload budget.
          if (phase !== "baseline" && bytesRequired && !Object.hasOwn(media, "video_bytes_received")) media = null;
          else requireEvidence(Object.hasOwn(media, "video_bytes_received") === bytesRequired, "byte_metrics_changed");
        }
      }
    }
    evidence.samples.push({ at_ms: elapsed(), phase, binding_matches: true,
      viewer_ready: Boolean(media), viewer_has_page: Boolean(viewer?.page_id),
      ...(typeof viewer?.status_text === "string" ? { viewer_status: viewer.status_text.slice(0, 240) } : {}),
      video_present: raw?.video?.present === true,
      ...(count(raw?.video?.ready_state) ? { video_ready_state: raw.video.ready_state } : {}),
      ...(count(raw?.video?.decoded_frames) ? { video_decoded_frames: raw.video.decoded_frames } : {}),
      video_bytes_reported: count(raw?.video?.video_bytes_received),
      ...(viewer ? { document_hash: hash(viewer.document_id) } : {}),
      ...(media || {}) });
    if (evidence.samples.length > 32) evidence.samples.shift();
    return media;
  }
  async function currentReceipt(deadline) {
    const value = receipt(await within(deadline, "receipt_deadline", readReceipt));
    requireEvidence(value.run === initialReceipt.run && value.events.at(-1)?.sequence >= receiptSequence,
      "receipt_changed_or_stale");
    requireEvidence(!value.events.some(event => event.sequence > receiptSequence && event.type === "load"),
      "guest_page_reloaded");
    const latest = value.events.findLast(event => event.type === "input");
    requireEvidence(latest?.page === input.page && text(latest.value), "input_receipt_invalid");
    return latest;
  }
  try {
    const baselineDeadline = clock.now() + WINDOW_MS;
    // Collect a successfully installed observer even if it reports a forbidden event during setup.
    stop = await within(baselineDeadline, "observer_deadline", budget => observeRequests(record, budget), true);
    requireEvidence(typeof stop === "function", "observer_missing");
    const raw = await within(baselineDeadline, "state_deadline", readState);
    initialViewer = raw?.viewer;
    requireEvidence(initialViewer && documentId(initialViewer.document_id), "baseline_viewer_missing");
    original = binding({ ...initialViewer, sessions: raw.sessions, page_status: raw.page_status });
    evidence.binding_hashes = original.map(hash);
    evidence.initial_document_hash = hash(initialViewer.document_id);
    requireEvidence(text(expectedUrl) && initialViewer.actual_url === expectedUrl, "fixture_url_mismatch");
    initialReceipt = receipt(await within(baselineDeadline, "receipt_deadline", readReceipt));
    input = initialReceipt.events.findLast(event => event.type === "input");
    receiptSequence = initialReceipt.events.at(-1)?.sequence;
    requireEvidence(input && ["main", "nav"].includes(input.page) && text(input.value) &&
      input.value.length + SUFFIX.length <= 96, "current_input_missing");
    let url;
    try { url = new URL(expectedUrl); } catch { throw new ReloadFailure("fixture_url_mismatch"); }
    requireEvidence(["http:", "https:"].includes(url.protocol) && url.searchParams.getAll("run").length === 1 &&
      url.searchParams.get("run") === initialReceipt.run && url.pathname === `/${input.page}`, "fixture_run_mismatch");
    const initialVideo = video(raw.video);
    bytesRequired = Object.hasOwn(initialVideo, "video_bytes_received");
    requireEvidence(initialVideo.visible && initialVideo.decoded_frames > 0, "baseline_video_not_visible");
    let current;
    do { await pause(baselineDeadline); current = await sample(baselineDeadline); }
    while (!advanced(current, initialVideo));

    phase = "reload";
    const reloadStarted = clock.now(), deadline = reloadStarted + WINDOW_MS;
    evidence.reload_started_ms = elapsed();
    await within(deadline, "reload_deadline", reloadViewer);
    while (true) {
      current = await sample(deadline);
      const beforeInput = await currentReceipt(deadline);
      requireEvidence(beforeInput.value === input.value, "input_changed_before_extension");
      if (current?.visible) {
        if (newVideo === undefined) newVideo = current;
        else if (advanced(current, newVideo)) break;
      }
      await pause(deadline);
    }
    evidence.new_document_hash = hash(newDocument);
    const preInputVideo = current;
    phase = "input";
    await within(deadline, "input_deadline", budget => extendInput(SUFFIX, budget));
    while (true) {
      current = await sample(deadline);
      const latest = await currentReceipt(deadline);
      const expected = input.value + SUFFIX;
      requireEvidence(expected.startsWith(latest.value), "input_receipt_mismatch");
      if (latest.sequence > receiptSequence && latest.value === expected && advanced(current, preInputVideo)) {
        evidence.input = { before_length: input.value.length, after_length: latest.value.length, sequence: latest.sequence };
        evidence.reload_ms = clock.now() - reloadStarted;
        requireEvidence(evidence.reload_ms <= WINDOW_MS, "reload_deadline");
        break;
      }
      await pause(deadline);
    }
  } catch (error) {
    failure = error instanceof ReloadFailure ? error.message : "reload_diagnostic_failed";
  } finally {
    if (typeof stop === "function") {
      try {
        await within(clock.now() + WINDOW_MS, "observer_stop_deadline", budget => stop(budget), true);
        evidence.observer.stopped = true;
      } catch { evidence.observer.failure = "observer_stop_failed"; failure ||= "observer_stop_failed"; }
    }
  }
  if (forbidden) failure = "page_open_or_close_observed";
  evidence.ok = !failure;
  if (failure) {
    evidence.failure = failure;
    const error = new Error(`Browser viewer reload diagnostic failed: ${failure}`);
    error.evidence = evidence;
    throw error;
  }
  return evidence;
}


/** Read fresh peer counters and the document identity in the same viewer context. */
export async function readBrowserViewerReloadDocument() {
  const query = window.__elastosBrowserReadRemoteDisplayMetrics;
  const metrics = typeof query === "function" ? await query() : null;
  const element = document.querySelector("#browser-remote-display");
  const rect = element?.getBoundingClientRect();
  const bytes = metrics?.latestVideoWebrtcStats?.video_bytes_received ?? metrics?.latestWebrtcStats?.video_bytes_received;
  return {
    viewer: {
      page_id: window.__elastosBrowserCurrentPageId || "",
      browser_instance: new URL(location.href).searchParams.get("browser_instance"),
      actual_url: document.querySelector("#browser-url")?.value || "",
      engine_id: document.querySelector("#browser-engine")?.value,
      exit_id: document.querySelector("#browser-exit")?.value,
      document_id: performance.timeOrigin,
      status_text: (document.querySelector("#browser-status .browser-status-message")?.textContent || "").slice(0, 240),
    },
    video: element ? { present: true, hidden: element.hidden, paused: element.paused,
      ready_state: element.readyState, video_width: element.videoWidth, video_height: element.videoHeight,
      client_width: Math.round(rect.width), client_height: Math.round(rect.height),
      decoded_frames: Number(element.webkitDecodedFrameCount || 0),
      ...(Number.isSafeInteger(bytes) ? { video_bytes_received: bytes } : {}) } : null,
  };
}


/** Keep signaling stage evidence bounded; SDP, candidates and authority remain private. */
export function browserViewerSignalMetadata(payload) {
  const types = ["display_attach", "offer", "answer", "candidate", "end_of_candidates"];
  const codes = ["display_attach_busy", "display_generation_mismatch", "display_owner_changed",
    "display_attach_unsupported", "display_attach_failed", "display_attach_uncertain"];
  const type = payload?.signal_type ?? payload?.type;
  const code = payload?.error_code ?? payload?.code;
  return {
    ...(types.includes(type) ? { signal_type: type } : {}),
    ...(codes.includes(code) ? { error_code: code } : {}),
    ...(payload?.schema === "elastos.browser.display-attach-result/v1" || payload?.attached === true ? { attached: true } : {}),
  };
}
