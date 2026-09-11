import { createHash } from "node:crypto";

const WINDOW_MS = 5000;
const POLL_MS = 250;
export { binding as browserJourneyBinding, video as browserJourneyVideo };
const hash = value => `sha256:${createHash("sha256").update(value).digest("hex").slice(0, 16)}`;
const text = value => typeof value === "string" && value.length > 0 && value.length <= 512;
const count = value => Number.isSafeInteger(value) && value >= 0;
const BINDING_FIELDS = ["page_id", "cleanup_schema", "cleanup_id", "profile_key_hash",
  "runtime_exit_id", "engine_adapter", "engine", "engine_selection", "exit_selection",
  "browser_instance", "actual_url", "active_sessions", "total_sessions", "principal_sessions"];
class RecoveryFailure extends Error {}
function requireEvidence(ok, code) {
  if (!ok) throw new RecoveryFailure(code);
}

// readBinding returns { sessions: summary.body.sessions, page_id: viewerPageId,
// engine_id: engineSelect.value, exit_id: exitSelect.value, browser_instance,
// actual_url: addressInput.value, page_status: freshStatus.body }. Read Runtime state
// with the existing fixture authority outside the interrupted viewer transport.
// recoverable_page.engine_page retains the launch URL; page_status reports the
// current URL after navigation and must bind the same Runtime-owned page.
function binding(value) {
  const sessions = value?.sessions;
  const owner = sessions?.recoverable_page;
  const rows = sessions?.lifecycle?.sessions;
  const pageHash = text(value?.page_id) ? hash(value.page_id) : null;
  const lifecycle = Array.isArray(rows) ? rows.filter(row => pageHash && row?.page_id === pageHash) : [];
  // These named predicates are both the acceptance check and its failure evidence.
  // Retain fixed names, allowlisted states and counts only, never response values.
  const checks = {
    sessions_schema: sessions?.schema === "elastos.browser.session-capacity/v1",
    owner_active: owner?.state === "active",
    owner_page_valid: text(owner?.page_id),
    viewer_page_matches: text(owner?.page_id) && owner.page_id === value?.page_id,
    cleanup_schema: owner?.cleanup?.schema === "elastos.browser.cleanup-handle/v1",
    cleanup_id_valid: text(owner?.cleanup?.id),
    lifecycle_unique: lifecycle.length === 1,
    lifecycle_profile_valid: text(lifecycle[0]?.profile_key_hash),
    lifecycle_exit_valid: text(lifecycle[0]?.exit_id),
    lifecycle_phase_active: ["ACTIVE_SESSION", "NAVIGATING"].includes(lifecycle[0]?.phase),
    engine_page_matches: text(owner?.page_id) && owner?.engine_page?.page_id === owner.page_id,
    engine_adapter_valid: text(owner?.engine_page?.adapter),
    engine_kind_valid: text(owner?.engine_page?.engine),
    engine_selection_valid: typeof value?.engine_id === "string" && value.engine_id.length <= 512,
    exit_selection_valid: typeof value?.exit_id === "string" && value.exit_id.length <= 512,
    browser_instance_valid: text(value?.browser_instance),
    viewer_url_valid: text(value?.actual_url),
    page_status_schema: value?.page_status?.schema === "elastos.browser.page-status/v1",
    page_status_page_matches: text(owner?.page_id) && value?.page_status?.page_id === owner.page_id,
    page_status_url_matches: text(value?.actual_url) && value?.page_status?.actual_url === value.actual_url,
    session_counts_valid: [sessions?.active_sessions, sessions?.total_sessions, sessions?.principal_sessions].every(count),
    active_sessions_positive: count(sessions?.active_sessions) && sessions.active_sessions > 0,
    principal_sessions_positive: count(sessions?.principal_sessions) && sessions.principal_sessions > 0,
    total_matches_active: count(sessions?.total_sessions) && sessions.total_sessions === sessions.active_sessions,
    principal_within_total: count(sessions?.principal_sessions) && count(sessions?.total_sessions) &&
      sessions.principal_sessions <= sessions.total_sessions,
    no_launching_sessions: sessions?.launching_sessions === 0,
    no_engine_cleanup: sessions?.engine_cleanup_obligations === 0,
    no_launch_reconciliation: sessions?.launch_reconciliation_obligations === 0,
  };
  const failed = Object.keys(checks).filter(key => !checks[key]);
  if (failed.length) {
    throw Object.assign(new RecoveryFailure("binding_unavailable_or_cleanup"), { binding_diagnostic: {
      failed_checks: failed,
      owner_state: ["active", "cleanup_pending"].includes(owner?.state) ? owner.state : "absent_or_unknown",
      lifecycle_matches: lifecycle.length,
      counts: Object.fromEntries(["active_sessions", "total_sessions", "principal_sessions", "launching_sessions",
        "engine_cleanup_obligations", "launch_reconciliation_obligations"].filter(key => count(sessions?.[key]))
        .map(key => [key, sessions[key]])),
    } });
  }
  // Keep handles private. Equality uses the exact strings; evidence uses hashes.
  return [owner.page_id, owner.cleanup.schema, owner.cleanup.id, lifecycle[0].profile_key_hash,
    lifecycle[0].exit_id, owner.engine_page.adapter, owner.engine_page.engine, value.engine_id, value.exit_id,
    value.browser_instance, value.actual_url, sessions.active_sessions, sessions.total_sessions, sessions.principal_sessions];
}

