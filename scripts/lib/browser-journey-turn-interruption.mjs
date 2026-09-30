import { createHash } from "node:crypto";
import { execFile, execFileSync, spawn } from "node:child_process";
import { lstat, readFile, realpath } from "node:fs/promises";
import { request } from "node:http";
import { homedir } from "node:os";
import { basename, isAbsolute, join } from "node:path";
import { fileURLToPath } from "node:url";
import { isDeepStrictEqual, promisify } from "node:util";

const execute = promisify(execFile);
const modulePath = fileURLToPath(import.meta.url);
const WATCHDOG_MS = 7000; // Leave one second for bounded resume retries.
const MAX_STOP_MS = 8000;
const hash = value => `sha256:${createHash("sha256").update(String(value)).digest("hex")}`;
class TurnInterruptionError extends Error {}
const requireProof = (ok, code) => { if (!ok) throw new TurnInterruptionError(code); };
const safeErrno = error => ["ENOENT", "EACCES", "EPERM", "ENOTDIR", "ELOOP", "EINVAL",
  "ECONNREFUSED", "ECONNRESET", "ETIMEDOUT", "EPIPE", "ESRCH", "EMFILE", "ENFILE"].includes(error?.code)
  ? error.code : null;

function deadline(budget = {}, maximum = 5000) {
  const end = Math.min(performance.now() + (budget.timeoutMs ?? maximum), budget.deadlineMs ?? Infinity);
  return () => {
    requireProof(!budget.signal?.aborted, "turn_interruption_aborted");
    const remaining = Math.floor(end - performance.now());
    requireProof(remaining > 0, "turn_interruption_deadline");
    return Math.min(remaining, maximum);
  };
}

function parseProcess(line) {
  const match = line.trim().match(/^(\d+)\s+(\d+)\s+(\d+)\s+(\w{3}\s+\w{3}\s+\d+\s+\d\d:\d\d:\d\d\s+\d{4})\s+(\S+)\s+(.+)$/);
  requireProof(match, "turn_process_identity_unavailable");
  return { pid: Number(match[1]), ppid: Number(match[2]), uid: Number(match[3]),
    start: match[4].replace(/\s+/g, " "), state: match[5], command: match[6] };
}
const psArgs = pid => ["-ww", "-p", String(pid), "-o", "pid=", "-o", "ppid=", "-o", "uid=", "-o", "lstart=", "-o", "stat=", "-o", "command="];
// PPID can change if the owning launcher exits. The same orphan still needs CONT.
const identity = value => hash(JSON.stringify([value.pid, value.uid, value.start, value.command]));

function lsofRows(text) {
  let pid, row;
  const rows = [];
  for (const line of text.split("\n")) {
    if (line.startsWith("p")) { pid = Number(line.slice(1)); row = undefined; }
    if (line.startsWith("f")) row = undefined;
    if (line.startsWith("n") && pid > 1) { row = { pid, name: line.slice(1) }; rows.push(row); }
    if (line.startsWith("TST=") && row) row.state = line.slice(4);
  }
  return rows;
}

