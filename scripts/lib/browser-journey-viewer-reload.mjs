import { createHash } from "node:crypto";
import { browserJourneyBinding, browserJourneyVideo } from "./browser-journey-recovery.mjs";

const WINDOW_MS = 5000, BASELINE_MS = 10000, POLL_MS = 250, SUFFIX = "-reload";
function signalingCensus(status) {
  const signaling = status?.webrtc_signaling;
  const picked = {};
  if (signaling && typeof signaling === "object") {
    for (const key of [
      "browser_answers_received",
      "browser_candidates_received",
      "selkies_offers_received",
      "selkies_candidates_received",
      "pending_selkies_candidates",
    ]) {
      if (count(signaling[key])) picked[key] = signaling[key];
    }
  }
  if (typeof status?.ice_connection_state === "string") {
    picked.ice_connection_state = status.ice_connection_state.slice(0, 32);
  }
  if (typeof status?.webrtc_connection_state === "string") {
    picked.webrtc_connection_state = status.webrtc_connection_state.slice(0, 32);
  }
  return Object.keys(picked).length ? { engine_signaling: picked } : {};
}
const hash = value => `sha256:${createHash("sha256").update(String(value)).digest("hex").slice(0, 16)}`;
const text = value => typeof value === "string" && value.length > 0 && value.length <= 512;
const count = value => Number.isSafeInteger(value) && value >= 0;
function viewerFailureFields(viewer) {
  return {
    ...(typeof viewer?.status_text === "string" ? { viewer_status: viewer.status_text.slice(0, 240) } : {}),
    ...(text(viewer?.failure_stage) ? { viewer_failure_stage: viewer.failure_stage.slice(0, 64) } : {}),
    ...(text(viewer?.failure_message) ? { viewer_failure_message: viewer.failure_message.slice(0, 240) } : {}),
    ...(count(viewer?.failure_status) && viewer.failure_status >= 400 && viewer.failure_status <= 599
      ? { viewer_failure_status: viewer.failure_status } : {}),
  };
}
const documentId = value => Number.isFinite(value) && value > 0;
class ReloadFailure extends Error {}
function requireEvidence(ok, code) { if (!ok) throw new ReloadFailure(code); }
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
  reloadViewer, extendInput, observeRequests, observeReadState, observeMs = 0,
  clock = { now: () => performance.now(), setTimeout, clearTimeout },
}) {
  const started = clock.now();
  const evidence = { schema: "elastos.browser.journey-viewer-reload/v1", ok: false,
    requests: [], samples: [], dropped_requests: 0, observer: { stopped: false },
    started_monotonic_ms: started };
  let phase = "baseline", stop, failure, forbidden = false, original, initialViewer;
  let newDocument, newVideo, bytesRequired, initialReceipt, input, receiptSequence;
  let reloadStarted;
  const interruptions = new Set();
  const elapsed = () => Math.round(clock.now() - started);
  function binding(raw, source) {
    try { return browserJourneyBinding(raw); }
    catch (error) {
      if (error.binding_diagnostic)
        evidence.binding_failure = { at_ms: elapsed(), phase, source, ...error.binding_diagnostic };
      throw new ReloadFailure("binding_unavailable_or_cleanup");
    }
  }
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
    if (!navigation && (!["opening", "closing", "renewal", "probe", "status", "summary", "heartbeat", "signaling"].includes(event.kind) ||
      !["request", "headers", "response", "failed"].includes(event.phase))) return;
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
    await within(deadline, phase === "baseline" ? "baseline_deadline" : phase === "observe" ? "observe_deadline" : "reload_deadline", ({ signal, timeoutMs }) =>
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
      page_id: raw?.sessions?.recoverable_page?.page_id, actual_url: raw?.page_status?.actual_url }, "runtime"));
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
        matches(binding({ ...viewer, sessions: raw.sessions, page_status: raw.page_status }, "viewer"));
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
      ...viewerFailureFields(viewer),
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
    const baselineDeadline = clock.now() + BASELINE_MS;
    // Collect a successfully installed observer even if it reports a forbidden event during setup.
    stop = await within(baselineDeadline, "observer_deadline", budget => observeRequests(record, budget), true);
    requireEvidence(typeof stop === "function", "observer_missing");
    const raw = await within(baselineDeadline, "state_deadline", readState);
    initialViewer = raw?.viewer;
    requireEvidence(initialViewer && documentId(initialViewer.document_id), "baseline_viewer_missing");
    original = binding({ ...initialViewer, sessions: raw.sessions, page_status: raw.page_status }, "baseline");
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
    reloadStarted = clock.now();
    const deadline = reloadStarted + WINDOW_MS;
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
  }
  if (observeMs > WINDOW_MS && reloadStarted) {
    phase = "observe";
    evidence.observe_ms = observeMs;
    const observeDeadline = reloadStarted + observeMs;
    const readObserve = typeof observeReadState === "function" ? observeReadState : readState;
    while (clock.now() < observeDeadline) {
      try {
        const raw = await within(Math.min(clock.now() + 1000, observeDeadline), "observe_state_deadline", readObserve);
        const viewer = raw?.viewer;
        let media = null;
        if (raw?.video) {
          try { media = video(raw.video, false); } catch { media = null; }
        }
        evidence.samples.push({
          at_ms: elapsed(),
          phase,
          binding_matches: "unmeasured",
          viewer_ready: Boolean(media),
          viewer_has_page: Boolean(viewer?.page_id),
          ...viewerFailureFields(viewer),
          video_present: raw?.video?.present === true,
          ...(count(raw?.video?.ready_state) ? { video_ready_state: raw.video.ready_state } : {}),
          ...(count(raw?.video?.decoded_frames) ? { video_decoded_frames: raw.video.decoded_frames } : {}),
          video_bytes_reported: count(raw?.video?.video_bytes_received),
          ...(viewer ? { document_hash: hash(viewer.document_id) } : {}),
          ...signalingCensus(raw?.page_status),
          ...(media || {}),
        });
        if (evidence.samples.length > 96) evidence.samples.shift();
      } catch {
        evidence.dropped_observe_samples = (evidence.dropped_observe_samples || 0) + 1;
      }
      if (clock.now() >= observeDeadline) break;
      try { await pause(observeDeadline); } catch { break; }
    }
    evidence.observe_last_ms = elapsed();
    const late = evidence.samples.find((row) =>
      row.phase === "observe" && row.visible === true && count(row.decoded_frames) && row.decoded_frames > 0);
    if (late && Number.isFinite(evidence.reload_started_ms)) {
      evidence.first_late_decoded_frame_ms = late.at_ms - evidence.reload_started_ms;
    }
  }
  if (typeof stop === "function") {
    try {
      await within(clock.now() + WINDOW_MS, "observer_stop_deadline", budget => stop(budget), true);
      evidence.observer.stopped = true;
    } catch { evidence.observer.failure = "observer_stop_failed"; failure ||= "observer_stop_failed"; }
  }
  if (forbidden) failure = "page_open_or_close_observed";
  evidence.ok = !failure;
  if (failure) {
    evidence.failure = failure;
    evidence.failure_phase = phase;
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
      failure_stage: (document.querySelector("#browser-status")?.dataset?.browserFailureStage || "").slice(0, 64),
      failure_message: (document.querySelector("#browser-status")?.dataset?.browserFailureMessage || "").slice(0, 240),
      failure_status: Number(document.querySelector("#browser-status")?.dataset?.browserFailureStatus || 0) || 0,
    },
    video: element ? { present: true, hidden: element.hidden, paused: element.paused,
      ready_state: element.readyState, video_width: element.videoWidth, video_height: element.videoHeight,
      client_width: Math.round(rect.width), client_height: Math.round(rect.height),
      decoded_frames: Number(element.webkitDecodedFrameCount || 0),
      ...(Number.isSafeInteger(bytes) ? { video_bytes_received: bytes } : {}) } : null,
  };
}