function video(value, bytesRequired = false) {
  requireEvidence(value && count(value.decoded_frames) &&
    (!Object.hasOwn(value, "video_bytes_received") || count(value.video_bytes_received)) &&
    (!bytesRequired || count(value.video_bytes_received)), "video_metrics_unavailable");
  return {
    visible: value.present === true && value.hidden === false && value.paused === false &&
      value.ready_state >= 2 && value.video_width > 0 && value.video_height > 0 &&
      value.client_width > 0 && value.client_height > 0,
    decoded_frames: value.decoded_frames,
    ...(count(value.video_bytes_received) ? { video_bytes_received: value.video_bytes_received } : {}),
  };
}

/**
 * Opt-in B06 viewer interruption diagnostic; importing this module has no effects.
 * cdp is a dedicated context.newCDPSession(viewer Page|Frame), owned by this call.
 * readVideo uses browserRemoteVideoMetrics, optionally merging video_bytes_received
 * from latestVideoWebrtcStats/latestWebrtcStats. readReceipt is the existing fixture
 * reader. extendInput(suffix, budget) appends through the existing viewer input path.
 * probeViewerRequest(budget) issues a read-only, cache-bypassing viewer fetch.
 *
 * observeRequests(record, budget) installs listeners before observation and returns
 * a stop function that flushes and removes them. Record exact-source requests as
 * { kind: opening|closing|renewal|probe|status|heartbeat, phase: request|response|failed,
 *   request_id, source_matches: true, status? }. Use requestfailed for network failure,
 * not an HTTP error. Match the exact viewer frame; bind Home renewal messages to
 * that frame too. Retain the same request_id across request/response/failure events.
 * Raw headers, bodies, error strings and URLs are discarded here.
 * expectedUrl is the exact controlled fixture URL, including its receipt run.
 *
 * All callbacks receive { signal, deadlineMs, timeoutMs }; honor this budget,
 * especially for input. Observations must be fresh, without an internal retry loop.
 */