const system = {
  platform: process.platform, uid: process.getuid?.(), home: homedir(),
  realpath,
  async ownedFile(path, max = 65536) {
    const stat = await lstat(path);
    requireProof(stat.isFile() && stat.uid === process.getuid() && !(stat.mode & 0o077) &&
      stat.nlink === 1 && stat.size <= max, "turn_private_file_invalid");
    return readFile(path, "utf8");
  },
  async socket(path) {
    const stat = await lstat(path);
    requireProof(stat.isSocket() && stat.uid === process.getuid(), "turn_socket_invalid");
  },
  async command(program, args, remaining) {
    try { return (await execute(program, args, { encoding: "utf8", timeout: Math.min(1000, remaining()),
      maxBuffer: 2 * 1024 * 1024, env: { ...process.env, LC_ALL: "C" } })).stdout; }
    catch (error) { throw Object.assign(new TurnInterruptionError("turn_process_inspection_failed"), { code: safeErrno(error) }); }
  },
  async process(pid, remaining) { return parseProcess(await this.command("/bin/ps", psArgs(pid), remaining)); },
  async socketPids(path, remaining) {
    const canonical = await realpath(path);
    const rows = lsofRows(await this.command("/usr/sbin/lsof", ["-nP", "-Fpn", "--", path], remaining));
    return [...new Set(rows.filter(row => [path, canonical].includes(row.name)).map(row => row.pid))];
  },
  async status(socketPath, remaining) {
    return new Promise((resolve, reject) => {
      const req = request({ socketPath, path: "/status", method: "GET", agent: false }, response => {
        let body = "";
        response.setEncoding("utf8");
        response.on("data", chunk => {
          body += chunk;
          if (body.length > 65536) req.destroy(new Error("oversize"));
        });
        response.on("error", () => req.destroy());
        response.on("end", () => {
          clearTimeout(timer);
          try { requireProof(response.statusCode === 200, "turn_status_http_failure"); resolve(JSON.parse(body)); }
          catch { reject(new TurnInterruptionError("turn_status_invalid")); }
        });
      });
      const timer = setTimeout(() => req.destroy(new Error("deadline")), remaining());
      req.on("error", error => { clearTimeout(timer); reject(Object.assign(
        new TurnInterruptionError("turn_status_unavailable"), { code: safeErrno(error) })); });
      req.end();
    });
  },
  watchdog: startWatchdog,
};

function adapterControlSocket(doc, dataDir) {
  requireProof(doc && typeof doc === "object" && Array.isArray(doc.adapters) && doc.adapters.length > 0 &&
    doc.adapters.length <= 16, "turn_adapter_invalid");
  const matches = doc.adapters.filter(adapter => adapter?.kind === "chromium_microvm" &&
    typeof adapter?.supervisor?.control_socket_path === "string" &&
    adapter.supervisor.env?.ELASTOS_BROWSER_VM_DATA_DIR === dataDir &&
    adapter.supervisor.env?.ELASTOS_BROWSER_VM_CONTROL_SOCKET === adapter.supervisor.control_socket_path);
  requireProof(matches.length === 1 && isAbsolute(matches[0].supervisor.control_socket_path),
    "turn_adapter_ambiguous_or_absent");
  return matches[0].supervisor.control_socket_path;
}

