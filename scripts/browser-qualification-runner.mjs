#!/usr/bin/env node
import { createHash } from "node:crypto";
import { spawn, execFile } from "node:child_process";
import { open, lstat, realpath } from "node:fs/promises";
import { readFileSync, writeFileSync, mkdirSync, statfsSync, statSync, openSync, writeSync, fsyncSync, closeSync } from "node:fs";
import { request } from "node:http";
import { dirname, resolve, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { promisify } from "node:util";
import { validatePlan, validateAttempt, cleanControl, sha256, boundedJson,
  requireEvidence, QUALIFICATION_SCHEMA } from "./browser-qualification-audit.mjs";
import { candidateFingerprint, validateInstallation, validateLiveIdentity } from "./browser-qualification-audit.mjs";

const execute = promisify(execFile);
const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const MAX_EVIDENCE = 64 * 1024 * 1024;
const RECEIPT_RESERVE = 256 * 1024;
const STATS = ["dev", "ino", "size", "mtimeNs", "ctimeNs"];
const identity = stat => STATS.map(k => String(stat[k])).join(":");
export function planTemplate() {
  return { schema: "elastos.browser.qualification-plan/v1", mode: "lifecycle", count: 100,
    probe: false, require_operator: true,
    candidate: { platform: "darwin-arm64", source_commit: "", source_tree: "",
      artifacts: ["runtime", "browser_ui", "engine_adapter", "image_manifest", "rootfs", "kernel",
        "initrd", "host_helper", "relay", "components", "installation_review"].map(role => ({ role, path: "", sha256: "" })) },
    runtime: { base_url: "", browser_ui_url: "", fixture_origin: "", profile: "", control_socket: "", operator_coords: "",
      browser_executable: "", node_path: "", viewer_version: "", headed: true, reuse_signed_home: false, engine_id: "", exit_id: "",
      resource_roots: [{ role: "runtime", pid: null, start: "" }, { role: "vm_control", pid: null, start: "" },
        { role: "exit_relay", pid: null, start: "" }] },
    network: { profile: "local", control_rtt_ms: null },
    limits: { rss_bytes: null, cpu_percent: null, fps: null, width: null, height: null } };
}
export async function freezeArtifacts(artifacts, signal) {
  const frozen = [];
  for (const artifact of artifacts) {
    requireEvidence(await realpath(artifact.path) === artifact.path, "candidate_symlink_rejected");
    const before = await lstat(artifact.path, { bigint: true });
    requireEvidence(before.isFile(), "candidate_not_regular");
    const file = await open(artifact.path, "r");
    try {
      requireEvidence(identity(await file.stat({ bigint: true })) === identity(before), "candidate_replaced_during_open");
      const hash = createHash("sha256"), chunk = Buffer.alloc(65536);
      for (;;) {
        requireEvidence(!signal?.aborted, "cancelled");
        const { bytesRead } = await file.read(chunk, 0, chunk.length, null);
        if (!bytesRead) break;
        hash.update(chunk.subarray(0, bytesRead));
      }
      requireEvidence(hash.digest("hex") === artifact.sha256 &&
        identity(await file.stat({ bigint: true })) === identity(before) &&
        identity(await lstat(artifact.path, { bigint: true })) === identity(before), "candidate_hash_or_identity_changed");
      frozen.push({ ...artifact, identity: identity(before) });
    } finally { await file.close(); }
  }
  return frozen;
}
export async function checkArtifacts(frozen) {
  for (const file of frozen) requireEvidence(await realpath(file.path) === file.path &&
    identity(await lstat(file.path, { bigint: true })) === file.identity, "candidate_changed:" + file.role);
}
export function executableInode(output, expectedPath) {
  let inode;
  for (const line of output.split("\n")) {
    if (line.startsWith("f")) inode = undefined;
    if (line.startsWith("i")) inode = line.slice(1);
    if (line === "n" + expectedPath && inode) return inode;
  }
  throw new Error("runtime_executable_not_mapped");
}
export async function liveCandidate(plan, frozen) {
  const root = plan.runtime.resource_roots.find(r => r.role === "runtime");
  const runtime = frozen.find(f => f.role === "runtime");
  const run = args => execute("/usr/sbin/lsof", args, { timeout: 3000, maxBuffer: 256 * 1024 });
  const { stdout: executable } = await run(["-a", "-p", String(root.pid), "-d", "txt", "-Ffin"]);
  const { stdout: listener } = await run(["-nP", "-iTCP:" + new URL(plan.runtime.base_url).port, "-sTCP:LISTEN", "-Fp"]);
  const { stdout: birth } = await execute("/bin/ps", ["-p", String(root.pid), "-o", "lstart="], { timeout: 3000 });
  const response = await fetch(plan.runtime.browser_ui_url, { redirect: "error", signal: AbortSignal.timeout(3000) });
  requireEvidence(response.ok, "served_browser_identity_failed");
  const hash = createHash("sha256"); let size = 0;
  for await (const chunk of response.body) {
    size += chunk.length; requireEvidence(size <= 1024 * 1024, "served_browser_identity_bound"); hash.update(chunk);
  }
  const live = { runtime_pid: root.pid, runtime_start: birth.trim().replace(/\s+/g, " "),
    runtime_executable: runtime.path, runtime_inode: executableInode(executable, runtime.path),
    listening_pids: [...new Set(listener.split("\n").filter(l => /^p\d+$/.test(l)).map(l => Number(l.slice(1))))],
    browser_ui_url: plan.runtime.browser_ui_url, browser_ui_sha256: hash.digest("hex") };
  validateLiveIdentity(plan, frozen, live); return live;
}
export function diskAvailable(path) {
  const s = statfsSync(path, { bigint: true });
  requireEvidence(s.blocks > 0n && s.bavail * 10n >= s.blocks, "disk_below_ten_percent");
  requireEvidence(s.bavail * s.bsize >= BigInt(MAX_EVIDENCE), "evidence_disk_bound");
}
export function controlStatus(socketPath) {
  return new Promise((resolve, reject) => {
    const req = request({ socketPath, path: "/status", method: "GET", agent: false }, response => {
      const chunks = []; let size = 0;
      response.on("data", chunk => {
        size += chunk.length;
        if (size > 65536) req.destroy(new Error("control_response_bound"));
        else chunks.push(chunk);
      });
      response.on("error", reject);
      response.on("end", () => {
        clearTimeout(timer);
        try {
          requireEvidence(response.statusCode === 200, "control_status_failed");
          resolve(JSON.parse(Buffer.concat(chunks).toString("utf8")));
        } catch (e) { reject(e); }
      });
    });
    const timer = setTimeout(() => req.destroy(new Error("control_status_deadline")), 3000);
    req.on("error", e => { clearTimeout(timer); reject(e); }); req.end();
  });
}
export function parseProcesses(text) {
  return text.split("\n").filter(x => x.trim()).map(line => {
    const m = line.match(/^\s*(\d+)\s+(\d+)\s+(\d+)\s+([\d.]+)\s+(.+)$/);
    requireEvidence(m, "process_metrics_parse");
    return { pid: Number(m[1]), ppid: Number(m[2]), rss_bytes: Number(m[3]) * 1024,
      cpu_percent: Number(m[4]), start: m[5].trim().replace(/\s+/g, " ") };
  });
}
export function ownedProcesses(rows, roots, tracked) {
  for (const root of roots) requireEvidence(rows.some(p => p.pid === root.pid && p.start === root.start),
    "resource_owner_replaced");
  const owned = new Map(rows.filter(p => roots.some(r => r.pid === p.pid) ||
    tracked.has(p.pid + ":" + p.start)).map(p => [p.pid, p]));
  let changed;
  do {
    changed = false;
    for (const p of rows) if (!owned.has(p.pid) && owned.has(p.ppid)) { owned.set(p.pid, p); changed = true; }
  } while (changed);
  requireEvidence(owned.size <= 128, "owned_process_bound");
  for (const p of owned.values()) tracked.add(p.pid + ":" + p.start);
  requireEvidence(tracked.size <= 4096, "process_history_bound");
  return [...owned.values()];
}
async function resources(roots, tracked, child = {}) {
  const { stdout } = await execute("/bin/ps", ["-axo", "pid=,ppid=,rss=,%cpu=,lstart="],
    { timeout: 3000, maxBuffer: 512 * 1024, env: { ...process.env, LC_ALL: "C" } });
  const processes = parseProcesses(stdout);
  if (child.pid && !child.identity) {
    const row = processes.find(p => p.pid === child.pid);
    if (row) { child.identity = row.pid + ":" + row.start; tracked.add(child.identity); }
  }
  const rows = ownedProcesses(processes, roots, tracked);
  return { identity_verified: true, process_ids: rows.map(r => r.pid + ":" + r.start).sort(),
    rss_bytes: rows.reduce((n, p) => n + p.rss_bytes, 0),
    cpu_percent: rows.reduce((n, p) => n + p.cpu_percent, 0) };
}
export function journeyEnvironment(plan, index) {
  // Matches the coordinator's proven run-installed-journey.py. Explicit values
  // keep unrelated inherited Home test modes out of this one journey.
  const env = { ...process.env };
  for (const key of Object.keys(env)) if (key.startsWith("HOME_VIRTUAL_AUTH_")) delete env[key];
  const p = plan.runtime;
  Object.assign(env, {
    ELASTOS_BASE_URL: p.base_url, HOME_URL: p.base_url.replace(/\/$/, "") + "/apps/home/",
    HOME_VIRTUAL_AUTH_NAME: "Browser qualification controlled Mac",
    HOME_VIRTUAL_AUTH_SYSTEM: "0", HOME_VIRTUAL_AUTH_SHELL_SWITCH: "0",
    HOME_VIRTUAL_AUTH_BROWSER: "1", HOME_VIRTUAL_AUTH_BROWSER_SUMMARY: "1",
    HOME_VIRTUAL_AUTH_BROWSER_EMBEDDED_UI_INPUT: "1",
    HOME_VIRTUAL_AUTH_BROWSER_CONTROLLED_JOURNEY: "1",
    HOME_VIRTUAL_AUTH_BROWSER_CONTROLLED_MEDIA: "1",
    HOME_VIRTUAL_AUTH_BROWSER_CONTROLLED_INSPECTION: "1",
    HOME_VIRTUAL_AUTH_BROWSER_CONTROLLED_VIEWER_RELOAD: "1",
    HOME_VIRTUAL_AUTH_BROWSER_CONTROLLED_RECOVERY: "0",
    HOME_VIRTUAL_AUTH_BROWSER_CONTROLLED_TURN_TEST_HOME: "",
    HOME_VIRTUAL_AUTH_BROWSER_REQUIRE_VZ_TRANSPORT: "1",
    HOME_VIRTUAL_AUTH_BROWSER_FIXTURE_ORIGIN: p.fixture_origin,
    HOME_VIRTUAL_AUTH_PROFILE: p.profile,
    HOME_VIRTUAL_AUTH_CLEANUP: "0", HOME_VIRTUAL_AUTH_PRESERVE_PROFILE: "1",
    HOME_VIRTUAL_AUTH_REUSE_SIGNED_HOME: p.reuse_signed_home === true ? "1" : "0",
    HOME_VIRTUAL_AUTH_BROWSER_CONTROLLED_OPERATOR: "1",
    HOME_VIRTUAL_AUTH_BROWSER_OPERATOR_COORDS: p.operator_coords,
    HOME_VIRTUAL_AUTH_BROWSER_QUALIFICATION_MODE: plan.mode,
    HOME_VIRTUAL_AUTH_BROWSER_QUALIFICATION_DURATION_MS: String(["media", "mixed"].includes(plan.mode) ? plan.duration_ms : 0),
    HOME_VIRTUAL_AUTH_BROWSER_QUALIFICATION_ATTEMPT: String(index),
  });
  if (p.browser_executable) env.ELASTOS_BROWSER_EXECUTABLE = p.browser_executable;
  if (p.node_path) env.NODE_PATH = p.node_path;
  if (p.headed === true) env.HOME_VIRTUAL_AUTH_HEADED = "1";
  if (p.remote_exit_id) env.HOME_VIRTUAL_AUTH_BROWSER_REMOTE_EXIT_ID = p.remote_exit_id;
  return env;
}
export async function childProcessTable() {
  const { stdout } = await execute("/bin/ps", ["-axo", "pid=,ppid=,pgid=,lstart="],
    { timeout: 3000, maxBuffer: 512 * 1024, env: { ...process.env, LC_ALL: "C" } });
  return stdout.split("\n").filter(l => l.trim()).map(line => {
    const m = line.match(/^\s*(\d+)\s+(\d+)\s+(\d+)\s+(.+)$/);
    requireEvidence(m, "child_process_identity_parse");
    return { pid: Number(m[1]), ppid: Number(m[2]), pgid: Number(m[3]), start: m[4].trim().replace(/\s+/g, " ") };
  });
}
export function runJourneyChild(plan, index, { signal, log, sample, started = () => {} },
  { spawnChild = spawn, scan = childProcessTable, signalProcess = (pid, sig) => process.kill(pid, sig),
    graceMs = 90_000, killMs = 5000, drainMs = 1000, finalMs = 4000 } = {}) {
  return new Promise(resolve => {
    const born = Math.floor(Date.now() / 1000) * 1000;
    const child = spawnChild(process.execPath, ["scripts/home-passkey-virtual-auth-smoke.mjs"], {
      cwd: ROOT, env: journeyEnvironment(plan, index), detached: true,
      stdio: ["ignore", "pipe", "pipe", "ipc"],
    });
    started(child.pid);
    const owned = new Map(), cleanup = { signals: [], remaining: [], identity_errors: [] };
    let journey = null, viewer = null, forced = false, error = null, bytes = 0, stdout = "", code = null, childSignal = null;
    let grace, killTimer, drainTimer, terminalTimer, finished = false, settling = false, scanBusy = false, childExited = false;
    const buffers = { stdout: "", stderr: "" };
    const capture = async () => {
      const rows = await scan();
      const currentRoot = rows.find(p => p.pid === child.pid);
      // Discover identities only while the spawned child is still alive. A
      // table returned after its exit cannot confer authority over a reused PID.
      if (!childExited && currentRoot?.pgid === child.pid && Date.parse(currentRoot.start) >= born && !owned.has(child.pid)) {
        owned.set(child.pid, currentRoot.start);
      }
      const sameGroup = currentRoot && owned.get(child.pid) === currentRoot.start;
      let changed;
      do {
        changed = false;
        for (const p of rows) if (!childExited && !owned.has(p.pid) &&
          ((sameGroup && p.pgid === child.pid && Date.parse(p.start) >= born) ||
           rows.some(parent => parent.pid === p.ppid && owned.get(parent.pid) === parent.start))) {
          owned.set(p.pid, p.start); changed = true;
        }
      } while (changed);
      requireEvidence(owned.size <= 4096, "child_history_bound");
      if (childExited && rows.some(p => owned.get(p.pid) !== p.start &&
        (p.pid === child.pid || p.pgid === child.pid || p.ppid === child.pid ||
          rows.some(parent => parent.pid === p.ppid && owned.get(parent.pid) === parent.start))) &&
        !cleanup.identity_errors.includes("child_identity_unknown_after_exit")) {
        cleanup.identity_errors.push("child_identity_unknown_after_exit");
      }
      return rows.filter(p => owned.get(p.pid) === p.start);
    };
    const terminate = async sig => {
      try {
        const rows = await capture();
        requireEvidence(rows.length <= 128, "child_active_process_bound");
        for (const p of rows) {
          // Re-read before signalling: a reused PID never inherits ownership.
          const current = (await scan()).find(row => row.pid === p.pid && row.start === p.start);
          if (finished || !current) continue;
          try { signalProcess(p.pid, sig); cleanup.signals.push({ pid: p.pid, start: p.start, signal: sig }); }
          catch (e) { if (e.code !== "ESRCH") cleanup.identity_errors.push(e.code || e.message); }
        }
      } catch (e) { cleanup.identity_errors.push(e.message); }
    };
    const finish = () => {
      if (finished) return; finished = true;
      for (const timer of [deadline, interval, grace, killTimer, drainTimer, terminalTimer]) clearTimeout(timer);
      clearInterval(interval); signal.removeEventListener("abort", cancel);
      child.stdout.destroy?.(); child.stderr.destroy?.();
      if (child.connected) { try { child.disconnect(); } catch {} }
      try {
        const start = stdout.indexOf('{\n  "schema": "elastos.home.passkey-virtual-auth-smoke/v1"');
        const report = JSON.parse(stdout.slice(start)); if (report.ok === true) viewer = report.viewer;
      } catch { error ||= "journey_final_report_missing"; }
      resolve({ journey, viewer, child_exit: code, child_signal: childSignal, forced_cleanup: forced,
        child_cleanup: cleanup, failure: error || (signal.aborted ? "cancelled" : null) });
    };
    const settle = async () => {
      if (settling) return; settling = true;
      terminalTimer = setTimeout(() => { error ||= "child_cleanup_deadline"; finish(); }, finalMs);
      await terminate("SIGKILL");
      if (cleanup.signals.length) { forced = true; error ||= "child_forced_cleanup"; }
      try { cleanup.remaining = await capture(); }
      catch (e) { cleanup.identity_errors.push(e.message); }
      if (cleanup.remaining.length || cleanup.identity_errors.length) error ||= "child_cleanup_unconfirmed";
      finish();
    };
    const cancel = () => {
      if (grace || finished) return;
      error ||= signal.aborted ? "cancelled" : "journey_cancelled";
      if (child.connected) child.send({ schema: "elastos.browser.qualification-cancel/v1" }, () => {});
      grace = setTimeout(() => {
        forced = true; void terminate("SIGTERM");
        killTimer = setTimeout(() => { void settle(); }, killMs);
      }, graceMs);
    };
    signal.addEventListener("abort", cancel, { once: true });
    const duration = ["media", "mixed"].includes(plan.mode) ? plan.duration_ms : 0;
    const deadline = setTimeout(() => { error ||= "journey_deadline"; cancel(); }, duration + 300_000);
    const interval = setInterval(() => {
      if (scanBusy || settling || finished) return; scanBusy = true;
      capture().catch(e => { error ||= e.message; cancel(); }).finally(() => { scanBusy = false; });
    }, 1000);
    void capture().catch(e => { error ||= e.message; cancel(); });
    const handle = (name, chunk) => {
      if (finished) return;
      try {
        bytes += chunk.length; requireEvidence(bytes <= 1024 * 1024, "journey_log_bound"); log(chunk);
        if (name === "stdout") stdout += chunk.toString("utf8");
        buffers[name] += chunk.toString("utf8");
        requireEvidence(buffers[name].length <= 512 * 1024, "journey_line_bound");
        let split;
        while ((split = buffers[name].indexOf("\n")) !== -1) {
          const line = buffers[name].slice(0, split); buffers[name] = buffers[name].slice(split + 1);
          if (!line.startsWith('{"stage":"browser:controlled-journey-result"')) continue;
          requireEvidence(!journey, "duplicate_journey_result"); journey = JSON.parse(line).result;
        }
      } catch (e) { error ||= e.message; cancel(); }
    };
    child.stdout.on("data", chunk => handle("stdout", chunk)); child.stderr.on("data", chunk => handle("stderr", chunk));
    child.on("message", message => {
      if (finished) return;
      try {
        requireEvidence(message?.schema === "elastos.browser.qualification-sample/v1" &&
          Buffer.byteLength(JSON.stringify(message)) <= 16384, "qualification_ipc_invalid"); sample(message);
      } catch (e) { error ||= e.message; cancel(); }
    });
    child.once("error", () => { error ||= "journey_spawn_failed"; void settle(); });
    child.once("exit", (value, sig) => {
      childExited = true;
      if (!owned.has(child.pid)) cleanup.identity_errors.push("child_identity_unobserved_before_exit");
      code = value; childSignal = sig;
      drainTimer = setTimeout(() => { error ||= "child_pipe_drain_deadline"; forced = true; void settle(); }, drainMs);
    });
    child.once("close", (value, sig) => {
      childExited = true;
      if (!owned.has(child.pid) && !cleanup.identity_errors.includes("child_identity_unobserved_before_exit")) {
        cleanup.identity_errors.push("child_identity_unobserved_before_exit");
      }
      code = value; childSignal = sig; void settle();
    });
    if (signal.aborted) cancel();
  });
}
export async function runQualification(planPath, output, { signal = new AbortController().signal } = {}) {
  const planBytes = readFileSync(planPath); requireEvidence(planBytes.length <= 65536, "plan_size_bound");
  const plan = validatePlan(JSON.parse(planBytes)), parent = dirname(resolve(output));
  diskAvailable(parent);
  // Exclusive directory and files retain every failed attempt and prevent a
  // successful rerun from replacing failure evidence.
  mkdirSync(output, { mode: 0o700 });
  writeFileSync(join(output, "plan.json"), planBytes, { flag: "wx", mode: 0o600 });
  let used = planBytes.length;
  const write = (file, value, terminal = false) => {
    const bytes = Buffer.from(JSON.stringify(value, null, 2) + "\n");
    requireEvidence(used + bytes.length <= MAX_EVIDENCE - (terminal ? 0 : RECEIPT_RESERVE), "evidence_total_bound");
    writeFileSync(join(output, file), bytes, { flag: "wx", mode: 0o600 }); used += bytes.length;
    return { file, sha256: sha256(bytes) };
  };
  const receipt = { schema: QUALIFICATION_SCHEMA, plan, plan_sha256: sha256(planBytes),
    started_at: new Date().toISOString(), attempts: [], completed: false,
    candidate_unchanged: false, cancelled: false, product_accepted: false,
    retention: "Retain through independent B16 review; preserve all failed attempts." };
  const tracked = new Set(), frozen = [];
  let sampleFd, sampleHash = createHash("sha256"), sampleBytes = 0, sequence = 0;
  try {
    frozen.push(...await freezeArtifacts(plan.candidate.artifacts, signal));
    const coordinates = await realpath(plan.runtime.operator_coords);
    requireEvidence(statSync(coordinates).size <= 32768, "operator_coordinates_bound");
    frozen.push(...await freezeArtifacts([{ role: "operator_coordinates", path: coordinates,
      sha256: sha256(readFileSync(coordinates)) }], signal));
    // Source helpers are part of this run's exact evidence, independently of
    // the installed Runtime/image's potentially older source lineage.
    for (const path of ["scripts/home-passkey-virtual-auth-smoke.mjs",
      "scripts/lib/browser-journey-fixture.mjs", "scripts/lib/browser-journey-operator.mjs",
      "scripts/lib/browser-journey-audio.mjs", "scripts/lib/browser-journey-recovery.mjs",
      "scripts/lib/browser-open-failure.mjs",
      "scripts/lib/browser-journey-viewer-reload.mjs", "scripts/lib/browser-journey-turn-interruption.mjs",
      "scripts/lib/browser-qualification-observer.mjs", "scripts/browser-qualification-runner.mjs",
      "scripts/browser-qualification-audit.mjs"]) {
      const absolute = join(ROOT, path);
      frozen.push(...await freezeArtifacts([{ role: path, path: absolute, sha256: sha256(readFileSync(absolute)) }], signal));
    }
    receipt.candidate_fingerprint = candidateFingerprint(plan, frozen);
    receipt.frozen_inputs = frozen;
    const installationBytes = readFileSync(plan.candidate.artifacts.find(a => a.role === "installation_review").path);
    requireEvidence(installationBytes.length <= 65536 && sha256(installationBytes) ===
      plan.candidate.artifacts.find(a => a.role === "installation_review").sha256, "installation_receipt_bound_or_hash");
    validateInstallation(plan, JSON.parse(installationBytes));
    writeFileSync(join(output, "installation-review.json"), installationBytes, { flag: "wx", mode: 0o600 });
    used += installationBytes.length;
    receipt.live_before = await liveCandidate(plan, frozen);
    const health = await fetch(plan.runtime.fixture_origin + "/health", { signal: AbortSignal.timeout(3000) }).then(r => r.json());
    requireEvidence(health.schema === "elastos.browser.journey-fixture/v1" && health.qualification === "bounded-v1",
      "qualification_fixture_hook_missing");
    sampleFd = openSync(join(output, "samples.jsonl"), "wx", 0o600);
    const start = performance.now();
    const append = (kind, attempt, value) => {
      const line = Buffer.from(JSON.stringify({ sequence: ++sequence, at_ms: performance.now() - start,
        kind, attempt, value }) + "\n");
      requireEvidence(line.length <= 32768 && used + line.length <= MAX_EVIDENCE - RECEIPT_RESERVE, "evidence_total_bound");
      writeSync(sampleFd, line); sampleHash.update(line); sampleBytes += line.length; used += line.length;
      if (sequence % 6 === 0) fsyncSync(sampleFd);
    };
    const count = ["media", "mixed"].includes(plan.mode) ? 1 : plan.count;
    for (let i = 1; i <= count; i++) {
      requireEvidence(!signal.aborted, "cancelled"); diskAvailable(output); await checkArtifacts(frozen);
      const attempt = { schema: "elastos.browser.qualification-attempt/v1", index: i, mode: plan.mode, ok: false,
        candidate_fingerprint: receipt.candidate_fingerprint, started_at: new Date().toISOString() };
      const controller = new AbortController(), abort = () => controller.abort();
      signal.addEventListener("abort", abort, { once: true });
      let stopSampling = false, sampleTask, logFd, logHash = createHash("sha256"), logBytes = 0;
      const observedChild = {};
      try {
        attempt.before = await controlStatus(plan.runtime.control_socket);
        cleanControl(attempt.before, plan.mode === "warm");
        const baseline = await resources(plan.runtime.resource_roots, tracked);
        attempt.resource_peak = { ...baseline };
        attempt.active_vm_keys = [];
        attempt.observed_page_ids = [];
        logFd = openSync(join(output, "attempt-" + i + ".log"), "wx", 0o600);
        sampleTask = (async () => {
          while (!stopSampling && !controller.signal.aborted) {
            await checkArtifacts(frozen); diskAvailable(output);
            const value = await resources(plan.runtime.resource_roots, tracked, observedChild);
            attempt.resource_peak.rss_bytes = Math.max(attempt.resource_peak.rss_bytes, value.rss_bytes);
            attempt.resource_peak.cpu_percent = Math.max(attempt.resource_peak.cpu_percent, value.cpu_percent);
            requireEvidence(value.rss_bytes <= plan.limits.rss_bytes && value.cpu_percent <= plan.limits.cpu_percent,
              "resource_budget_exceeded");
            const control = await controlStatus(plan.runtime.control_socket);
            for (const id of control.page_ids || []) if (!attempt.observed_page_ids.includes(id)) {
              requireEvidence(attempt.observed_page_ids.length < 16, "page_identity_bound");
              attempt.observed_page_ids.push(id);
            }
            for (const s of control.lifecycle?.sessions || []) if (s.vm_key_hash && !attempt.active_vm_keys.includes(s.vm_key_hash)) {
              requireEvidence(attempt.active_vm_keys.length < 16, "vm_identity_bound");
              attempt.active_vm_keys.push(s.vm_key_hash);
            }
            append("resources", i, { ...value, control });
            await new Promise(resolve => { const timer = setTimeout(done, 5000);
              function done() { clearTimeout(timer); controller.signal.removeEventListener("abort", done); resolve(); }
              controller.signal.addEventListener("abort", done, { once: true });
            });
          }
        })().catch(error => { attempt.sampling_failure = error.message; controller.abort(); });
        const child = await runJourneyChild(plan, i, { signal: controller.signal, started: pid => { observedChild.pid = pid; },
          log: chunk => {
            requireEvidence(used + chunk.length <= MAX_EVIDENCE - RECEIPT_RESERVE, "evidence_total_bound");
            writeSync(logFd, chunk); logHash.update(chunk); logBytes += chunk.length; used += chunk.length;
          }, sample: value => append("media", i, value) });
        Object.assign(attempt, child);
        stopSampling = true; controller.abort(); await sampleTask;
        requireEvidence(!attempt.sampling_failure && !child.failure, attempt.sampling_failure || child.failure);
        attempt.after = await controlStatus(plan.runtime.control_socket); cleanControl(attempt.after);
        const after = await resources(plan.runtime.resource_roots, tracked);
        requireEvidence(after.process_ids.every(id => baseline.process_ids.includes(id)), "owned_process_residue");
        attempt.live_after = await liveCandidate(plan, frozen);
        await checkArtifacts(frozen); attempt.candidate_unchanged = true;
        attempt.ok = true; validateAttempt(attempt, plan);
      } catch (e) { attempt.ok = false; attempt.failure = e.message; controller.abort(); }
      finally {
        stopSampling = true; controller.abort(); await sampleTask;
        // Failure and cancellation retain the same bounded ownership checks.
        attempt.terminal_control = await controlStatus(plan.runtime.control_socket).catch(e => ({ error: e.message }));
        attempt.terminal_resources = await resources(plan.runtime.resource_roots, tracked).catch(e => ({ error: e.message }));
        signal.removeEventListener("abort", abort);
        if (logFd !== undefined) { fsyncSync(logFd); closeSync(logFd);
          attempt.log = { file: "attempt-" + i + ".log", bytes: logBytes, sha256: logHash.digest("hex") }; }
        attempt.finished_at = new Date().toISOString();
        receipt.attempts.push(write("attempt-" + i + ".json", attempt));
      }
      requireEvidence(attempt.ok, attempt.failure || "attempt_failed");
      console.log(JSON.stringify({ stage: "qualification_attempt_complete", mode: plan.mode, attempt: i, total: count }));
    }
    await checkArtifacts(frozen); receipt.live_after = await liveCandidate(plan, frozen);
    receipt.candidate_unchanged = true; receipt.completed = true;
  } catch (e) { receipt.failure = e.message; receipt.cancelled = signal.aborted || e.message === "cancelled"; }
  finally {
    if (sampleFd !== undefined) {
      fsyncSync(sampleFd); closeSync(sampleFd);
      receipt.samples = { file: "samples.jsonl", bytes: sampleBytes, records: sequence, sha256: sampleHash.digest("hex") };
    }
    receipt.finished_at = new Date().toISOString();
    write("qualification.json", receipt, true);
  }
  return receipt;
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const controller = new AbortController();
  const cancel = () => controller.abort();
  process.once("SIGINT", cancel); process.once("SIGTERM", cancel);
  try {
    if (process.argv.length === 3 && process.argv[2] === "--template") {
      console.log(JSON.stringify(planTemplate(), null, 2));
    } else {
    requireEvidence(process.argv.length === 6 && process.argv[2] === "--plan" && process.argv[4] === "--out",
      "Usage: node scripts/browser-qualification-runner.mjs --template | --plan <plan.json> --out <new-private-directory>");
    const result = await runQualification(resolve(process.argv[3]), resolve(process.argv[5]), { signal: controller.signal });
    console.log(JSON.stringify({ completed: result.completed, product_accepted: false, failure: result.failure || null }));
    if (!result.completed) process.exitCode = 1;
    }
  } catch (e) { console.error(JSON.stringify({ ok: false, code: e.message })); process.exitCode = 1; }
  finally { process.removeListener("SIGINT", cancel); process.removeListener("SIGTERM", cancel); }
}