const CENSUS_WATCH_MS = 5000;
const CENSUS_POLL_MS = 250;

function recordCensusRequest(evidence, elapsed, event) {
  if (event?.source_matches !== true) return;
  const navigation = event.kind === "navigation" && event.phase === "commit";
  if (!navigation && (event.kind !== "signaling" ||
    !["request", "headers", "response", "failed"].includes(event.phase))) return;
  if (evidence.requests.length === 64) {
    evidence.dropped_requests++;
    return;
  }
  evidence.requests.push({
    at_ms: elapsed(),
    kind: event.kind,
    event: event.phase,
    ...browserViewerSignalMetadata(event),
    ...(count(event.document_generation) ? { document_generation: event.document_generation } : {}),
    ...(count(event.status) && event.status <= 599 ? { status: event.status } : {}),
  });
}

function recordCensusDiagnostic(evidence, elapsed, raw) {
  if (!raw || raw.schema !== "elastos.browser.media-diagnostic/v1") return;
  if (evidence.diagnostics.length === 64) {
    evidence.dropped_diagnostics++;
    return;
  }
  evidence.diagnostics.push({
    at_ms: elapsed(),
    event: raw.event,
    ...(typeof raw.attach_from_url === "boolean" ? { attach_from_url: raw.attach_from_url } : {}),
    ...(typeof raw.from_summary === "boolean" ? { from_summary: raw.from_summary } : {}),
    ...(typeof raw.request_id === "string" && /^[a-f0-9]{32}$/.test(raw.request_id)
      ? { request_id: raw.request_id }
      : {}),
    ...(typeof raw.candidate_type === "string" ? { candidate_type: raw.candidate_type } : {}),
    ...(typeof raw.ice_connection_state === "string" ? { ice_connection_state: raw.ice_connection_state } : {}),
    ...(typeof raw.ice_gathering_state === "string" ? { ice_gathering_state: raw.ice_gathering_state } : {}),
    ...(count(raw.video_packets_received) ? { video_packets_received: raw.video_packets_received } : {}),
    ...(count(raw.browser_candidate_count) ? { browser_candidate_count: raw.browser_candidate_count } : {}),
  });
}