async function bind(options, io, remaining, stage = () => {}) {
  stage("input");
  requireProof(io.platform === "darwin", "turn_interruption_requires_macos");
  const { testHome, pageId, runtimeOrigin } = options;
  const controlSocketPath = typeof options.controlSocketPath === "string" && options.controlSocketPath
    ? options.controlSocketPath : undefined;
  requireProof(typeof testHome === "string" && isAbsolute(testHome) &&
    typeof pageId === "string" && /^page:vz-[0-9a-f]{64}$/.test(pageId) &&
    (controlSocketPath === undefined || isAbsolute(controlSocketPath)), "turn_explicit_fixture_required");
  stage("home_path");
  const home = await io.realpath(testHome);
  stage("personal_home_path");
  requireProof(home !== await io.realpath(io.home) && home !== "/", "turn_personal_home_rejected");
  const dataDir = join(home, "Library/Application Support/elastos");
  stage("data_dir");
  requireProof(await io.realpath(dataDir) === dataDir, "turn_data_dir_redirected");
  stage("runtime_origin");
  const origin = new URL(runtimeOrigin);
  requireProof(origin.origin === runtimeOrigin && origin.protocol === "http:" &&
    ["localhost", "127.0.0.1", "[::1]"].includes(origin.hostname) &&
    !origin.username && !origin.password, "turn_runtime_origin_invalid");
  let controlSocket;
  if (controlSocketPath) {
    stage("adapter_config");
    const resolved = adapterControlSocket(
      JSON.parse(await io.ownedFile(join(dataDir, "config/browser-engine-adapter.json"))), dataDir);
    requireProof(resolved === controlSocketPath, "turn_control_socket_mismatch");
    controlSocket = resolved;
  } else {
    stage("restart_receipt");
    const receipt = JSON.parse(await io.ownedFile(join(dataDir, "receipts/mac-source-home-restart.json")));
    const homeUrl = new URL(receipt.home_url);
    requireProof(homeUrl.origin === origin.origin && homeUrl.pathname === "/apps/home/" &&
      !homeUrl.username && !homeUrl.password && !homeUrl.search && !homeUrl.hash &&
      receipt.schema === "elastos.mac-source-home-restart/v1" && receipt.ok === true && receipt.dry_run === false &&
      receipt.test_home === home && receipt.data_dir === dataDir, "turn_restart_receipt_mismatch");
    controlSocket = join(home, "run/browser.sock");
  }
  stage("control_socket");
  await io.socket(controlSocket);
  stage("control_status");
  const status = await io.status(controlSocket, remaining);
  requireProof(status.schema === "elastos.browser.vm-control-service.status/v1" && status.ok === true &&
    status.active_pages === 1 && status.pending_launches === 0 && status.page_ids?.length === 1 &&
    status.page_ids[0] === pageId && status.network_mode === "runtime_net_only" && status.direct_network === false &&
    Number.isSafeInteger(status.pid) && status.pid > 1, "turn_active_page_mismatch");
  stage("control_socket_process");
  const controlPids = await io.socketPids(controlSocket, remaining);
  requireProof(controlPids.length === 1 && controlPids[0] === status.pid, "turn_control_process_mismatch");
  stage("launch_journal");
  const journal = JSON.parse(await io.ownedFile(`${controlSocket}.launch-reconciliations.json`, 4 * 1024 * 1024));
  requireProof(journal?.schema === "elastos.browser.vm-control-service.launch-reconciliations/v1" &&
    Array.isArray(journal.records) && journal.records.length <= 128, "turn_launch_journal_invalid");
  const records = journal.records.filter(record => record?.launch?.page_id === pageId &&
    ["effect_acquired", "cleanup_pending"].includes(record.state));
  requireProof(records.length === 1 && records[0].state === "effect_acquired", "turn_launch_record_ambiguous_or_absent");
  const record = records[0], launch = record.launch, cleanup = record.cleanup_binding;
  const authority = launch.transport_authority;
  requireProof(record.schema === "elastos.browser.vm-control-service.launch-reconciliation/v1" &&
    record.effects?.page_acquired === true && record.effects?.vm_acquired === true &&
    authority?.schema === "elastos.browser.vz-transport-authority/v1" &&
    /^sha256:[0-9a-f]{64}$/.test(authority.binding_hash) && /^sha256:[0-9a-f]{64}$/.test(authority.generation) &&
    authority.page_id === pageId && typeof authority.vm_id === "string" && authority.vm_id.length > 0 &&
    launch.vm_id === authority.vm_id && launch.lifecycle_generation === authority.generation &&
    typeof authority.egress?.stream_id === "string" && launch.stream_id === authority.egress.stream_id &&
    typeof authority.media?.stream_id === "string" && authority.media.stream_id !== launch.stream_id &&
    cleanup?.schema === "elastos.browser.engine-cleanup-binding/v2" && cleanup.page_id === pageId &&
    cleanup.generation === authority.generation && cleanup.stream_id === launch.stream_id &&
    isDeepStrictEqual(cleanup.transport_authority, authority), "turn_launch_binding_mismatch");
  const service = status.control_service, processBinding = cleanup.process;
  requireProof(service?.schema === "elastos.browser.vm-control-service.identity/v1" &&
    /^service:[0-9a-f]{64}$/.test(service.service_id) && service.control_socket_path === controlSocket &&
    (service.config_fingerprint === null || /^[0-9a-f]{64}$/.test(service.config_fingerprint)) &&
    isDeepStrictEqual(record.control_service, service) && isDeepStrictEqual(cleanup.control_service, service) &&
    cleanup.shutdown_socket_path === controlSocket, "turn_launch_control_service_mismatch");
  requireProof(processBinding?.schema === "elastos.browser.host-process-binding/v1" &&
    /^process:[0-9a-f]{64}$/.test(processBinding.ownership_id) &&
    Number.isSafeInteger(processBinding.pid) && processBinding.pid > 1 && processBinding.pid <= 0x7fffffff &&
    processBinding.stream_bridge_pid === null, "turn_launch_process_mismatch");
  const turnAuthority = authority.turn;
  requireProof(turnAuthority?.schema === "elastos.browser.vz-turn-authority/v1" &&
    ["127.0.0.1", "::1"].includes(turnAuthority.listen_host) &&
    Number.isInteger(turnAuthority.listen_port) && turnAuthority.listen_port > 0 && turnAuthority.listen_port <= 65535 &&
    isDeepStrictEqual(turnAuthority.protocols, ["turn", "tcp"]), "turn_launch_endpoint_invalid");
  const address = `${turnAuthority.listen_host === "::1" ? "[::1]" : turnAuthority.listen_host}:${turnAuthority.listen_port}`;
  requireProof(authority.media.target === `tcp://${address}`, "turn_launch_media_target_mismatch");
  const name = authority.binding_hash.slice(7, 39);
  stage("owner_binding");
  const owner = JSON.parse(await io.ownedFile(`/tmp/evzrc/${name}/owner.json`, 8192));
  requireProof(owner.schema === "elastos.browser.vz-socket-owner/v1" &&
    owner.binding_hash === authority.binding_hash && owner.page_id === pageId && owner.generation === authority.generation &&
    owner.vm_id === authority.vm_id && owner.stream_id === launch.stream_id && owner.media_stream_id === authority.media.stream_id,
  "turn_owner_binding_mismatch");
  const nativeSocket = `/tmp/evzrc/${name}/c.sock`;
  requireProof(cleanup.control_socket_path === nativeSocket, "turn_launch_native_socket_mismatch");
  stage("native_socket");
  await io.socket(nativeSocket);
  stage("native_socket_process");
  const nativePids = await io.socketPids(nativeSocket, remaining);
  requireProof(nativePids.length === 1 && nativePids[0] === processBinding.pid, "turn_native_process_ambiguous");
  stage("native_process");
  const native = await io.process(nativePids[0], remaining);
  stage("native_executable");
  const nativeExe = await io.realpath(join(dataDir, "bin/browser-vz-engine-supervisor"));
  const nativeCommand = join(dataDir, "bin/browser-vz-engine-supervisor");
  requireProof(native.uid === io.uid && (native.command === nativeCommand || native.command.startsWith(`${nativeCommand} `)),
    "turn_native_executable_mismatch");
  stage("native_executable_map");
  const executableRows = lsofRows(await io.command("/usr/sbin/lsof", ["-nP", "-a", "-p", String(native.pid), "-d", "txt", "-Fpn"], remaining));
  requireProof(executableRows.some(row => row.pid === native.pid && row.name === nativeExe), "turn_native_executable_mismatch");
  let ancestor = native;
  stage("native_ancestry");
  const ancestry = [identity(native)];
  for (let depth = 0; ancestor.pid !== status.pid && depth < 8; depth++) {
    requireProof(ancestor.ppid > 1, "turn_control_ancestry_mismatch");
    ancestor = await io.process(ancestor.ppid, remaining);
    requireProof(ancestor.uid === io.uid, "turn_control_ancestry_mismatch");
    ancestry.push(identity(ancestor));
  }
  requireProof(ancestor.pid === status.pid, "turn_control_ancestry_mismatch");
  // Native deletes this secret file after startup. Bind its exact retained argv
  // plus the native owner/socket/process chain; never read the deleted secret.
  const configPath = `/tmp/evzs/vz-${name}/turnserver.conf`;
  stage("turn_process_scan");
  const table = await io.command("/bin/ps", ["-ww", "-axo", "pid=,ppid=,uid=,lstart=,stat=,command="], remaining);
  const candidates = table.trim().split("\n").filter(line => line.endsWith(` -c ${configPath}`)).map(parseProcess);
  requireProof(candidates.length === 1, "turn_process_ambiguous_or_absent");
  const turn = candidates[0];
  const turnProgram = turn.command.slice(0, -` -c ${configPath}`.length);
  requireProof(turn.pid > 1 && turn.ppid === native.pid && turn.uid === io.uid &&
    isAbsolute(turnProgram) && basename(turnProgram) === "turnserver" && !/[TZ]/.test(turn.state), "turn_process_owner_mismatch");
  stage("turn_executable");
  const turnExe = await io.realpath(turnProgram);
  stage("turn_executable_map");
  const turnExecutableRows = lsofRows(await io.command("/usr/sbin/lsof", ["-nP", "-a", "-p", String(turn.pid), "-d", "txt", "-Fpn"], remaining));
  requireProof(turnExecutableRows.some(row => row.pid === turn.pid && row.name === turnExe), "turn_executable_mismatch");
  stage("turn_tcp_sockets");
  const tcp = lsofRows(await io.command("/usr/sbin/lsof", ["-nP", "-a", "-p", String(turn.pid), "-iTCP", "-FpnfT"], remaining));
  const listeners = [...new Set(tcp.filter(row => row.pid === turn.pid && row.state === "LISTEN").map(row => row.name))];
  requireProof(listeners.length === 1, "turn_listener_ambiguous_or_absent");
  requireProof(listeners[0] === address, "turn_listener_mismatch");
  requireProof(tcp.some(row => row.pid === turn.pid && row.name.startsWith(`${address}->`) && row.state === "ESTABLISHED"), "turn_tcp_connection_absent");
  remaining();
  stage("complete");
  return { turn, proof: { page: hash(pageId), binding: hash(owner.binding_hash), home: hash(home),
    origin: hash(runtimeOrigin), launch_config_path: hash(configPath), tcp_endpoint: hash(address),
    generation: hash(authority.generation), control_service: hash(JSON.stringify(service)),
    process_ownership: hash(processBinding.ownership_id), process: identity(turn), ancestry,
    native_executable: hash(nativeExe), turn_executable: hash(turnExe), tcp_listener: true, tcp_connection: true } };
}