export async function diagnoseBrowserJourneyRecovery({
  cdp, readBinding, readVideo, readReceipt, extendInput, probeViewerRequest, observeRequests,
  expectedUrl, mediaInterruption,
  inputSuffix = "-recovered",
  clock = { now: () => performance.now(), setTimeout, clearTimeout },
}) {
  const started = clock.now();
  const evidence = {
    schema: "elastos.browser.journey-recovery/v1", ok: false,
    requests: [], samples: [], dropped_requests: 0, viewer_request_failed: false,
    restore: { attempted: false, ok: false }, detach: { attempted: false, ok: false },
  };
  let phase = "baseline", stop, failure, forbidden = false;
  let original, initialReceipt, input, receiptSequence, initialVideo, cutVideo, bytesRequired;
  const pending = new Set();
  const elapsed = () => Math.round(clock.now() - started);
  const pause = ms => new Promise(resolve => clock.setTimeout(resolve, ms));
  function checkedBinding(raw) {
    try { return binding(raw); }
    catch (error) {
      if (error instanceof RecoveryFailure && error.binding_diagnostic)
        evidence.binding_failure = { at_ms: elapsed(), phase, ...error.binding_diagnostic };
      throw error;
    }
  }
  async function within(deadlineMs, code, action) {
    const timeoutMs = deadlineMs - clock.now();
    requireEvidence(timeoutMs > 0, code);
    const controller = new AbortController();
    let timer;
    try {
      const result = await Promise.race([
        Promise.resolve().then(() => action({ signal: controller.signal, deadlineMs, timeoutMs })),
        new Promise((_, reject) => {
          const expire = () => {
            const remaining = deadlineMs - clock.now();
            if (remaining > 0) {
              timer = clock.setTimeout(expire, Math.max(1, Math.ceil(remaining)));
              return;
            }
            controller.abort();
            reject(new RecoveryFailure(code));
          };
          timer = clock.setTimeout(expire, Math.ceil(timeoutMs));
        }),
      ]);
      requireEvidence(clock.now() <= deadlineMs, code);
      return result;
    } finally {
      clock.clearTimeout(timer);
    }
  }
  function record(event) {
    if (event?.source_matches !== true || !text(event.request_id) ||
      !["opening", "closing", "renewal", "probe", "status", "heartbeat"].includes(event.kind) ||
      !["request", "response", "failed"].includes(event.phase)) return;
    if (["opening", "closing"].includes(event.kind)) forbidden = true;
    const key = hash(event.request_id);
    if (phase === "cut" && event.kind === "probe") {
      if (event.phase === "request" && pending.size < 64) pending.add(key);
      if (event.phase === "failed" && pending.delete(key)) evidence.viewer_request_failed = true;
    }
    if (evidence.requests.length === 64) { evidence.dropped_requests++; return; }
    evidence.requests.push({ at_ms: elapsed(), phase, kind: event.kind, event: event.phase,
      request_hash: key, ...(count(event.status) && event.status <= 599 ? { status: event.status } : {}) });
  }
  async function sample(deadline) {
    const raw = await within(deadline, "binding_deadline", readBinding);
    const state = raw?.sessions?.recoverable_page?.state;
    evidence.last_runtime = {
      owner_state: ["active", "cleanup_pending"].includes(state) ? state : "absent_or_unknown",
      ...Object.fromEntries(["active_sessions", "launching_sessions", "engine_cleanup_obligations",
        "launch_reconciliation_obligations"].filter(key => count(raw?.sessions?.[key]))
        .map(key => [key, raw.sessions[key]])),
    };
    const value = checkedBinding(raw);
    evidence.changed_binding_fields = BINDING_FIELDS.filter((_, i) => value[i] !== original[i]);
    requireEvidence(evidence.changed_binding_fields.length === 0, "binding_changed");
    const media = video(await within(deadline, "video_deadline", readVideo), bytesRequired);
    requireEvidence(Object.hasOwn(media, "video_bytes_received") === bytesRequired, "byte_metrics_changed");
    evidence.samples.push({ at_ms: elapsed(), phase, binding_matches: true, ...media });
    if (evidence.samples.length > 64) evidence.samples.shift();
    return media;
  }
  function validReceipt(receipt) {
    requireEvidence(receipt?.schema === "elastos.browser.journey-receipt/v1" &&
      text(receipt.run) && Array.isArray(receipt.events) && receipt.events.length <= 128 &&
      receipt.events.every((event, i, events) => count(event.sequence) && event.sequence > 0 &&
        (i === 0 || event.sequence > events[i - 1].sequence)), "receipt_invalid");
    return receipt;
  }
  const advanced = (after, before) => after.visible && after.decoded_frames > before.decoded_frames &&
    (!bytesRequired || after.video_bytes_received > before.video_bytes_received);
  try {
    try {
      const deadline = clock.now() + WINDOW_MS;
      stop = await within(deadline, "observer_deadline", budget => observeRequests(record, budget));
      requireEvidence(typeof stop === "function", "observer_missing");
      original = checkedBinding(await within(deadline, "binding_deadline", readBinding));
      evidence.binding_hashes = Object.fromEntries(BINDING_FIELDS.map((key, i) => [key, hash(String(original[i]))]));
      initialReceipt = validReceipt(await within(deadline, "receipt_deadline", readReceipt));
      input = initialReceipt.events.findLast(event => event.type === "input");
      receiptSequence = initialReceipt.events.at(-1)?.sequence;
      requireEvidence(input && ["main", "nav"].includes(input.page) && text(input.value) &&
        typeof inputSuffix === "string" && inputSuffix.length > 0 &&
        input.value.length + inputSuffix.length <= 96, "current_input_missing");
      requireEvidence(text(expectedUrl) && original[BINDING_FIELDS.indexOf("actual_url")] === expectedUrl,
        "fixture_url_mismatch");
      const fixtureUrl = new URL(expectedUrl);
      requireEvidence(["http:", "https:"].includes(fixtureUrl.protocol) &&
        fixtureUrl.searchParams.getAll("run").length === 1 &&
        fixtureUrl.searchParams.get("run") === initialReceipt.run && fixtureUrl.pathname === `/${input.page}`,
      "fixture_run_mismatch");
      initialVideo = video(await within(deadline, "video_deadline", readVideo));
      bytesRequired = Object.hasOwn(initialVideo, "video_bytes_received");
      requireEvidence(initialVideo.visible && initialVideo.decoded_frames > 0, "baseline_video_not_visible");
      do {
        await pause(Math.min(POLL_MS, Math.max(0, deadline - clock.now())));
        cutVideo = await sample(deadline);
      } while (!advanced(cutVideo, initialVideo));
      await within(deadline, "network_enable_deadline", () => cdp.send("Network.enable"));
      await within(deadline, "network_cut_deadline", () => cdp.send("Network.emulateNetworkConditions", {
        offline: true, latency: 0, downloadThroughput: -1, uploadThroughput: -1, packetLoss: 100,
      }));
      if (mediaInterruption) {
        await within(deadline, "media_cut_deadline", budget => mediaInterruption.cut(budget));
        evidence.media_cut_acknowledged = true;
      }
      phase = "cut";
      const cutStarted = clock.now(), cutDeadline = cutStarted + WINDOW_MS;
      let unchangedSince = cutStarted;
      await within(cutDeadline, "viewer_probe_deadline", probeViewerRequest);
      while (clock.now() < cutDeadline) {
        let current;
        try {
          current = await sample(cutDeadline);
        } catch (error) {
          // The cut ends on time even if its last observation is unfinished.
          // Earlier completed samples must still prove the request failure and
          // media stall. Ownership changes and real observation errors fail.
          if (error instanceof RecoveryFailure && clock.now() >= cutDeadline &&
            ["binding_deadline", "video_deadline"].includes(error.message)) break;
          throw error;
        }
        requireEvidence(current.visible && current.decoded_frames >= cutVideo.decoded_frames &&
          (!bytesRequired || current.video_bytes_received >= cutVideo.video_bytes_received), "cut_video_reset_or_hidden");
        if (current.decoded_frames !== cutVideo.decoded_frames ||
          (bytesRequired && current.video_bytes_received !== cutVideo.video_bytes_received)) unchangedSince = clock.now();
        cutVideo = current;
        evidence.stall_ms = Math.round(clock.now() - unchangedSince);
        await pause(Math.min(POLL_MS, Math.max(0, cutDeadline - clock.now())));
      }
      evidence.cut_ms = Math.round(clock.now() - cutStarted);
      requireEvidence(evidence.viewer_request_failed, "viewer_request_failure_unproven");
      requireEvidence(evidence.stall_ms >= 1000, "viewer_media_cut_unproven");
    } catch (error) {
      failure = error instanceof RecoveryFailure ? error.message : "observation_failed";
      throw error;
    } finally {
      phase = "restoring";
      evidence.restore.attempted = true;
      try {
        const restoreDeadline = clock.now() + WINDOW_MS;
        // These independent restores both run even if either transport fails.
        const [http, media] = await Promise.allSettled([
          within(restoreDeadline, "network_restore_deadline", () => cdp.send("Network.emulateNetworkConditions", {
            offline: false, latency: 0, downloadThroughput: -1, uploadThroughput: -1, packetLoss: 0,
          })),
          mediaInterruption
            ? within(restoreDeadline, "media_restore_deadline", budget => mediaInterruption.restore(budget))
            : Promise.resolve(),
        ]);
        if (mediaInterruption) {
          evidence.restore.http_ok = http.status === "fulfilled";
          evidence.restore.media_ok = media.status === "fulfilled";
        }
        requireEvidence(http.status === "fulfilled" && media.status === "fulfilled", "network_restore_failed");
        evidence.restore.ok = true;
      } catch {
        evidence.restore.error = "network_restore_failed";
        if (!failure) throw new RecoveryFailure("network_restore_failed");
      }
    }

    phase = "recovery";
    const restored = clock.now(), deadline = restored + WINDOW_MS;
    // Every observation and the input effect share this one restoration deadline.
    const restoredVideo = await sample(deadline);
    let sent = false, beforeInputVideo;
    while (true) {
      requireEvidence(!forbidden, "cleanup_or_replacement_requested");
      const current = await sample(deadline);
      if (advanced(current, restoredVideo)) {
        if (!sent) {
          beforeInputVideo = current;
          await within(deadline, "input_deadline", budget => extendInput(inputSuffix, budget));
          sent = true;
        }
        const receipt = validReceipt(await within(deadline, "receipt_deadline", readReceipt));
        requireEvidence(receipt.run === initialReceipt.run, "receipt_run_changed");
        requireEvidence(!receipt.events.some(event => event.sequence > receiptSequence && event.type === "load"), "page_reloaded");
        const received = receipt.events.find(event => event.sequence > receiptSequence &&
          event.type === "input" && event.page === input.page && event.value === input.value + inputSuffix);
        if (received && advanced(await sample(deadline), beforeInputVideo)) {
          // Input and fresh frames must both arrive within the shared deadline.
          evidence.input = { before_length: input.value.length, after_length: received.value.length, sequence: received.sequence };
          evidence.recovery_ms = Math.round(clock.now() - restored);
          break;
        }
      }
      await pause(Math.min(POLL_MS, Math.max(0, deadline - clock.now())));
    }
  } catch (error) {
    failure = error instanceof RecoveryFailure ? error.message : "observation_failed";
  } finally {
    evidence.detach.attempted = true;
    try {
      await within(clock.now() + WINDOW_MS, "detach_deadline", () => cdp.detach());
      evidence.detach.ok = true;
    } catch { failure ||= "detach_failed"; }
    if (stop) {
      try { await within(clock.now() + WINDOW_MS, "observer_stop_deadline", stop); }
      catch { failure ||= "observer_stop_failed"; }
    }
  }
  if (forbidden) failure ||= "cleanup_or_replacement_requested";
  if (failure) {
    evidence.failure = failure;
    throw Object.assign(new Error(`Browser recovery diagnostic failed: ${failure}`), { evidence });
  }
  evidence.ok = true;
  return evidence;
}