/** Attribute reload time. This is a measurement, not the five-second product gate. */
export function classifyReloadCensus(evidence) {
  const origin = Number.isFinite(evidence?.reload_started_ms) ? evidence.reload_started_ms : 0;
  const rel = (ms) => Number.isFinite(ms) ? Math.round(ms - origin) : null;
  const rows = evidence?.requests || [];
  const diagnostics = evidence?.diagnostics || [];
  const samples = evidence?.samples || [];
  const request = (type, phase) => rows.find((row) =>
    row.kind === "signaling" && row.signal_type === type && row.event === phase);
  const navigation = rows.find((row) => row.kind === "navigation" && row.event === "commit");
  const firstFrame = samples.find((row) =>
    (row.phase === "reload" || row.phase === "observe") && row.visible === true && count(row.decoded_frames) && row.decoded_frames > 0);
  const generations = [...new Set(rows.filter((row) => count(row.document_generation)).map((row) => row.document_generation))];
  const attachRequests = rows.filter((row) =>
    row.kind === "signaling" && row.signal_type === "display_attach" && row.event === "request");
  const attachResponses = rows.filter((row) =>
    row.kind === "signaling" && row.signal_type === "display_attach" && row.event === "response");
  const attach = attachResponses[0];
  const reloadSamples = samples.filter((row) => row.phase === "reload" || row.phase === "observe");
  const documentHashes = [...new Set(reloadSamples.map((row) => row.document_hash).filter((value) => text(value)))];
  const firstReloadHash = documentHashes[0];
  const secondDocument = reloadSamples.find((row) =>
    text(row.document_hash) && firstReloadHash && row.document_hash !== firstReloadHash);
  const viewerFailure = reloadSamples.find((row) =>
    text(row.viewer_failure_message) ||
    text(row.viewer_failure_stage) ||
    (typeof row.viewer_status === "string" &&
      /could not complete|session expired|failed to start|temporarily unavailable/i.test(row.viewer_status)));
  return {
    schema: "elastos.browser.journey-reload-census-classification/v1",
    reload_started_ms: origin,
    navigation_commit_ms: rel(navigation?.at_ms),
    display_attach_request_ms: rel(attachRequests[0]?.at_ms),
    display_attach_response_ms: rel(attachResponses[0]?.at_ms),
    display_attach_request_count: attachRequests.length,
    last_display_attach_request_ms: rel(attachRequests[attachRequests.length - 1]?.at_ms),
    last_display_attach_response_ms: rel(attachResponses[attachResponses.length - 1]?.at_ms),
    dying_page_attach_ms: rel(diagnostics.find((row) => row.event === "dying_page_attach")?.at_ms),
    dying_page_attach_request_id:
      diagnostics.find((row) => row.event === "dying_page_attach" && typeof row.request_id === "string")
        ?.request_id ?? null,
    restore_boot_ms: rel(diagnostics.find((row) => row.event === "restore_boot")?.at_ms),
    restore_boot_attach_from_url:
      diagnostics.find((row) => row.event === "restore_boot" && typeof row.attach_from_url === "boolean")
        ?.attach_from_url ?? null,
    restore_boot_request_id:
      diagnostics.find((row) => row.event === "restore_boot" && typeof row.request_id === "string")
        ?.request_id ?? null,
    restore_boot_ready_ms: rel(diagnostics.find((row) => row.event === "restore_boot_ready")?.at_ms),
    restore_boot_from_summary:
      diagnostics.find((row) => row.event === "restore_boot_ready" && typeof row.from_summary === "boolean")
        ?.from_summary ?? null,
    first_answer_request_ms: rel(request("answer", "request")?.at_ms),
    first_answer_response_ms: rel(request("answer", "response")?.at_ms),
    first_local_candidate_created_ms: rel(diagnostics.find((row) => row.event === "viewer_browser_candidate")?.at_ms),
    first_candidate_request_ms: rel(request("candidate", "request")?.at_ms),
    first_candidate_response_ms: rel(request("candidate", "response")?.at_ms),
    ice_checking_ms: rel(diagnostics.find((row) => row.ice_connection_state === "checking")?.at_ms),
    ice_connected_ms: rel(diagnostics.find((row) => row.ice_connection_state === "connected")?.at_ms),
    first_decoded_frame_ms: rel(firstFrame?.at_ms),
    first_decoded_frame_after_deadline: Number.isFinite(rel(firstFrame?.at_ms)) && rel(firstFrame?.at_ms) > WINDOW_MS,
    document_generations: generations,
    observer_navigation_events: rows.filter((row) => row.kind === "navigation" && row.event === "commit").length,
    document_hashes: documentHashes,
    same_document: documentHashes.length <= 1,
    second_document_ms: secondDocument ? rel(secondDocument.at_ms) : null,
    viewer_failure_ms: viewerFailure ? rel(viewerFailure.at_ms) : null,
    ...(text(viewerFailure?.viewer_failure_stage) ? { viewer_failure_stage: viewerFailure.viewer_failure_stage } : {}),
    ...(text(viewerFailure?.viewer_failure_message) ? { viewer_failure_message: viewerFailure.viewer_failure_message } : {}),
    ...(count(viewerFailure?.viewer_failure_status) ? { viewer_failure_status: viewerFailure.viewer_failure_status } : {}),
    ...(typeof viewerFailure?.viewer_status === "string" ? { viewer_failure_status_text: viewerFailure.viewer_status.slice(0, 240) } : {}),
    attach_initial_offer_ice: attach?.initial_offer_ice || null,
    attach_audio_offer_ice: attach?.audio_offer_ice || null,
    last_sample_ms: rel(samples.at(-1)?.at_ms),
    last_sample: samples.at(-1) ? {
      video_present: samples.at(-1).video_present === true,
      video_decoded_frames: count(samples.at(-1).video_decoded_frames)
        ? samples.at(-1).video_decoded_frames : "none",
      ...(samples.at(-1).engine_signaling ? { engine_signaling: samples.at(-1).engine_signaling } : {}),
    } : null,
  };
}