/** Test-only, explicit testHome opt-in. Creation reads ownership; cut alone starts the watchdog.
 * cut/restore accept {signal, timeoutMs, deadlineMs}, using performance.now() deadlines.
 * Always call restore independently of CDP restoration, including when cut rejects.
 */
export async function createBrowserTurnInterruption(options) { return createDriver(options, system); }

async function createDriver(options, io) {
  const evidence = { schema: "elastos.browser.journey-turn-interruption/v1", bound: false,
    cut: false, restored: false, watchdog_ms: 8000 };
  const safeError = code => Object.assign(new Error(code), { evidence });
  let bindingStage = "input";
  const stage = value => { bindingStage = value; };
  function recordFailure(error) {
    evidence.failure = { stage: bindingStage,
      code: error instanceof TurnInterruptionError ? error.message : `turn_binding_${bindingStage}_failed`,
      errno: safeErrno(error) };
    return evidence.failure.code;
  }
  let bound;
  try { bound = await bind(options, io, deadline(), stage); }
  catch (error) { throw safeError(recordFailure(error)); }
  Object.assign(evidence, { bound: true, proof: bound.proof });
  let watchdog, attempted = false, restored;
  return {
    evidence,
    async cut(budget) {
      requireProof(!attempted, "turn_cut_already_attempted");
      attempted = true;
      try {
        const remaining = deadline(budget);
        const fresh = await bind(options, io, remaining, stage);
        stage("binding_compare");
        requireProof(JSON.stringify(fresh.proof) === JSON.stringify(bound.proof), "turn_binding_changed");
        stage("watchdog_start");
        watchdog = io.watchdog(fresh.turn);
        stage("watchdog_ready");
        await watchdog.ready(remaining);
        remaining();
        stage("watchdog_cut");
        const result = await watchdog.cut(remaining);
        Object.assign(evidence, result);
      } catch (error) { recordFailure(error); throw safeError("turn_cut_failed"); }
    },
    async restore(budget) {
      if (!watchdog) { evidence.restored = true; evidence.resume_reason = "not_started"; return; }
      if (!restored) restored = watchdog.restore(deadline(budget)).then(result => {
        Object.assign(evidence, result);
        requireProof(evidence.restored === true, "turn_restore_unconfirmed");
      }).catch(() => { evidence.restored = false; evidence.manual_recovery_required = true; throw safeError("turn_restore_failed"); });
      return restored;
    },
  };
}

