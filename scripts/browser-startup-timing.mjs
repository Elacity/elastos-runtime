#!/usr/bin/env node
// One native owner trace. Viewer timing comes from the existing qualification observer.
import { closeSync, openSync, readSync } from "node:fs";
import { pathToFileURL } from "node:url";

const PREFIX = "browser-vz-engine-supervisor stage=";
const MAX_BYTES = 8 * 1024 * 1024;
const U128_MAX = (1n << 128n) - 1n;
const VIEWER_METHOD = "Runtime open request to first usable product video callback; host observation upper bound";
const STAGES = [
  "read_request", "prepare_rootfs_start", "prepared_rootfs",
  "provider_init_start", "provider_init_done", "load_vm_start", "load_vm_done",
  "start_vm_start", "start_vm_done", "transport_bootstrap_start", "transport_bootstrap_done",
  "open_guest_page_start", "open_guest_page_done",
];
const INTERVALS = {
  rootfs_prepare: ["prepare_rootfs_start", "prepared_rootfs"],
  provider_init: ["provider_init_start", "provider_init_done"],
  load_vm: ["load_vm_start", "load_vm_done"],
  start_vm: ["start_vm_start", "start_vm_done"],
  transport_bootstrap: ["transport_bootstrap_start", "transport_bootstrap_done"],
  open_guest_page: ["open_guest_page_start", "open_guest_page_done"],
};
const requireEvidence = (condition, code) => { if (!condition) throw new Error(code); };

function viewerTiming(witness) {
  if (witness == null) return null;
  requireEvidence(witness.clock === "host_monotonic" && witness.method === VIEWER_METHOD &&
    typeof witness.page_id === "string" && /^[A-Za-z0-9:_-]{1,256}$/.test(witness.page_id) &&
    Number.isFinite(witness.usable_frame_ms) && witness.usable_frame_ms > 0 &&
    Number.isFinite(witness.width) && witness.width > 0 &&
    Number.isFinite(witness.height) && witness.height > 0,
  "viewer_launch_witness_invalid");
  return { clock: witness.clock, method: witness.method, page_id: witness.page_id,
    usable_frame_ms: witness.usable_frame_ms, width: witness.width, height: witness.height };
}

export function parseStartupTiming(text, viewerLaunch = null) {
  requireEvidence(typeof text === "string" && Buffer.byteLength(text) <= MAX_BYTES, "startup_trace_size_bound");
  const seen = new Map(), stages = [];
  let previous = null, previousRequired = null;
  for (const line of text.split(/\r?\n/)) {
    if (!line.startsWith(PREFIX)) continue;
    const label = line.slice(PREFIX.length).split(" ", 1)[0];
    // Overlay and shutdown diagnostics also use this prefix, without a clock.
    if (!STAGES.includes(label) && !line.includes(" elapsed_us=")) continue;
    const match = /^browser-vz-engine-supervisor stage=([a-z][a-z0-9_]*) elapsed_us=(0|[1-9][0-9]*)(?: .*)?$/.exec(line);
    requireEvidence(match && match[2].length <= 39, "startup_trace_line_invalid");
    const [, stage, raw] = match, elapsed = BigInt(raw);
    requireEvidence(elapsed <= U128_MAX, "startup_trace_time_range");
    requireEvidence(previous === null || elapsed >= previous, "startup_trace_time_decreased");
    previous = elapsed;
    requireEvidence(seen.size > 0 || stage === "read_request", "startup_trace_start_missing");
    if (!STAGES.includes(stage)) continue;
    requireEvidence(!seen.has(stage), "startup_trace_duplicate:" + stage);
    requireEvidence(stage === STAGES[seen.size], "startup_trace_order:expected_" + STAGES[seen.size]);
    seen.set(stage, elapsed);
    stages.push({ stage, elapsed_us: raw,
      since_previous_stage_us: previousRequired === null ? "0" : String(elapsed - previousRequired) });
    previousRequired = elapsed;
  }
  requireEvidence(seen.size === STAGES.length, "startup_trace_missing:" + STAGES[seen.size]);
  const duration = (start, end) => String(seen.get(end) - seen.get(start));
  return {
    schema: "elastos.browser.startup-timing/v1",
    host: {
      clock: "process_monotonic",
      total_us: duration("read_request", "open_guest_page_done"),
      durations_us: Object.fromEntries(Object.entries(INTERVALS).map(([name, pair]) => [name, duration(...pair)])),
      stages,
    },
    // This separate clock measures Runtime open to usable video, not VM boot time.
    viewer: viewerTiming(viewerLaunch),
  };
}

function readBounded(path, limit, code) {
  const fd = openSync(path, "r"), chunks = [];
  let size = 0;
  try {
    for (;;) {
      const chunk = Buffer.allocUnsafe(Math.min(65536, limit + 1 - size));
      const count = readSync(fd, chunk, 0, chunk.length);
      if (count === 0) return Buffer.concat(chunks, size).toString("utf8");
      size += count;
      requireEvidence(size <= limit, code);
      chunks.push(chunk.subarray(0, count));
    }
  } finally { closeSync(fd); }
}

function main(args) {
  const usage = "Usage: node scripts/browser-startup-timing.mjs <owner.log> [--viewer-launch <launch.json>]\n" +
    "launch.json is the existing qualification.launch witness. Host microseconds are decimal strings.\n";
  if (args.length === 1 && args[0] === "--help") { process.stdout.write(usage); return; }
  requireEvidence((args.length === 1 || (args.length === 3 && args[1] === "--viewer-launch")) &&
    !args[0].startsWith("-"), usage.trim());
  const text = readBounded(args[0], MAX_BYTES, "startup_trace_size_bound");
  let viewer = null;
  if (args.length === 3) {
    const raw = readBounded(args[2], 16384, "viewer_launch_size_bound");
    viewer = JSON.parse(raw);
  }
  process.stdout.write(JSON.stringify(parseStartupTiming(text, viewer), null, 2) + "\n");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try { main(process.argv.slice(2)); }
  catch (error) { process.stderr.write(`${error.message}\n`); process.exitCode = 1; }
}