/**
 * Reload a working viewer and record attach, answer, candidate, ICE, and frame times.
 * Reuses the journey observer. Does not type, scroll, or apply the five-second gate.
 */
export async function censusBrowserViewerReload({
  readState, reloadViewer, observeRequests, observeDiagnostics,
  clock = { now: () => performance.now(), setTimeout, clearTimeout },
  watchMs = CENSUS_WATCH_MS,
}) {
  const started = clock.now();
  const evidence = {
    schema: "elastos.browser.journey-reload-census/v1",
    ok: false,
    requests: [],
    samples: [],
    diagnostics: [],
    dropped_requests: 0,
    dropped_diagnostics: 0,
    observer: { stopped: false },
    started_monotonic_ms: started,
  };
  let phase = "baseline", stopRequests, stopDiagnostics, failure;
  const elapsed = () => Math.round(clock.now() - started);
  async function within(deadlineMs, code, action) {
    const timeoutMs = deadlineMs - clock.now();
    if (timeoutMs <= 0) throw new ReloadFailure(code);
    const controller = new AbortController();
    let timer;
    try {
      const limit = new Promise((_, reject) => {
        timer = clock.setTimeout(() => {
          controller.abort();
          reject(new ReloadFailure(code));
        }, timeoutMs);
      });
      const result = await Promise.race([
        limit,
        Promise.resolve().then(() => action({ signal: controller.signal, deadlineMs, timeoutMs })),
      ]);
      if (clock.now() > deadlineMs) throw new ReloadFailure(code);
      return result;
    } catch (error) {
      throw error instanceof ReloadFailure ? error : new ReloadFailure(code.replace(/_deadline$/, "_failed"));
    } finally {
      clock.clearTimeout(timer);
    }
  }
  async function pause(deadline) {
    await within(deadline, phase === "baseline" ? "baseline_deadline" : "census_deadline", ({ signal, timeoutMs }) =>
      new Promise((resolve) => {
        const onAbort = () => { clock.clearTimeout(timer); resolve(); };
        const timer = clock.setTimeout(() => {
          signal.removeEventListener("abort", onAbort);
          resolve();
        }, Math.min(CENSUS_POLL_MS, timeoutMs));
        signal.addEventListener("abort", onAbort, { once: true });
      }));
  }
  async function sample(deadline) {
    const raw = await within(deadline, "state_deadline", readState);
    const viewer = raw?.viewer;
    let media = null;
    if (raw?.video) {
      try { media = video(raw.video); } catch { media = null; }
    }
    evidence.samples.push({
      at_ms: elapsed(),
      phase,
      viewer_ready: Boolean(media?.visible),
      viewer_has_page: Boolean(viewer?.page_id),
      ...viewerFailureFields(viewer),
      video_present: raw?.video?.present === true,
      visible: media?.visible === true,
      ...(count(raw?.video?.ready_state) ? { video_ready_state: raw.video.ready_state } : {}),
      ...(count(raw?.video?.decoded_frames) ? { video_decoded_frames: raw.video.decoded_frames } : {}),
      ...(count(media?.decoded_frames) ? { decoded_frames: media.decoded_frames } : {}),
      ...(viewer?.document_id ? { document_hash: hash(viewer.document_id) } : {}),
      ...signalingCensus(raw?.page_status),
    });
    const sampleLimit = watchMs > CENSUS_WATCH_MS ? 96 : 32;
    if (evidence.samples.length > sampleLimit) evidence.samples.shift();
    return media;
  }
  try {
    const baselineDeadline = clock.now() + BASELINE_MS;
    stopRequests = await within(baselineDeadline, "observer_deadline",
      (budget) => observeRequests((event) => recordCensusRequest(evidence, elapsed, event), budget));
    if (typeof observeDiagnostics === "function") {
      stopDiagnostics = await within(baselineDeadline, "observer_deadline",
        (budget) => observeDiagnostics((raw) => recordCensusDiagnostic(evidence, elapsed, raw), budget));
    }
    const raw = await within(baselineDeadline, "state_deadline", readState);
    requireEvidence(raw?.viewer && documentId(raw.viewer.document_id), "baseline_viewer_missing");
    const initialVideo = video(raw.video);
    requireEvidence(initialVideo.visible && initialVideo.decoded_frames > 0, "baseline_video_not_visible");
    let current;
    do {
      await pause(baselineDeadline);
      current = await sample(baselineDeadline);
    } while (!(current?.visible && current.decoded_frames > initialVideo.decoded_frames));

    phase = "reload";
    const reloadStarted = clock.now();
    const deadline = reloadStarted + watchMs;
    evidence.reload_started_ms = elapsed();
    await within(deadline, "reload_deadline", reloadViewer);
    while (clock.now() < deadline) {
      try {
        await sample(deadline);
      } catch (error) {
        if (error instanceof ReloadFailure && error.message === "state_deadline") {
          evidence.dropped_observe_samples = (evidence.dropped_observe_samples || 0) + 1;
        } else {
          throw error;
        }
      }
      if (clock.now() >= deadline) break;
      try {
        await pause(deadline);
      } catch (error) {
        if (error instanceof ReloadFailure && error.message === "census_deadline") break;
        throw error;
      }
    }
    evidence.watch_ms = clock.now() - reloadStarted;
  } catch (error) {
    failure = error instanceof ReloadFailure ? error.message : "reload_census_failed";
  } finally {
    if (typeof stopRequests === "function") {
      try {
        await within(clock.now() + WINDOW_MS, "observer_stop_deadline", (budget) => stopRequests(budget));
        evidence.observer.stopped = true;
      } catch {
        evidence.observer.failure = "observer_stop_failed";
        failure ||= "observer_stop_failed";
      }
    }
    if (typeof stopDiagnostics === "function") {
      try {
        await within(clock.now() + WINDOW_MS, "observer_stop_deadline", (budget) => stopDiagnostics(budget));
      } catch {
        evidence.observer.diagnostics_failure = "observer_stop_failed";
      }
    }
  }
  evidence.classification = classifyReloadCensus(evidence);
  evidence.first_frame_in_window = Number.isFinite(evidence.classification.first_decoded_frame_ms);
  evidence.ok = !failure;
  if (failure) {
    evidence.failure = failure;
    evidence.failure_phase = phase;
    const error = new Error(`Browser viewer reload census failed: ${failure}`);
    error.evidence = evidence;
    throw error;
  }
  return evidence;
}