function startWatchdog(target) {
  const child = spawn(process.execPath, [modulePath, "--turn-interruption-watchdog"],
    { detached: true, stdio: ["pipe", "pipe", "ignore"], env: { ...process.env, LC_ALL: "C" } });
  const messages = new Map(), waiters = new Map();
  let buffer = "", failure;
  const closed = new Promise(resolve => child.once("close", (code, signal) => resolve({ code, signal })));
  function rejectAll() {
    failure = new Error("turn_watchdog_disconnected");
    for (const waiter of waiters.values()) waiter.reject(failure);
    waiters.clear();
  }
  child.on("error", rejectAll);
  child.on("close", rejectAll);
  child.stdin.on("error", () => {});
  child.stdout.on("data", chunk => {
    buffer += chunk;
    if (buffer.length > 8192) { child.stdin.end(); rejectAll(); return; }
    let newline;
    while ((newline = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, newline); buffer = buffer.slice(newline + 1);
      try {
        const message = JSON.parse(line);
        if (!["ready", "cut", "restore"].includes(message.phase)) throw new Error();
        messages.set(message.phase, message);
        waiters.get(message.phase)?.resolve(message);
        waiters.delete(message.phase);
      } catch { child.stdin.end(); rejectAll(); }
    }
  });
  function wait(phase, remaining) {
    if (messages.has(phase)) return Promise.resolve(messages.get(phase));
    if (failure) return Promise.reject(failure);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { waiters.delete(phase); child.stdin.end(); reject(new Error("turn_watchdog_deadline")); }, remaining());
      waiters.set(phase, { resolve: value => { clearTimeout(timer); resolve(value); },
        reject: error => { clearTimeout(timer); reject(error); } });
    });
  }
  child.stdin.write(`${JSON.stringify({ target })}\n`);
  return {
    ready: remaining => wait("ready", remaining),
    async cut(remaining) {
      child.stdin.write('"cut"\n');
      const message = await wait("cut", remaining);
      requireProof(message.cut === true, "turn_stop_unconfirmed");
      return { cut: true, stopped_at_ms: message.stopped_at_ms };
    },
    async restore(remaining) {
      child.stdin.end('"restore"\n');
      const message = await wait("restore", remaining);
      let timer;
      try {
        const exit = await Promise.race([closed, new Promise((_, reject) => {
          timer = setTimeout(() => reject(new Error("turn_watchdog_exit_deadline")), remaining());
        })]);
        requireProof(exit.code === 0 && !exit.signal, "turn_watchdog_exit_failed");
      } finally { clearTimeout(timer); }
      return { restored: message.restored === true, resume_reason: message.resume_reason,
        stopped_ms: message.stopped_ms, process_identity_matched: message.process_identity_matched === true,
        resume_attempts: message.resume_attempts, resume_error: message.resume_error,
        resume_errno: message.resume_errno, manual_recovery_required: message.manual_recovery_required === true,
        watchdog_reaped: true };
    },
  };
}

