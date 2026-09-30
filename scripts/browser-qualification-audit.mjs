#!/usr/bin/env node
// Installed qualification admission. Source/fixture execution cannot certify a device.
import { createHash } from "node:crypto";
import { readFileSync, statSync } from "node:fs";
import { resolve, dirname, basename } from "node:path";
import { pathToFileURL } from "node:url";
import { isDeepStrictEqual } from "node:util";
import { controlledTonePresent } from "./lib/browser-journey-audio.mjs";
import { browserJourneyTargetConfig } from "./lib/browser-journey-target.mjs";

export const QUALIFICATION_SCHEMA = "elastos.browser.qualification/v1";
export const MODES = ["lifecycle", "cold", "warm", "media", "mixed"];
export const TERMINAL_EFFECTS = ["page_absent", "child_absent", "vm_absent", "route_absent",
  "socket_absent", "transport_session_absent", "turn_process_absent", "turn_listener_absent",
  "turn_relay_ports_absent", "ordinary_vsock_bridge_absent", "media_vsock_bridge_absent",
  "bootstrap_vsock_bridge_absent", "hibernation_state_absent"];
export const requireEvidence = (condition, code) => { if (!condition) throw new Error(code); };
export const sha256 = bytes => createHash("sha256").update(bytes).digest("hex");
export function candidateFingerprint(plan, inputs) {
  requireEvidence(Array.isArray(inputs) && inputs.length <= 64 &&
    new Set(inputs.map(f => f.role)).size === inputs.length, "frozen_inputs_required");
  for (const a of plan.candidate.artifacts) requireEvidence(inputs.some(f =>
    f.role === a.role && f.path === a.path && f.sha256 === a.sha256 && typeof f.identity === "string"),
  "candidate_frozen_input_mismatch:" + a.role);
  requireEvidence(inputs.some(f => f.role === "operator_coordinates" && f.path === plan.runtime.operator_coords) &&
    ["scripts/browser-qualification-runner.mjs", "scripts/browser-qualification-audit.mjs",
      "scripts/lib/browser-qualification-observer.mjs", "scripts/lib/browser-journey-target.mjs",
      "scripts/home-passkey-virtual-auth-smoke.mjs"].every(role =>
      inputs.some(f => f.role === role && hash(f.sha256))), "frozen_harness_inputs_required");
  return sha256(JSON.stringify({ plan, inputs }));
}
export function validateInstallation(plan, review) {
  requireEvidence(review?.schema === "elastos.browser.qualification-installation/v1" && review.reviewed === true &&
    typeof review.reviewer === "string" && review.reviewer.length > 0 &&
    review.source_commit === plan.candidate.source_commit && review.source_tree === plan.candidate.source_tree,
  "installation_source_mismatch");
  for (const a of plan.candidate.artifacts.filter(a => a.role !== "installation_review")) {
    const installed = review.artifacts?.find(f => f.role === a.role);
    requireEvidence(installed?.path === a.path && installed.installed_sha256 === a.sha256 &&
      installed.built_sha256 === a.sha256 && /^[a-f0-9]{40}$/.test(installed.source_commit) &&
      /^[a-f0-9]{40}$/.test(installed.source_tree), "installation_artifact_mismatch:" + a.role);
  }
  const runtime = review.artifacts.find(f => f.role === "runtime");
  requireEvidence(runtime.source_commit === review.source_commit && runtime.source_tree === review.source_tree,
    "installation_runtime_source_mismatch");
}
export function validateLiveIdentity(plan, frozen, live) {
  const root = plan.runtime.resource_roots.find(r => r.role === "runtime");
  const binary = frozen.find(f => f.role === "runtime"), ui = frozen.find(f => f.role === "browser_ui");
  requireEvidence(root && live?.runtime_pid === root.pid && live.runtime_start === root.start &&
    live.runtime_executable === binary.path && live.runtime_inode === binary.identity.split(":")[1] &&
    live.listening_pids?.includes(root.pid) && live.listening_pids.every(pid => pid === root.pid) &&
    live.browser_ui_url === plan.runtime.browser_ui_url && live.browser_ui_sha256 === ui.sha256,
  "live_candidate_identity_mismatch");
}
export const finite = value => typeof value === "number" && Number.isFinite(value);
const hash = value => typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
export function boundedJson(path, max = 1024 * 1024) {
  requireEvidence(statSync(path).size <= max, "receipt_size_bound");
  return JSON.parse(readFileSync(path, "utf8"));
}
export function percentile(values, fraction = 0.95) {
  requireEvidence(values.length > 0 && values.every(value => finite(value) && value >= 0), "latency_samples_invalid");
  return [...values].sort((a, b) => a - b)[Math.ceil(values.length * fraction) - 1];
}
export function deriveMedia(rows, { page_id, run, engine_id, exit_id, mode = "media", parent_times }) {
  requireEvidence(rows.length >= 2 && rows.length <= 6000, "media_trace_required");
  requireEvidence(["media", "mixed"].includes(mode), "media_mode_required");
  let prior, priorWindow, cursor = rows[0].initial_cursor, gap = 0, hidden = 0, idle = 0;
  let active = 0, interaction = 0, lastWitness = cursor;
  const actions = [], activePhases = [], phaseLengths = { active: 240_000, idle: 30_000, hidden: 30_000 };
  // Measured start/read bounds account for RPC overhead. Allow at most 100 ms
  // additional monotonic clock error, and 3 s variation in parent IPC delivery.
  const clockError = 100, origin = rows[0].clock_origin;
  requireEvidence(origin && finite(origin.started_ms) && finite(origin.ready_ms) && origin.started_ms >= 0 &&
    origin.ready_ms >= origin.started_ms && origin.ready_ms - origin.started_ms <= 3000, "media_clock_origin");
  if (parent_times) requireEvidence(parent_times.length === rows.length && parent_times.every(finite), "media_parent_clock");
  requireEvidence(Number.isSafeInteger(cursor) && cursor >= 0, "fixture_initial_cursor_required");
  for (const [sampleIndex, row] of rows.entries()) {
    const s = row.snapshot;
    requireEvidence(row.schema === "elastos.browser.qualification-sample/v1" && row.page_id === page_id && row.run === run &&
      s?.page_id === page_id && s.engine_id === engine_id && s.exit_id === exit_id &&
      finite(s.document_id) && s.receiver_unchanged === true && ["active", "idle", "hidden"].includes(s.phase),
    "media_sample_binding");
    for (const v of [s.duration_ms, s.phase_started_ms, s.active_ms, s.interaction_ms, s.video?.frames, s.video?.total,
      s.video?.dropped, s.video?.max_frame_gap_ms, s.audio?.observed_ms, s.audio?.max_silence_ms]) {
      requireEvidence(finite(v) && v >= 0, "media_counter_invalid");
    }
    requireEvidence(s.interaction_ms <= s.active_ms && s.active_ms <= s.duration_ms &&
      s.video.dropped <= s.video.total && s.audio.invalid_samples === 0 &&
      s.audio.receiver_unchanged === true && (s.phase !== "hidden" || s.hidden === true), "media_phase_or_receiver_invalid");
    if (s.duration_ms > 1000) requireEvidence(s.audio.observed_ms >= s.active_ms - 100 &&
      s.audio.max_silence_ms < 100 && Math.abs(s.audio.tone_hz - 440) <= 40 &&
      s.video.max_frame_gap_ms < 250, "media_progress_or_budget_failed");
    const delta = s.duration_ms - (prior?.duration_ms || 0);
    requireEvidence(delta >= 0 && delta <= 10_000 && (prior || delta <= 5000), "media_sample_gap");
    gap = Math.max(gap, delta);
    requireEvidence(s.phase_started_ms <= s.duration_ms &&
      (mode !== "media" || s.phase === "active"), "media_phase_coverage");
    let previousPart = 0;
    if (!prior) requireEvidence(s.phase === "active" && s.phase_started_ms === 0, "media_phase_origin");
    else if (s.phase === prior.phase) requireEvidence(s.phase_started_ms === prior.phase_started_ms, "media_phase_restart");
    else {
      requireEvidence(mode === "mixed" && s.phase === { active: "idle", idle: "hidden", hidden: "active" }[prior.phase] &&
        s.phase_started_ms >= prior.duration_ms &&
        Math.abs(s.phase_started_ms - prior.phase_started_ms - phaseLengths[prior.phase]) <= 10_000,
      "media_phase_transition");
      previousPart = s.phase_started_ms - prior.duration_ms;
    }
    if (mode === "mixed") requireEvidence(s.duration_ms - s.phase_started_ms <= phaseLengths[s.phase] + 10_000,
      "media_phase_overrun");
    active += (prior?.phase !== "hidden" ? previousPart : 0) + (s.phase !== "hidden" ? delta - previousPart : 0);
    interaction += (prior?.phase === "active" ? previousPart : 0) + (s.phase === "active" ? delta - previousPart : 0);
    // Compare cumulative coverage, so per-sample tolerance cannot hide a long gap.
    requireEvidence(Math.abs(s.active_ms - active) <= 1 && Math.abs(s.interaction_ms - interaction) <= 1,
      "media_phase_coverage");
    const visibleDelta = active - (prior?.active_ms || 0);
    const delivered = s.video.frames - (prior?.video.frames || 0), total = s.video.total - (prior?.video.total || 0);
    requireEvidence(visibleDelta <= (Math.min(delivered, total) + 1) * Math.max(1, s.video.max_frame_gap_ms) + 0.001,
      "media_frame_progress_gap");
    if (prior) {
      requireEvidence(s.document_id === prior.document_id && s.active_ms >= prior.active_ms &&
        s.active_ms - prior.active_ms <= delta + 1 && s.interaction_ms >= prior.interaction_ms &&
        s.interaction_ms - prior.interaction_ms <= delta + 1 &&
        s.video.frames >= prior.video.frames && s.video.total >= prior.video.total && s.video.dropped >= prior.video.dropped &&
        s.audio.observed_ms >= prior.audio.observed_ms && s.video.width === prior.video.width && s.video.height === prior.video.height,
      "media_counters_or_document_changed");
    }
    if (s.phase === "hidden" && prior?.phase !== "hidden") hidden++;
    if (s.phase === "idle" && prior?.phase !== "idle") idle++;
    const window = row.action_window;
    requireEvidence(window && finite(window.started_ms) && finite(window.observed_ms) && window.started_ms >= 0 &&
      window.observed_ms >= window.started_ms && window.observed_ms - window.started_ms <= 10_000 &&
      (!priorWindow || window.started_ms === priorWindow.observed_ms), "interaction_sample_window");
    const read = row.snapshot_window;
    requireEvidence(isDeepStrictEqual(row.clock_origin, origin) && (!prior || window.started_ms >= origin.ready_ms) &&
      (prior || window.started_ms === origin.ready_ms) && read && finite(read.started_ms) && finite(read.observed_ms) &&
      read.started_ms >= window.started_ms && read.observed_ms >= read.started_ms &&
      read.observed_ms - read.started_ms <= 3000 && read.observed_ms <= window.observed_ms &&
      window.observed_ms - read.observed_ms <= 6000 &&
      s.duration_ms >= read.started_ms - origin.ready_ms - clockError &&
      s.duration_ms <= read.observed_ms - origin.started_ms + clockError, "media_observation_clock");
    if (parent_times) requireEvidence(Math.abs((parent_times[sampleIndex] - parent_times[0]) -
      (window.observed_ms - rows[0].action_window.observed_ms)) <= 3000 + clockError, "media_parent_clock");
    requireEvidence(Array.isArray(row.events), "fixture_events_required");
    for (const e of row.events) {
      requireEvidence(e.sequence === ++cursor && e.page === "nav" && ["input", "scroll"].includes(e.type),
        "fixture_event_gap_or_page");
    }
    requireEvidence(row.fixture_cursor === cursor, "fixture_cursor_mismatch");
    if (!prior || (s.phase === "active" && prior.phase !== "active")) {
      activePhases.push({ start: s.phase_started_ms, end: s.duration_ms, actions: [] });
    }
    if (s.phase === "active") activePhases.at(-1).end = s.duration_ms;
    else if (prior?.phase === "active") activePhases.at(-1).end = s.phase_started_ms;
    if (row.interaction) {
      requireEvidence(s.phase === "active", "interaction_phase_invalid");
      actions.push({ action: row.interaction, events: row.events, window, read });
      activePhases.at(-1).actions.push(row.interaction);
    }
    prior = s; priorWindow = window;
  }
  for (const [i, { action, events, window, read }] of actions.entries()) {
    const input = events.find(e => e.sequence === action.input?.sequence), scroll = events.find(e => e.sequence === action.scroll?.sequence);
    requireEvidence(action.index === i && action.page_id === page_id && action.run === run &&
      input?.type === "input" && input.value === action.expected && scroll?.type === "scroll" &&
      scroll.value === action.expected && input.sequence > lastWitness && scroll.sequence > input.sequence &&
      finite(action.scroll.before_y) && [-320, 320].includes(action.scroll.delta) &&
      (scroll.scroll_y - action.scroll.before_y) * Math.sign(action.scroll.delta) >= 100 &&
      [action.input, action.scroll].every(a => finite(a.started_ms) && finite(a.receipt_ms) &&
        a.receipt_ms >= a.started_ms && a.receipt_ms - a.started_ms <= 5000) &&
      action.input.started_ms >= window.started_ms && action.scroll.started_ms >= action.input.receipt_ms &&
      action.scroll.receipt_ms <= read.started_ms, "sustained_interaction_unconfirmed");
    lastWitness = scroll.sequence;
  }
  requireEvidence(actions.length >= Math.floor(prior.interaction_ms / 60_000), "sustained_interaction_missing");
  for (const phase of activePhases) {
    const first = phase.actions[0], last = phase.actions.at(-1);
    if (!first) { requireEvidence(phase.end - phase.start <= 10_000, "sustained_interaction_cadence"); continue; }
    requireEvidence(first.input.started_ms - origin.ready_ms - phase.start <= 10_000 + clockError &&
      first.input.started_ms - origin.started_ms >= phase.start - clockError &&
      phase.end - (last.input.started_ms - origin.started_ms) <= 70_000 + clockError,
    "sustained_interaction_cadence");
    // The 60 s schedule allows one 5 s sampling interval and one bounded 5 s action.
    for (let i = 1; i < phase.actions.length; i++) requireEvidence(
      phase.actions[i].input.started_ms - phase.actions[i - 1].input.started_ms <= 70_000,
      "sustained_interaction_cadence");
  }
  const video = { frames: prior.video.frames, total: prior.video.total, dropped: prior.video.dropped,
    fps: prior.active_ms > 0 ? prior.video.frames * 1000 / prior.active_ms : 0,
    drop_ratio: prior.video.total > 0 ? prior.video.dropped / prior.video.total : 0,
    max_frame_gap_ms: Math.max(...rows.map(r => r.snapshot.video.max_frame_gap_ms)),
    width: prior.video.width, height: prior.video.height };
  const audio = { observed_ms: prior.audio.observed_ms, max_silence_ms: Math.max(...rows.map(r => r.snapshot.audio.max_silence_ms)),
    receiver_unchanged: true };
  return { duration_ms: prior.duration_ms, active_ms: prior.active_ms, interaction_ms: prior.interaction_ms,
    samples: rows.length, interactions: actions.length, hidden_cycles: hidden, idle_cycles: idle,
    max_sample_gap_ms: gap, video, audio };
}
export function qualificationTarget(runtime) {
  requireEvidence(runtime.allow_remote_fixture === undefined || typeof runtime.allow_remote_fixture === "boolean",
    "remote_fixture_option_invalid");
  requireEvidence(runtime.fixture_admin_origin === undefined ||
    typeof runtime.fixture_admin_origin === "string",
  "fixture_admin_origin_invalid");
  requireEvidence(typeof runtime.fixture_origin === "string" && runtime.fixture_origin.length > 0,
    "fixture_origin_required");
  for (const key of ["engine_id", "exit_id"]) requireEvidence(typeof runtime[key] === "string" &&
    (runtime[key] === "" || /^[A-Za-z0-9:_-]{1,128}$/.test(runtime[key])), "selected_service_identity_required");
  requireEvidence(runtime.remote_exit_id === undefined || runtime.remote_exit_id === runtime.exit_id,
    "conflicting_legacy_exit_selection");
  return browserJourneyTargetConfig({
    HOME_VIRTUAL_AUTH_BROWSER_ENGINE_ID: runtime.engine_id,
    HOME_VIRTUAL_AUTH_BROWSER_FIXTURE_ORIGIN: runtime.fixture_origin,
    HOME_VIRTUAL_AUTH_BROWSER_FIXTURE_ADMIN_ORIGIN: runtime.fixture_admin_origin,
    HOME_VIRTUAL_AUTH_BROWSER_ALLOW_REMOTE_FIXTURE: runtime.allow_remote_fixture === true ? "1" : "0",
  });
}
export function qualificationVmIdentity(proof, pageId) {
  requireEvidence(proof?.schema === "elastos.browser.vz-transport-public-proof/v1" && proof.page_id === pageId &&
    typeof pageId === "string" && /^page:vz-[a-f0-9]{64}$/.test(pageId) &&
    /^browser-vm-[a-f0-9]{64}$/.test(proof.vm_id) &&
    /^sha256:[a-f0-9]{64}$/.test(proof.generation) && /^sha256:[a-f0-9]{64}$/.test(proof.binding_hash),
  "launch_transport_identity_required");
  return Object.fromEntries(["schema", "page_id", "vm_id", "generation", "binding_hash"].map(k => [k, proof[k]]));
}
export function validateLocalLaunchSample(sample, pageId, plan, before) {
  const control = sample?.control, owner = plan.runtime.resource_roots.find(p => p.role === "vm_control");
  requireEvidence(sample?.identity_verified === true && finite(sample.rss_bytes) && sample.rss_bytes >= 0 &&
    sample.rss_bytes <= plan.limits.rss_bytes && finite(sample.cpu_percent) && sample.cpu_percent >= 0 &&
    sample.cpu_percent <= plan.limits.cpu_percent && Array.isArray(sample.process_ids) &&
    plan.runtime.resource_roots.every(p => sample.process_ids.includes(p.pid + ":" + p.start)),
  "launch_resource_identity_required");
  requireEvidence(owner && control?.schema === "elastos.browser.vm-control-service.status/v1" && control.ok === true &&
    control.pid === owner.pid && before?.pid === owner.pid &&
    control.control_service?.schema === "elastos.browser.vm-control-service.identity/v1" &&
    typeof control.control_service.service_id === "string" && /^service:[a-f0-9]{64}$/.test(control.control_service.service_id) &&
    isDeepStrictEqual(control.control_service, before.control_service) &&
    hash(control.config_fingerprint) && control.config_fingerprint === before.config_fingerprint &&
    control.direct_network === false && control.network_mode === "runtime_net_only" &&
    control.active_pages > 0 && control.page_ids?.includes(pageId) &&
    control.lifecycle?.schema === "elastos.browser.lifecycle-status/v1" &&
    control.lifecycle.sessions?.some(s => s.phase === "ACTIVE_SESSION" && s.warm_vm === false &&
      s.page_id === "sha256:" + sha256(pageId).slice(0, 16)), "launch_control_identity_required");
}
export function validatePlan(plan) {
  requireEvidence(plan?.schema === "elastos.browser.qualification-plan/v1" && MODES.includes(plan.mode), "plan_schema_or_mode");
  requireEvidence(plan.mode !== "warm", "warm_conditioning_unsupported");
  requireEvidence(plan.candidate?.platform === "darwin-arm64" &&
    /^[a-f0-9]{40}$/.test(plan.candidate.source_commit) && /^[a-f0-9]{40}$/.test(plan.candidate.source_tree),
  "candidate_identity_required");
  const artifacts = plan.candidate.artifacts;
  requireEvidence(Array.isArray(artifacts) && artifacts.length >= 10 && artifacts.length <= 40 &&
    new Set(artifacts.map(a => a.role)).size === artifacts.length &&
    artifacts.every(a => /^[a-z][a-z0-9_-]{0,47}$/.test(a.role) && typeof a.path === "string" &&
      a.path.startsWith("/") && hash(a.sha256)), "candidate_artifacts_invalid");
  for (const role of ["runtime", "browser_ui", "engine_adapter", "image_manifest", "rootfs",
    "kernel", "initrd", "host_helper", "relay", "components", "installation_review"]) {
    requireEvidence(artifacts.some(a => a.role === role), "candidate_artifact_missing:" + role);
  }
  const runtime = plan.runtime;
  requireEvidence(runtime?.reuse_signed_home === undefined || typeof runtime.reuse_signed_home === "boolean",
    "signed_home_reuse_option_invalid");
  requireEvidence(typeof runtime?.viewer_version === "string" && runtime.viewer_version.length > 0 &&
    runtime.viewer_version.length < 120 && typeof runtime.headed === "boolean", "viewer_identity_required");
  qualificationTarget(runtime);
  for (const name of ["base_url"]) {
    const url = new URL(runtime?.[name]);
    requireEvidence(url.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname) &&
      !url.username && !url.password && url.pathname === "/" && !url.search && !url.hash, "task_loopback_origin_required");
  }
  const uiUrl = new URL(runtime.browser_ui_url);
  requireEvidence(uiUrl.origin === new URL(runtime.base_url).origin && uiUrl.pathname.startsWith("/apps/browser/") &&
    !uiUrl.username && !uiUrl.password && !uiUrl.hash, "served_browser_identity_url_required");
  for (const key of ["profile", "control_socket", "operator_coords"]) {
    requireEvidence(typeof runtime[key] === "string" && runtime[key].startsWith("/"), "task_path_required:" + key);
  }
  requireEvidence(Array.isArray(runtime.resource_roots) && runtime.resource_roots.length >= 2 &&
    runtime.resource_roots.length <= 16 && runtime.resource_roots.every(p =>
      Number.isSafeInteger(p.pid) && p.pid > 1 && typeof p.start === "string" && p.start.length < 80),
  "resource_root_identity_required");
  requireEvidence(runtime.resource_roots.filter(r => r.role === "runtime").length === 1,
    "runtime_process_role_required");
  requireEvidence(runtime.resource_roots.filter(r => r.role === "vm_control").length === 1,
    "vm_control_process_role_required");
  requireEvidence(plan.require_operator === true, "combined_operator_journey_required");
  requireEvidence(["local", "wan"].includes(plan.network?.profile) &&
    finite(plan.network.control_rtt_ms) && plan.network.control_rtt_ms >= 0, "network_profile_required");
  const l = plan.limits;
  requireEvidence(l && ["rss_bytes", "cpu_percent", "fps", "width", "height"].every(k => finite(l[k]) && l[k] > 0) &&
    l.fps <= 120 && l.width <= 4096 && l.height <= 4096, "declared_resource_media_limits_required");
  const count = plan.count ?? 100;
  requireEvidence(Number.isInteger(count) && count >= 1 && count <= 100, "iteration_bound");
  const duration = plan.duration_ms ?? (plan.mode === "mixed" ? 28_800_000 : 1_800_000);
  requireEvidence(Number.isInteger(duration) && duration >= 1000 && duration <= 28_800_000, "duration_bound");
  if (plan.probe !== true) {
    requireEvidence(["media", "mixed"].includes(plan.mode) || count === 100, "hundred_samples_required");
    requireEvidence(plan.mode !== "media" || duration >= 1_800_000, "thirty_minutes_required");
    requireEvidence(plan.mode !== "mixed" || duration >= 28_800_000, "eight_hours_required");
  }
  return { ...plan, count, duration_ms: duration, probe: plan.probe === true };
}
export function validateJourney(raw, requireOperator = true) {
  const j = raw?.controlled_journey;
  requireEvidence(raw?.display_mode === "webrtc_remote_display" &&
    j?.schema === "elastos.browser.controlled-journey/v1" && typeof j.run === "string", "product_journey_required");
  requireEvidence(!j.prior_open_settlements?.length, "prior_open_failure_requires_explanation");
  requireEvidence(!j.qualification || (j.qualification.open_attempts?.length === 1 &&
    j.qualification.open_attempts[0].outcome === "completed"), "open_retry_or_failure");
  requireEvidence(Array.isArray(j.pages) && j.pages.length === 2 &&
    j.pages.map(p => p.name).join(",") === "main,nav" &&
    j.pages.every(p => p.page_id === raw.page_id && p.load?.type === "load" &&
      p.video?.decoded?.decoded_frames > p.video?.ready?.decoded_frames), "engine_pages_and_decoded_progress_required");
  requireEvidence(controlledTonePresent(j.audio) && controlledTonePresent(j.audio_after_recovery), "decoded_audio_required");
  requireEvidence(j.input?.receipt?.events?.some(e => e.type === "input" && e.value === j.input.text) &&
    j.input.receipt.events.some(e => e.type === "scroll" && e.value === j.input.text && e.scroll_y >= 100),
  "actual_fixture_input_required");
  requireEvidence(j.inspection?.pages?.length > 1 &&
    j.inspection.after_navigation?.status === 409 &&
    j.inspection.after_navigation?.body?.code === "stale_inspection", "actual_inspection_and_stale_reference_required");
  requireEvidence(j.viewer_reload?.ok === true, "viewer_reload_required");
  if (requireOperator) requireEvidence(j.operator?.ok === true, "installed_operator_required");
  const close = j.close, receipt = close?.receipt;
  requireEvidence(close?.window_detached === true && receipt?.schema === "elastos.browser.close-result/v1" &&
    receipt.closed === true && receipt.already_closed !== true && receipt.page_id === raw.page_id &&
    typeof receipt.cleanup_id === "string" && receipt.cleanup_id.length > 0 &&
    receipt.cleanup?.ok === true && receipt.cleanup.action === "released_exact_runtime_browser_ownership" &&
    receipt.transport_proof?.page_id === raw.page_id &&
    TERMINAL_EFFECTS.every(k => receipt.terminal_effects?.[k] === true), "exact_thirteen_effect_close_required");
  const after = close.sessions_after_close;
  requireEvidence(after && ["active_sessions", "principal_sessions", "total_sessions", "launching_sessions",
    "engine_cleanup_obligations", "launch_reconciliation_obligations"].every(k => after[k] === 0) &&
    after.recoverable_page === null, "runtime_cleanup_required");
  return j;
}
export function cleanControl(value, warm = false) {
  requireEvidence(value?.schema === "elastos.browser.vm-control-service.status/v1" && value.ok === true &&
    value.direct_network === false && value.network_mode === "runtime_net_only" &&
    value.active_pages === 0 && value.pending_launches === 0 && value.page_ids?.length === 0 &&
    value.active_stream_ids?.length === 0 && value.pending_stream_ids?.length === 0, "control_cleanup_required");
  requireEvidence(warm ? value.warm_vms > 0 && value.active_vms === value.warm_vms :
    value.warm_vms === 0 && value.active_vms === 0 && value.lifecycle?.sessions?.length === 0,
  warm ? "warm_condition_unavailable" : "control_vm_residue");
}
export function validateAttempt(attempt, plan) {
  requireEvidence(plan.mode !== "warm", "warm_conditioning_unsupported");
  requireEvidence(attempt?.schema === "elastos.browser.qualification-attempt/v1" &&
    attempt.ok === true && attempt.child_exit === 0 && attempt.mode === plan.mode &&
    !attempt.failure && !attempt.sampling_failure && !attempt.forced_cleanup, "attempt_failed_or_wrong_mode");
  const j = validateJourney(attempt.journey, plan.require_operator);
  cleanControl(attempt.before, plan.mode === "warm");
  cleanControl(attempt.after);
  requireEvidence(attempt.candidate_unchanged === true && attempt.resource_peak?.identity_verified === true &&
    attempt.resource_peak.rss_bytes <= plan.limits.rss_bytes &&
    attempt.resource_peak.cpu_percent <= plan.limits.cpu_percent, "candidate_or_resource_budget");
  requireEvidence(attempt.viewer?.version === plan.runtime.viewer_version &&
    attempt.viewer.headed === plan.runtime.headed, "viewer_version_or_mode_changed");
  const q = j.qualification;
  requireEvidence(q?.schema === "elastos.browser.qualification-observation/v1", "qualification_observer_required");
  if (["cold", "warm"].includes(plan.mode)) requireEvidence(q.launch?.page_id === attempt.journey.page_id &&
    q.launch.clock === "host_monotonic" && finite(q.launch.usable_frame_ms) && q.launch.usable_frame_ms > 0,
  "monotonic_launch_witness_required");
  requireEvidence(q.services?.engine_id === plan.runtime.engine_id && q.services?.exit_id === plan.runtime.exit_id &&
    attempt.observed_page_ids?.includes(attempt.journey.page_id), "selected_service_or_local_vm_witness_missing");
  requireEvidence(isDeepStrictEqual(qualificationVmIdentity(q.vm_identity, attempt.journey.page_id),
    qualificationVmIdentity(j.close.receipt.transport_proof, attempt.journey.page_id)), "launch_close_vm_identity_mismatch");
  validateLocalLaunchSample(attempt.local_launch_sample, attempt.journey.page_id, plan, attempt.before);
  requireEvidence(attempt.after.pid === attempt.before.pid &&
    attempt.after.config_fingerprint === attempt.before.config_fingerprint &&
    isDeepStrictEqual(attempt.after.control_service, attempt.before.control_service), "closed_control_identity_changed");
  if (["media", "mixed"].includes(plan.mode)) {
    const o = q.observation;
    requireEvidence(o?.ok === true && o.duration_ms >= plan.duration_ms &&
      o.samples >= Math.floor(plan.duration_ms / 5000) &&
      o.max_sample_gap_ms <= 10_000 && o.cleanup?.probe_closed === true &&
      o.cleanup?.observer_stopped === true && o.interactions >= Math.floor(o.interaction_ms / 60_000),
    "continuous_workload_incomplete");
    requireEvidence(o.video.max_frame_gap_ms < 250 && o.video.fps >= 0.95 * plan.limits.fps &&
      o.video.total > 0 && o.video.dropped >= 0 && o.video.drop_ratio >= 0 && o.video.drop_ratio < 0.01 &&
      o.video.width === plan.limits.width && o.video.height === plan.limits.height &&
      o.audio.max_silence_ms < 100 && o.audio.observed_ms >= o.active_ms - 100 &&
      o.audio.receiver_unchanged === true, "sustained_media_budget_failed");
    if (plan.mode === "mixed") requireEvidence(o.hidden_cycles > 0 && o.idle_cycles > 0, "mixed_workload_phases_missing");
  }
  return j;
}
export function auditQualification(receiptPath) {
  const r = boundedJson(receiptPath);
  requireEvidence(r.schema === QUALIFICATION_SCHEMA, "qualification_receipt_schema");
  const plan = validatePlan(r.plan);
  requireEvidence(candidateFingerprint(plan, r.frozen_inputs) === r.candidate_fingerprint, "candidate_fingerprint_mismatch");
  const installationBytes = readFileSync(resolve(dirname(receiptPath), "installation-review.json"));
  requireEvidence(installationBytes.length <= 65536 && sha256(installationBytes) ===
    plan.candidate.artifacts.find(a => a.role === "installation_review").sha256, "installation_receipt_hash_mismatch");
  validateInstallation(plan, JSON.parse(installationBytes));
  validateLiveIdentity(plan, r.frozen_inputs, r.live_before);
  validateLiveIdentity(plan, r.frozen_inputs, r.live_after);
  const planPath = resolve(dirname(receiptPath), "plan.json");
  requireEvidence(statSync(planPath).size <= 65536, "plan_size_bound");
  const planBytes = readFileSync(planPath);
  requireEvidence(sha256(planBytes) === r.plan_sha256 &&
    isDeepStrictEqual(validatePlan(JSON.parse(planBytes)), plan), "plan_hash_or_content_mismatch");
  requireEvidence(hash(r.plan_sha256) && hash(r.candidate_fingerprint) &&
    r.completed === true && r.candidate_unchanged === true && r.cancelled === false && !r.failure, "run_incomplete");
  requireEvidence(!plan.probe, "probe_cannot_qualify");
  requireEvidence(r.samples?.file === "samples.jsonl" && hash(r.samples.sha256) &&
    Number.isSafeInteger(r.samples.records) && r.samples.records > 0, "sample_evidence_required");
  const samplePath = resolve(dirname(receiptPath), r.samples.file);
  requireEvidence(statSync(samplePath).size <= 64 * 1024 * 1024, "sample_file_bound");
  const sampleBytes = readFileSync(samplePath);
  requireEvidence(sampleBytes.length === r.samples.bytes && sha256(sampleBytes) === r.samples.sha256,
    "sample_file_hash_mismatch");
  const lines = sampleBytes.toString("utf8").trimEnd().split("\n");
  requireEvidence(lines.length === r.samples.records && lines.length <= 20_000, "sample_count_bound");
  const observed = new Map(); let lastAt = -1;
  for (let i = 0; i < lines.length; i++) {
    requireEvidence(Buffer.byteLength(lines[i]) <= 32768, "sample_line_bound");
    const row = JSON.parse(lines[i]);
    requireEvidence(row.sequence === i + 1 && finite(row.at_ms) && row.at_ms >= lastAt &&
      Number.isInteger(row.attempt) && row.attempt >= 1 && row.attempt <= 100 &&
      ["resources", "media"].includes(row.kind), "sample_sequence_or_kind");
    lastAt = row.at_ms;
    const rows = observed.get(row.attempt) || { resources: 0, resource_hashes: new Set(), media: [], parent_times: [] };
    if (row.kind === "resources") {
      requireEvidence(row.value?.identity_verified === true && finite(row.value.rss_bytes) &&
        row.value.rss_bytes <= plan.limits.rss_bytes && finite(row.value.cpu_percent) &&
        row.value.cpu_percent <= plan.limits.cpu_percent, "resource_sample_invalid");
      rows.resources++;
      rows.resource_hashes.add(sha256(JSON.stringify(row.value)));
    } else { rows.media.push(row.value); rows.parent_times.push(row.at_ms); }
    observed.set(row.attempt, rows);
  }
  const expected = ["media", "mixed"].includes(plan.mode) ? 1 : 100;
  requireEvidence(Array.isArray(r.attempts) && r.attempts.length === expected &&
    new Set(r.attempts.map(a => a.file)).size === expected, "attempt_count_or_duplicate");
  const ids = new Set(), latencies = [], pending = [];
  for (const [index, ref] of r.attempts.entries()) {
    requireEvidence(basename(ref.file) === ref.file && hash(ref.sha256), "attempt_reference_invalid");
    const path = resolve(dirname(receiptPath), ref.file);
    requireEvidence(statSync(path).size <= 1024 * 1024, "attempt_size_bound");
    const bytes = readFileSync(path);
    requireEvidence(bytes.length <= 1024 * 1024 && sha256(bytes) === ref.sha256, "attempt_hash_mismatch");
    const a = JSON.parse(bytes), j = validateAttempt(a, plan);
    validateLiveIdentity(plan, r.frozen_inputs, a.live_after);
    requireEvidence(a.index === index + 1 && a.candidate_fingerprint === r.candidate_fingerprint && !ids.has(j.run), "candidate_or_run_id_mismatch");
    const rows = observed.get(a.index);
    requireEvidence(rows?.resources > 0, "resource_observation_missing");
    requireEvidence(rows.resource_hashes.has(sha256(JSON.stringify(a.local_launch_sample))), "launch_sample_not_in_raw_evidence");
    requireEvidence(a.log && basename(a.log.file) === a.log.file && hash(a.log.sha256), "journey_log_required");
    const logPath = resolve(dirname(receiptPath), a.log.file);
    requireEvidence(statSync(logPath).size <= 1024 * 1024 && sha256(readFileSync(logPath)) === a.log.sha256,
      "journey_log_hash_mismatch");
    if (["media", "mixed"].includes(plan.mode)) {
      const derived = deriveMedia(rows.media, { page_id: a.journey.page_id, run: j.run,
        engine_id: plan.runtime.engine_id, exit_id: plan.runtime.exit_id, mode: plan.mode, parent_times: rows.parent_times });
      const reported = Object.fromEntries(Object.keys(derived).map(k => [k, j.qualification.observation[k]]));
      requireEvidence(isDeepStrictEqual(derived, reported), "media_summary_trace_mismatch");
      if (plan.mode === "mixed") requireEvidence(derived.duration_ms - derived.active_ms >= plan.duration_ms / 15 &&
        derived.active_ms - derived.interaction_ms >= plan.duration_ms / 15, "mixed_phase_duration_missing");
    }
    ids.add(j.run);
    if (["cold", "warm"].includes(plan.mode)) latencies.push(j.qualification.launch.usable_frame_ms);
  }
  const metrics = { samples: r.attempts.length };
  if (["cold", "warm"].includes(plan.mode)) {
    metrics.p95_usable_frame_ms = percentile(latencies);
    const local = plan.network.profile === "local";
    metrics.budget_ms = plan.mode === "cold" ? (local ? 5000 : 7000) : (local ? 1000 : 2000);
    requireEvidence(metrics.p95_usable_frame_ms <= metrics.budget_ms, "launch_percentile_budget_failed");
  }
  if (["media", "mixed"].includes(plan.mode)) pending.push("synchronized_receiver_av_offset_measurement", "input_to_visible_latency_measurement");
  return { schema: "elastos.browser.qualification-audit/v1", ok: pending.length === 0,
    completed_execution: true, mode: plan.mode, metrics, pending,
    product_accepted: false, full_acceptance_pending: "B01-B16, device matrix, manual UX, Wallet, other placements and unexplained failures",
    receipt_sha256: sha256(readFileSync(receiptPath)) };
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    requireEvidence(process.argv.length === 4 && process.argv[2] === "--receipt",
      "Usage: node scripts/browser-qualification-audit.mjs --receipt <qualification.json>");
    const result = auditQualification(process.argv[3]);
    console.log(JSON.stringify(result, null, 2)); if (!result.ok) process.exitCode = 1;
  } catch (error) { console.error(JSON.stringify({ ok: false, code: error.message })); process.exitCode = 1; }
}
