import assert from "node:assert/strict";
import { closeSync, ftruncateSync, mkdtempSync, openSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { parseStartupTiming } from "./browser-startup-timing.mjs";

const events = [
  ["read_request", 7], ["prepare_rootfs_start", 100], ["prepared_rootfs", 2100],
  ["provider_init_start", 2200], ["provider_init_done", 2250],
  ["load_vm_start", 2260], ["load_vm_done", 2460],
  ["start_vm_start", 2500], ["start_vm_done", 2600],
  ["transport_bootstrap_start", 2610], ["transport_bootstrap_done", 9000],
  ["open_guest_page_start", 9010], ["open_guest_page_done", 12007],
];
const trace = (rows = events) => rows.map(([stage, us]) =>
  `browser-vz-engine-supervisor stage=${stage} elapsed_us=${us}`).join("\n");
const viewer = { clock: "host_monotonic", page_id: "page-1", usable_frame_ms: 1500.25,
  method: "Runtime open request to first usable product video callback; host observation upper bound",
  width: 1280, height: 720, private_detail: "keep-private" };

test("reports stage durations and open completion without claiming a viewer frame", () => {
  const result = parseStartupTiming(trace());
  assert.equal(result.host.total_us, "12000");
  assert.deepEqual(result.host.durations_us, {
    rootfs_prepare: "2000", provider_init: "50", load_vm: "200", start_vm: "100",
    transport_bootstrap: "6390", open_guest_page: "2997",
  });
  assert.equal(result.host.stages[1].since_previous_stage_us, "93");
  assert.equal(result.viewer, null);
});

test("accepts other owner phases while dropping raw paths and unstructured log text", () => {
  const rows = [...events];
  rows.splice(1, 0, ["validated_request", 10]);
  const text = "unrelated stage=read_request elapsed_us=1\n" + trace(rows)
    .replace("elapsed_us=2100", "elapsed_us=2100 launch=/private/operator/rootfs") +
    "\nbrowser-vz-engine-supervisor stage=close_done elapsed_us=13000\n" +
    "browser-vz-engine-supervisor stage=shutdown_wait reason=guest_stopped state=Stopped\n";
  const result = parseStartupTiming(text.replaceAll("\n", "\r\n"));
  assert.equal(result.host.total_us, "12000");
  assert.equal(result.host.stages.length, 13);
  assert.doesNotMatch(JSON.stringify(result), /private|validated_request|close_done/);
});

test("uses exact u128 arithmetic beyond JavaScript's safe integer range", () => {
  const offset = (1n << 128n) - 1n - 12007n;
  const result = parseStartupTiming(trace(events.map(([stage, us]) => [stage, offset + BigInt(us)])));
  assert.equal(result.host.total_us, "12000");
  assert.equal(result.host.durations_us.transport_bootstrap, "6390");
});

test("allows equal clock ticks", () => {
  assert.equal(parseStartupTiming(trace(events.map(([stage]) => [stage, 0]))).host.total_us, "0");
});

test("rejects a truncated trace and a missing stage in the middle", () => {
  assert.throws(() => parseStartupTiming(trace(events.slice(0, -1))), /startup_trace_missing:open_guest_page_done/);
  assert.throws(() => parseStartupTiming(trace(events.filter(([stage]) => stage !== "load_vm_start"))),
    /startup_trace_order:expected_load_vm_start/);
});

test("rejects duplicate stages and concatenated sessions", () => {
  const rows = [...events]; rows.splice(3, 0, events[2]);
  assert.throws(() => parseStartupTiming(trace(rows)), /startup_trace_duplicate:prepared_rootfs/);
  assert.throws(() => parseStartupTiming(trace() + "\n" + trace()), /startup_trace_time_decreased/);
  assert.throws(() => parseStartupTiming(trace() + "\nbrowser-vz-engine-supervisor stage=read_request elapsed_us=12008"),
    /startup_trace_duplicate:read_request/);
});

test("rejects reordered phases even when their times increase", () => {
  const rows = events.map(row => [...row]);
  [rows[5][0], rows[6][0]] = [rows[6][0], rows[5][0]];
  assert.throws(() => parseStartupTiming(trace(rows)), /startup_trace_order:expected_load_vm_start/);
});

test("checks monotonic time on additional phases too", () => {
  const rows = [...events]; rows.splice(3, 0, ["detail", 2099]);
  assert.throws(() => parseStartupTiming(trace(rows)), /startup_trace_time_decreased/);
  assert.throws(() => parseStartupTiming("browser-vz-engine-supervisor stage=detail elapsed_us=0\n" + trace()),
    /startup_trace_start_missing/);
});

test("rejects malformed clock values and oversized evidence", () => {
  for (const value of ["-1", "1.5", "1e3", "NaN", "07", "9".repeat(40)]) {
    assert.throws(() => parseStartupTiming(trace().replace("elapsed_us=7", `elapsed_us=${value}`)),
      /startup_trace_line_invalid/);
  }
  assert.throws(() => parseStartupTiming(trace().replace("elapsed_us=7", `elapsed_us=${1n << 128n}`)),
    /startup_trace_time_range/);
  assert.throws(() => parseStartupTiming("x".repeat(8 * 1024 * 1024 + 1)), /startup_trace_size_bound/);
  assert.throws(() => parseStartupTiming("log browser-vz-engine-supervisor stage=read_request elapsed_us=0"),
    /startup_trace_missing:read_request/);
  assert.throws(() => parseStartupTiming(trace().replace(" elapsed_us=7", "")), /startup_trace_line_invalid/);
});

test("keeps the existing viewer witness separate and filters its fields", () => {
  const result = parseStartupTiming(trace(), viewer);
  assert.equal(result.host.total_us, "12000");
  assert.deepEqual(result.viewer, { clock: "host_monotonic", method: viewer.method, page_id: "page-1",
    usable_frame_ms: 1500.25, width: 1280, height: 720 });
  assert.doesNotMatch(JSON.stringify(result), /keep-private/);
  for (const change of [{ clock: "wall_clock" }, { method: "manual" }, { usable_frame_ms: 0 }, { usable_frame_ms: Infinity },
    { width: 0 }, { page_id: "" }]) {
    assert.throws(() => parseStartupTiming(trace(), { ...viewer, ...change }), /viewer_launch_witness_invalid/);
  }
});

test("CLI reads the trace and viewer witness and fails without emitting a partial report", t => {
  const dir = mkdtempSync(join(tmpdir(), "browser-startup-timing-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const log = join(dir, "owner.log"), launch = join(dir, "launch.json");
  writeFileSync(log, trace()); writeFileSync(launch, JSON.stringify(viewer));
  const script = fileURLToPath(new URL("./browser-startup-timing.mjs", import.meta.url));
  const run = args => spawnSync(process.execPath, [script, ...args], { encoding: "utf8" });
  const ok = run([log, "--viewer-launch", launch]);
  assert.equal(ok.status, 0, ok.stderr);
  assert.equal(JSON.parse(ok.stdout).viewer.usable_frame_ms, 1500.25);
  writeFileSync(log, trace(events.slice(0, -1)));
  const bad = run([log]);
  assert.equal(bad.status, 1); assert.equal(bad.stdout, "");
  assert.match(bad.stderr, /startup_trace_missing/);
  assert.equal(run(["--help"]).status, 0);
  assert.equal(run([]).status, 1);
});

test("CLI bounds descriptor reads for sparse trace and viewer files", t => {
  const dir = mkdtempSync(join(tmpdir(), "browser-startup-size-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const log = join(dir, "owner.log"), launch = join(dir, "launch.json");
  const sparse = (path, size) => {
    const fd = openSync(path, "w");
    try { ftruncateSync(fd, size); } finally { closeSync(fd); }
  };
  const script = fileURLToPath(new URL("./browser-startup-timing.mjs", import.meta.url));
  const run = args => spawnSync(process.execPath, [script, ...args], { encoding: "utf8" });
  sparse(log, 2 * 1024 * 1024 * 1024);
  const traceResult = run([log]);
  assert.equal(traceResult.status, 1);
  assert.equal(traceResult.stdout, "");
  assert.match(traceResult.stderr, /startup_trace_size_bound/);
  writeFileSync(log, trace());
  sparse(launch, 2 * 1024 * 1024 * 1024);
  const viewerResult = run([log, "--viewer-launch", launch]);
  assert.equal(viewerResult.status, 1);
  assert.equal(viewerResult.stdout, "");
  assert.match(viewerResult.stderr, /viewer_launch_size_bound/);
});