// The watchdog owns both signals. stdin EOF survives the controller's SIGKILL;
// its own wall-clock timer also covers a live but stalled controller.
function watchdogController(target, io) {
  let stoppedAt, finished = false, timer, retryTimer, resumeReason, attempts = 0, lastErrno = null;
  const matches = value => identity(value) === identity(target);
  const emit = value => io.emit(value);
  const remaining = () => MAX_STOP_MS - (io.now() - stoppedAt);
  const inspect = () => io.inspect(target.pid, stoppedAt === undefined ? 200 : Math.max(1, Math.floor(Math.min(200, remaining()))));
  function complete(restored, matched, error = null) {
    if (finished) return;
    finished = true;
    io.clearTimeout(timer);
    io.clearTimeout(retryTimer);
    emit({ phase: "restore", restored, process_identity_matched: matched,
      resume_reason: resumeReason, stopped_ms: stoppedAt === undefined ? 0 : Math.max(0, io.now() - stoppedAt),
      resume_attempts: attempts, resume_error: error, resume_errno: lastErrno,
      manual_recovery_required: !restored });
    io.finish();
  }
  function attemptResume() {
    retryTimer = undefined;
    if (finished) return;
    if (stoppedAt === undefined) { complete(true, false); return; }
    if (remaining() <= 0) { complete(false, false, "resume_deadline"); return; }
    attempts += 1;
    try {
      const current = inspect();
      if (!matches(current)) { complete(false, false, "identity_changed"); return; }
      if (!/[TZ]/.test(current.state)) { complete(true, true); return; }
      requireProof(remaining() > 0, "turn_resume_deadline");
      io.signal(target.pid, "SIGCONT");
      const resumed = inspect();
      if (!matches(resumed)) { complete(false, false, "identity_changed"); return; }
      if (!/[TZ]/.test(resumed.state)) { complete(true, true); return; }
    } catch (error) {
      lastErrno = safeErrno(error);
    }
    // Keep the resume obligation and watchdog alive until confirmed or until
    // the hard deadline. A transient ps/kill failure is not terminal cleanup.
    if (remaining() <= 0) complete(false, false, "resume_deadline");
    else retryTimer = io.setTimeout(attemptResume, Math.min(50, remaining()));
  }
  function restore(reason) {
    if (finished) return;
    resumeReason ||= reason;
    if (retryTimer === undefined) attemptResume();
  }
  return {
    cut() {
      if (finished || stoppedAt !== undefined) return;
      try {
        const current = inspect();
        requireProof(matches(current) && !/[TZ]/.test(current.state), "turn_identity_changed");
        timer = io.setTimeout(() => restore("watchdog_deadline"), WATCHDOG_MS);
        stoppedAt = io.now(); // Every subsequent error retains a resume obligation.
        io.signal(target.pid, "SIGSTOP");
        const stopped = inspect();
        requireProof(matches(stopped) && stopped.state.includes("T"), "turn_stop_unconfirmed");
        emit({ phase: "cut", cut: true, stopped_at_ms: stoppedAt });
      } catch { emit({ phase: "cut", cut: false }); restore("cut_failed"); }
    },
    restore,
  };
}