function iceCensus(candidates) {
  const list = Array.isArray(candidates) ? candidates : [];
  return {
    candidate_count: list.length,
    relay_candidate_count: list.filter((candidate) =>
      /\btyp\s+relay\b/i.test(String(candidate?.candidate || "")),
    ).length,
  };
}

function passIceCensus(value) {
  return value &&
    Number.isSafeInteger(value.candidate_count) &&
    value.candidate_count >= 0 &&
    Number.isSafeInteger(value.relay_candidate_count) &&
    value.relay_candidate_count >= 0 &&
    value.relay_candidate_count <= value.candidate_count
    ? {
        candidate_count: value.candidate_count,
        relay_candidate_count: value.relay_candidate_count,
      }
    : null;
}

/** Keep signaling stage evidence bounded; SDP, candidates and authority remain private. */
export function browserViewerSignalMetadata(payload) {
  const types = ["display_attach", "offer", "answer", "candidate", "end_of_candidates"];
  const codes = ["display_attach_busy", "display_generation_mismatch", "display_owner_changed",
    "display_attach_unsupported", "display_attach_failed", "display_attach_uncertain"];
  const type = payload?.signal_type ?? payload?.type;
  const code = payload?.error_code ?? payload?.code;
  const attach = payload?.schema === "elastos.browser.display-attach-result/v1" || payload?.attached === true;
  const initialOfferIce = passIceCensus(payload?.initial_offer_ice) ||
    (attach && Array.isArray(payload?.initial_offer?.candidates)
      ? iceCensus(payload.initial_offer.candidates)
      : null);
  const audioOfferIce = passIceCensus(payload?.audio_offer_ice) ||
    (attach && Array.isArray(payload?.audio_offer?.candidates)
      ? iceCensus(payload.audio_offer.candidates)
      : null);
  const ackIce = passIceCensus(payload?.ack_ice) ||
    (payload?.schema === "elastos.browser.webrtc-signal-ack/v1" && Array.isArray(payload?.candidates)
      ? iceCensus(payload.candidates)
      : null);
  return {
    ...(types.includes(type) ? { signal_type: type } : {}),
    ...(codes.includes(code) ? { error_code: code } : {}),
    ...(attach ? { attached: true } : {}),
    ...(initialOfferIce ? { initial_offer_ice: initialOfferIce } : {}),
    ...(audioOfferIce ? { audio_offer_ice: audioOfferIce } : {}),
    ...(ackIce ? { ack_ice: ackIce } : {}),
  };
}