function watchdogMain() {
  let controller, buffer = "";
  const initTimer = setTimeout(() => process.exit(1), 5000);
  process.stdout.on("error", () => {});
  function finish() { process.stdin.destroy(); process.stdout.end(() => process.exit(0)); }
  const emit = message => { if (!process.stdout.destroyed) process.stdout.write(`${JSON.stringify(message)}\n`); };
  const restore = reason => controller ? controller.restore(reason) : finish();
  process.on("SIGTERM", () => restore("watchdog_signal"));
  process.on("SIGINT", () => restore("watchdog_signal"));
  process.stdin.on("end", () => restore("controller_eof"));
  process.stdin.on("error", () => restore("controller_eof"));
  process.stdin.on("data", chunk => {
    buffer += chunk;
    if (buffer.length > 8192) { restore("invalid_command"); return; }
    let newline;
    while ((newline = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, newline); buffer = buffer.slice(newline + 1);
      try {
        const value = JSON.parse(line);
        if (!controller) {
          requireProof(value.target?.pid > 1 && value.target.uid === process.getuid() &&
            typeof value.target.start === "string" && typeof value.target.command === "string", "invalid_target");
          clearTimeout(initTimer);
          controller = watchdogController(value.target, {
            now: () => performance.now(), setTimeout, clearTimeout, emit, finish,
            inspect: (pid, timeout) => parseProcess(execFileSync("/bin/ps", psArgs(pid), { encoding: "utf8", timeout,
              maxBuffer: 16384, stdio: ["ignore", "pipe", "ignore"], env: { ...process.env, LC_ALL: "C" } })),
            signal: (pid, signal) => process.kill(pid, signal),
          });
          emit({ phase: "ready" });
        } else if (value === "cut") controller.cut();
        else if (value === "restore") controller.restore("requested");
        else restore("invalid_command");
      } catch { restore("invalid_command"); }
    }
  });
}

export const __test = { bind, createDriver, parseProcess, lsofRows, watchdogController, startWatchdog, identity };
if (process.argv[1] === modulePath && process.argv[2] === "--turn-interruption-watchdog") watchdogMain();
