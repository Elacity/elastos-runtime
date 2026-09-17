import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";
import { test } from "node:test";
import { __test } from "./browser-journey-turn-interruption.mjs";

const home = "/fixture/task-home";
const data = `${home}/Library/Application Support/elastos`;
const pageId = `page:vz-${"c".repeat(64)}`;
const binding = `sha256:${"a".repeat(64)}`;
const generation = `sha256:${"d".repeat(64)}`;
const ownerDir = `/tmp/evzrc/${"a".repeat(32)}`;
const journalPath = `${home}/run/browser.sock.launch-reconciliations.json`;
const configPath = `/tmp/evzs/vz-${"a".repeat(32)}/turnserver.conf`;
const nativeExe = `${data}/bin/browser-vz-engine-supervisor`;
const turnExe = "/fixture/bin/turnserver";
const options = { testHome: home, pageId, runtimeOrigin: "http://localhost:61510" };
const processRow = (pid, ppid, command) => ({ pid, ppid, uid: 501,
  start: "Tue Sep 8 05:00:00 2026", state: "S", command });
const format = row => `${row.pid} ${row.ppid} ${row.uid} ${row.start} ${row.state} ${row.command}`;

function fixture() {
  const receipt = { schema: "elastos.mac-source-home-restart/v1", ok: true, dry_run: false,
    test_home: home, data_dir: data, home_url: `${options.runtimeOrigin}/apps/home/` };
  const service = { schema: "elastos.browser.vm-control-service.identity/v1",
    service_id: `service:${"e".repeat(64)}`, control_socket_path: `${home}/run/browser.sock`, config_fingerprint: "f".repeat(64) };
  const status = { schema: "elastos.browser.vm-control-service.status/v1", ok: true,
    active_pages: 1, pending_launches: 0, page_ids: [pageId], pid: 101,
    network_mode: "runtime_net_only", direct_network: false, control_service: structuredClone(service) };
  const owner = { schema: "elastos.browser.vz-socket-owner/v1", binding_hash: binding, page_id: pageId,
    generation, vm_id: "vm:fixture", stream_id: "stream:egress", media_stream_id: "stream:media" };
  const authority = { schema: "elastos.browser.vz-transport-authority/v1", binding_hash: binding,
    generation, page_id: pageId, vm_id: owner.vm_id, egress: { stream_id: owner.stream_id },
    media: { stream_id: owner.media_stream_id, target: "tcp://127.0.0.1:49000" },
    turn: { schema: "elastos.browser.vz-turn-authority/v1", listen_host: "127.0.0.1", listen_port: 49000,
      protocols: ["turn", "tcp"] } };
  const record = { schema: "elastos.browser.vm-control-service.launch-reconciliation/v1", state: "effect_acquired",
    effects: { page_acquired: true, vm_acquired: true }, control_service: structuredClone(service),
    launch: { page_id: pageId, vm_id: owner.vm_id, lifecycle_generation: generation,
      stream_id: owner.stream_id, transport_authority: authority },
    cleanup_binding: { schema: "elastos.browser.engine-cleanup-binding/v2", page_id: pageId, generation,
      stream_id: owner.stream_id, control_service: structuredClone(service), shutdown_socket_path: `${home}/run/browser.sock`,
      control_socket_path: `${ownerDir}/c.sock`, transport_authority: structuredClone(authority),
      process: { schema: "elastos.browser.host-process-binding/v1", pid: 102,
        ownership_id: `process:${"b".repeat(64)}`, stream_bridge_pid: null } } };
  const journal = { schema: "elastos.browser.vm-control-service.launch-reconciliations/v1", records: [record] };
  const processes = new Map([
    [101, processRow(101, 90, "/fixture/bin/node /fixture/control-service.mjs")],
    [102, processRow(102, 101, nativeExe)],
    [103, processRow(103, 102, `${turnExe} -c ${configPath}`)],
  ]);
  const events = [];
  const state = { receipt, status, owner, journal, record, processes, events, reads: [],
    controlPids: [101], nativePids: [102],
    tcp: "p103\nf10\nn127.0.0.1:49000\nTST=LISTEN\nf11\nn127.0.0.1:49000->127.0.0.1:50000\nTST=ESTABLISHED\n" };
  const io = {
    platform: "darwin", uid: 501, home: "/fixture/personal-home",
    realpath: async path => path,
    ownedFile: async (path, max) => {
      state.reads.push(path);
      if (path.endsWith("mac-source-home-restart.json")) return JSON.stringify(state.receipt);
      if (path === journalPath) { assert.equal(max, 4 * 1024 * 1024); return JSON.stringify(state.journal); }
      if (path === `${ownerDir}/owner.json`) return JSON.stringify(state.owner);
      throw Object.assign(new Error(`deleted private config ${path} NEVER-PRINT-THIS-SECRET`), { code: "ENOENT" });
    },
    socket: async () => {},
    status: async () => state.status,
    socketPids: async path => path.endsWith("browser.sock") ? state.controlPids : state.nativePids,
    process: async pid => { assert.ok(processes.has(pid)); return processes.get(pid); },
    command: async (program, args) => {
      if (program === "/bin/ps") return [...processes.values()].map(format).join("\n");
      assert.equal(program, "/usr/sbin/lsof");
      if (args.includes("-iTCP")) return state.tcp;
      const pid = Number(args[args.indexOf("-p") + 1]);
      return `p${pid}\nn${pid === 102 ? nativeExe : turnExe}\n`;
    },
    watchdog: target => {
      events.push(["watchdog", target.pid]);
      return {
        ready: async () => { events.push(["ready"]); },
        cut: async () => { events.push(["cut"]); return { cut: true, stopped_at_ms: 100 }; },
        restore: async () => { events.push(["restore"]); return { restored: true, stopped_ms: 5000,
          resume_reason: "requested", process_identity_matched: true }; },
      };
    },
  };
  return { state, io };
}

const adapterSocket = "/tmp/elastos-browser-fixture-vm-control.sock";
const adapterJournal = `${adapterSocket}.launch-reconciliations.json`;
const adapterDoc = {
  adapters: [{
    kind: "chromium_microvm",
    supervisor: {
      control_socket_path: adapterSocket,
      env: { ELASTOS_BROWSER_VM_DATA_DIR: data, ELASTOS_BROWSER_VM_CONTROL_SOCKET: adapterSocket },
    },
  }],
};

function adapterFixture() {
  const { state, io } = fixture();
  state.adapter = adapterDoc;
  state.status.control_service.control_socket_path = adapterSocket;
  state.record.control_service.control_socket_path = adapterSocket;
  state.record.cleanup_binding.control_service.control_socket_path = adapterSocket;
  state.record.cleanup_binding.shutdown_socket_path = adapterSocket;
  const originalOwned = io.ownedFile;
  const originalPids = io.socketPids;
  io.ownedFile = async (path, max) => {
    if (path === `${data}/config/browser-engine-adapter.json`) {
      state.reads.push(path);
      return JSON.stringify(state.adapter);
    }
    if (path === adapterJournal) {
      state.reads.push(path);
      assert.equal(max, 4 * 1024 * 1024);
      return JSON.stringify(state.journal);
    }
    return originalOwned(path, max);
  };
  io.socketPids = async path => path === adapterSocket ? state.controlPids : originalPids(path);
  return { state, io };
}

test("adapter identity binds the live control socket without a matching restart receipt", async () => {
  const { state, io } = adapterFixture();
  state.receipt.home_url = "http://localhost:61510/home/";
  state.receipt.ok = false;
  const driver = await __test.createDriver({ ...options, controlSocketPath: adapterSocket }, io);
  assert.equal(driver.evidence.bound, true);
  assert.ok(state.reads.includes(`${data}/config/browser-engine-adapter.json`));
  assert.ok(state.reads.includes(adapterJournal));
  assert.ok(!state.reads.includes(`${data}/receipts/mac-source-home-restart.json`));
  await driver.cut({ timeoutMs: 1000 });
  await driver.restore({ timeoutMs: 1000 });
  assert.equal(driver.evidence.restored, true);
  const evidence = JSON.stringify(driver.evidence);
  assert.ok(!evidence.includes(adapterSocket));
  assert.ok(!evidence.includes(home));
});

test("adapter identity rejects a control socket that the adapter does not own", async () => {
  const { io } = adapterFixture();
  await assert.rejects(__test.createDriver({ ...options, controlSocketPath: "/tmp/foreign.sock" }, io), error => {
    assert.equal(error.message, "turn_control_socket_mismatch");
    assert.ok(!JSON.stringify(error).includes(adapterSocket));
    return true;
  });
});

test("adapter identity filesystem failure exposes only the fixed stage", async () => {
  const { io } = adapterFixture();
  const original = io.ownedFile;
  io.ownedFile = async (path, max) => {
    if (path.endsWith("browser-engine-adapter.json")) {
      throw Object.assign(new Error("ENOENT private adapter NEVER-PRINT-THIS-SECRET"), { code: "ENOENT" });
    }
    return original(path, max);
  };
  await assert.rejects(__test.createDriver({ ...options, controlSocketPath: adapterSocket }, io), error => {
    assert.equal(error.message, "turn_binding_adapter_config_failed");
    assert.deepEqual(error.evidence.failure, {
      stage: "adapter_config", code: "turn_binding_adapter_config_failed", errno: "ENOENT",
    });
    assert.ok(!JSON.stringify(error).includes("NEVER-PRINT"));
    return true;
  });
});

test("creation only binds; cut revalidates and starts watchdog; restore is idempotent and redacted", async () => {
  const { state, io } = fixture();
  const driver = await __test.createDriver(options, io);
  assert.equal(driver.evidence.bound, true);
  assert.deepEqual(state.reads, [`${data}/receipts/mac-source-home-restart.json`, journalPath, `${ownerDir}/owner.json`]);
  assert.ok(!state.reads.includes(configPath), "native deletes its secret configuration after readiness");
  assert.deepEqual(state.events, []);
  await driver.cut({ timeoutMs: 1000 });
  assert.deepEqual(state.events, [["watchdog", 103], ["ready"], ["cut"]]);
  await driver.restore({ timeoutMs: 1000 });
  await driver.restore({ timeoutMs: 1000 });
  assert.equal(state.events.filter(([kind]) => kind === "restore").length, 1);
  assert.equal(driver.evidence.restored, true);
  const evidence = JSON.stringify(driver.evidence);
  for (const privateValue of [home, pageId, binding, configPath, "NEVER-PRINT", "05:00:00", "49000", '"pid"']) {
    assert.ok(!evidence.includes(privateValue), privateValue);
  }
  assert.ok(evidence.length < 4096);
  await assert.rejects(driver.cut(), /already_attempted/);
});

const invalidFixtures = {
  "personal Home": (s, io) => { io.home = home; },
  "different Runtime origin": s => { s.receipt.home_url = "http://localhost:8090/apps/home/"; },
  "foreign data root": s => { s.receipt.data_dir = "/fixture/foreign"; },
  "unsuccessful restart": s => { s.receipt.ok = false; },
  "dry-run restart": s => { s.receipt.dry_run = true; },
  "redirected data root": (s, io) => { io.realpath = async path => path === data ? "/foreign" : path; },
  "different active page": s => { s.status.page_ids = ["vz-other"]; },
  "multiple pages": s => { s.status.active_pages = 2; },
  "pending launch": s => { s.status.pending_launches = 1; },
  "foreign control socket process": s => { s.controlPids = [999]; },
  "absent owner": s => { s.owner.page_id = "vz-other"; },
  "owner generation mismatch": s => { s.owner.generation = `sha256:${"0".repeat(64)}`; },
  "owner VM mismatch": s => { s.owner.vm_id = "vm:foreign"; },
  "owner stream mismatch": s => { s.owner.media_stream_id = "stream:foreign"; },
  "wrong binding directory": s => { s.owner.binding_hash = `sha256:${"b".repeat(64)}`; },
  "ambiguous native socket": s => { s.nativePids.push(999); },
  "foreign native executable": s => { s.processes.get(102).command = "/foreign/browser-vz-engine-supervisor --request-stdin"; },
  "foreign ancestry": s => { s.processes.get(102).ppid = 1; },
  "foreign TURN parent": s => { s.processes.get(103).ppid = 999; },
  "foreign TURN UID": s => { s.processes.get(103).uid = 502; },
  "already stopped TURN": s => { s.processes.get(103).state = "T"; },
  "ambiguous TURN process": s => { s.processes.set(104, { ...s.processes.get(103), pid: 104 }); },
  "extra TURN argv": s => { s.processes.get(103).command += " --daemon"; },
  "journal schema": s => { s.journal.schema = "foreign"; },
  "oversized record set": s => { s.journal.records = Array(129).fill(s.record); },
  "absent active record": s => { s.journal.records = []; },
  "ambiguous active record": s => { s.journal.records.push(structuredClone(s.record)); },
  "stale terminal record": s => { s.record.state = "terminal_post_effect_cleanup"; },
  "pending cleanup record": s => { s.record.state = "cleanup_pending"; },
  "conflicting cleanup obligation": s => { s.journal.records.push({ ...s.record, state: "cleanup_pending" }); },
  "record schema": s => { s.record.schema = "foreign"; },
  "page effect absent": s => { s.record.effects.page_acquired = false; },
  "VM effect unknown": s => { s.record.effects.vm_acquired = null; },
  "authority schema": s => { s.record.launch.transport_authority.schema = "foreign"; },
  "authority generation mismatch": s => { s.record.launch.lifecycle_generation = "foreign"; },
  "authority VM mismatch": s => { s.record.launch.vm_id = "vm:foreign"; },
  "cleanup authority mismatch": s => { s.record.cleanup_binding.transport_authority.turn.listen_port++; },
  "stale control service": s => { s.status.control_service.service_id = `service:${"0".repeat(64)}`; },
  "different cleanup service": s => { s.record.cleanup_binding.control_service.config_fingerprint = null; },
  "foreign service socket": s => { s.record.control_service.control_socket_path = "/foreign.sock"; },
  "foreign shutdown socket": s => { s.record.cleanup_binding.shutdown_socket_path = "/foreign.sock"; },
  "foreign native socket": s => { s.record.cleanup_binding.control_socket_path = "/foreign.sock"; },
  "process schema": s => { s.record.cleanup_binding.process.schema = "foreign"; },
  "process ownership": s => { s.record.cleanup_binding.process.ownership_id = "foreign"; },
  "foreign native PID": s => { s.record.cleanup_binding.process.pid = 999; },
  "transport UDP selection": s => {
    s.record.launch.transport_authority.turn.protocols = ["turn", "udp"];
    s.record.cleanup_binding.transport_authority = structuredClone(s.record.launch.transport_authority);
  },
  "nonloopback listener authority": s => {
    s.record.launch.transport_authority.turn.listen_host = "192.0.2.1";
    s.record.cleanup_binding.transport_authority = structuredClone(s.record.launch.transport_authority);
  },
  "foreign media target": s => {
    s.record.launch.transport_authority.media.target = "tcp://127.0.0.1:49001";
    s.record.cleanup_binding.transport_authority = structuredClone(s.record.launch.transport_authority);
  },
  "wrong TCP listener": s => { s.tcp = s.tcp.replace("49000\nTST=LISTEN", "49001\nTST=LISTEN"); },
  "no established TCP connection": s => { s.tcp = s.tcp.replace("ESTABLISHED", "SYN_SENT"); },
};
for (const [name, mutate] of Object.entries(invalidFixtures)) {
  test(`binding rejects ${name} before watchdog creation`, async () => {
    const { state, io } = fixture(); mutate(state, io);
    await assert.rejects(__test.createDriver(options, io), error => {
      assert.match(error.message, /^turn_[a-z_]+$/);
      assert.ok(!JSON.stringify(error).includes("NEVER-PRINT")); return true;
    });
    assert.deepEqual(state.events, []);
  });
}

test("cut rejects changed birth identity after read-only binding", async () => {
  const { state, io } = fixture();
  const driver = await __test.createDriver(options, io);
  state.processes.get(103).start = "Tue Sep 8 05:01:00 2026";
  await assert.rejects(driver.cut(), /turn_cut_failed/);
  await driver.restore();
  assert.deepEqual(state.events, []);
});

for (const value of ["vz-fixture-page", `vz-${"c".repeat(64)}`, `page:vz-${"C".repeat(64)}`, `page:vz-${"c".repeat(63)}`]) {
  test(`rejects noncanonical page ID ${value.slice(0, 16)}`, async () => {
    const { io } = fixture();
    await assert.rejects(__test.createDriver({ ...options, pageId: value }, io), /turn_explicit_fixture_required/);
  });
}

test("native supervisor may have arguments as well as its normal stdin-only invocation", async () => {
  const { state, io } = fixture(); state.processes.get(102).command += " --fixture-argument";
  assert.equal((await __test.createDriver(options, io)).evidence.bound, true);
});

test("aborted cut never creates a watchdog", async () => {
  const { state, io } = fixture();
  const driver = await __test.createDriver(options, io);
  await assert.rejects(driver.cut({ signal: AbortSignal.abort() }), /turn_cut_failed/);
  await driver.restore(); assert.deepEqual(state.events, []);
});

for (const [operation, input, stage] of [
  ["realpath", home, "home_path"],
  ["realpath", data, "data_dir"],
  ["ownedFile", `${data}/receipts/mac-source-home-restart.json`, "restart_receipt"],
  ["socket", `${home}/run/browser.sock`, "control_socket"],
  ["socketPids", `${home}/run/browser.sock`, "control_socket_process"],
  ["ownedFile", journalPath, "launch_journal"],
  ["ownedFile", `${ownerDir}/owner.json`, "owner_binding"],
  ["socketPids", `${ownerDir}/c.sock`, "native_socket_process"],
  ["realpath", nativeExe, "native_executable"],
  ["realpath", turnExe, "turn_executable"],
]) test(`raw filesystem failure at ${stage} exposes only fixed stage and safe errno`, async () => {
  const { state, io } = fixture(), original = io[operation];
  io[operation] = async (...args) => {
    if (args[0] === input) throw Object.assign(new Error(`ENOENT private ${input} NEVER-PRINT-THIS-SECRET`),
      { code: "ENOENT", path: input, stdout: "private output", stderr: "private stderr" });
    return original(...args);
  };
  await assert.rejects(__test.createDriver(options, io), error => {
    assert.equal(error.message, `turn_binding_${stage}_failed`);
    assert.deepEqual(error.evidence.failure, { stage, code: error.message, errno: "ENOENT" });
    assert.equal(error.cause, undefined);
    for (const secret of [home, configPath, "NEVER-PRINT", "private output", "private stderr"]) {
      assert.ok(!JSON.stringify(error).includes(secret));
    }
    return true;
  });
  assert.deepEqual(state.events, []);
});

test("malformed receipt JSON identifies its fixed stage without echoing content", async () => {
  const { io } = fixture(); io.ownedFile = async () => "NEVER-PRINT-THIS-SECRET";
  await assert.rejects(__test.createDriver(options, io), error => {
    assert.deepEqual(error.evidence.failure, { stage: "restart_receipt", code: "turn_binding_restart_receipt_failed", errno: null });
    assert.ok(!JSON.stringify(error).includes("NEVER-PRINT")); return true;
  });
});

test("arbitrary error code and turn-prefixed message cannot enter public evidence", async () => {
  const { io } = fixture(); io.realpath = async () => {
    throw Object.assign(new Error("turn_private_credential"), { code: "PRIVATE_CREDENTIAL" });
  };
  await assert.rejects(__test.createDriver(options, io), error => {
    assert.deepEqual(error.evidence.failure, { stage: "home_path", code: "turn_binding_home_path_failed", errno: null });
    return true;
  });
});

test("cut revalidation retains the filesystem stage and errno and starts no watchdog", async () => {
  const { state, io } = fixture(), driver = await __test.createDriver(options, io);
  io.socketPids = async () => { throw Object.assign(new Error("private socket path"), { code: "EACCES" }); };
  await assert.rejects(driver.cut(), error => {
    assert.equal(error.message, "turn_cut_failed");
    assert.deepEqual(error.evidence.failure, { stage: "control_socket_process", code: "turn_binding_control_socket_process_failed", errno: "EACCES" });
    return true;
  });
  await driver.restore(); assert.deepEqual(state.events, []);
});

for (const stage of ["ready", "cut"]) test(`restore remains available when watchdog ${stage} rejects`, async () => {
  const { state, io } = fixture(), original = io.watchdog;
  io.watchdog = target => ({ ...original(target), [stage]: async () => { throw new Error("private path / secret"); } });
  const driver = await __test.createDriver(options, io);
  await assert.rejects(driver.cut(), /turn_cut_failed/);
  await driver.restore();
  assert.equal(driver.evidence.restored, true);
  assert.equal(state.events.at(-1)[0], "restore");
});

function watchdogFixture() {
  const target = processRow(103, 102, `${turnExe} -c ${configPath}`), signals = [], messages = [];
  const current = { ...target }, timers = new Map();
  let time = 1000, nextTimer = 0, finished = false;
  const io = {
    inspect: () => current, signal: (pid, signal) => {
      signals.push([pid, signal]); current.state = signal === "SIGSTOP" ? "T" : "S";
    },
    now: () => time, emit: message => messages.push(message), finish: () => { finished = true; },
    setTimeout: (callback, ms) => { const id = ++nextTimer; timers.set(id, { callback, at: time + ms, ms }); return id; },
    clearTimeout: id => timers.delete(id),
  };
  const controller = __test.watchdogController(target, io);
  return { controller, io, signals, messages, current, target, timer: () => [...timers.values()][0],
    advance: ms => {
      const end = time + ms;
      for (let count = 0; count < 300; count++) {
        const next = [...timers.entries()].sort((a, b) => a[1].at - b[1].at)[0];
        if (!next || next[1].at > end) { time = end; return; }
        timers.delete(next[0]); time = next[1].at; next[1].callback();
      }
      assert.fail("watchdog retry loop exceeded bound");
    }, finished: () => finished };
}

for (const reason of ["requested", "controller_eof", "watchdog_signal", "watchdog_deadline"]) {
  test(`watchdog resumes exact process on ${reason}`, () => {
    const f = watchdogFixture(); f.controller.cut();
    assert.equal(f.timer().ms, 7000);
    f.advance(reason === "watchdog_deadline" ? 7000 : 5000);
    if (reason !== "watchdog_deadline") f.controller.restore(reason);
    f.controller.restore(reason);
    assert.deepEqual(f.signals, [[103, "SIGSTOP"], [103, "SIGCONT"]]);
    assert.equal(f.messages.at(-1).restored, true); assert.equal(f.finished(), true);
  });
}

test("watchdog rejects recycled PID before STOP", () => {
  const f = watchdogFixture(); f.current.start += " changed"; f.controller.cut();
  assert.deepEqual(f.signals, []); assert.equal(f.messages[0].cut, false);
});
test("watchdog rejects recycled PID before CONT", () => {
  const f = watchdogFixture(); f.controller.cut(); f.current.start += " changed";
  f.controller.restore("requested");
  assert.deepEqual(f.signals, [[103, "SIGSTOP"]]); assert.equal(f.messages.at(-1).restored, false);
});
test("watchdog resumes the same process after parent death reparents it", () => {
  const f = watchdogFixture(); f.controller.cut(); f.current.ppid = 1;
  f.controller.restore("controller_eof"); assert.equal(f.messages.at(-1).restored, true);
});
test("watchdog leaves an independently stopped process alone", () => {
  const f = watchdogFixture(); f.current.state = "T"; f.controller.cut(); assert.deepEqual(f.signals, []);
});

for (const operation of ["inspect", "signal", "post_signal_inspect"]) {
  test(`watchdog retries transient ${operation} failure while retaining resume obligation`, () => {
    const f = watchdogFixture(); f.controller.cut(); f.advance(5000);
    const key = operation === "signal" ? "signal" : "inspect", original = f.io[key];
    let calls = 0;
    f.io[key] = (...args) => {
      if (++calls === (operation === "post_signal_inspect" ? 2 : 1)) {
        throw Object.assign(new Error("private path NEVER-PRINT"), { code: "EACCES" });
      }
      return original(...args);
    };
    f.controller.restore("requested");
    assert.equal(f.finished(), false);
    assert.equal(f.messages.some(message => message.phase === "restore"), false);
    f.advance(50);
    assert.equal(f.finished(), true);
    assert.equal(f.messages.at(-1).restored, true);
    assert.equal(f.messages.at(-1).resume_attempts, 2);
    assert.equal(f.messages.at(-1).stopped_ms, 5050);
    assert.equal(f.messages.at(-1).manual_recovery_required, false);
    assert.deepEqual(f.signals, [[103, "SIGSTOP"], [103, "SIGCONT"]]);
    assert.ok(!JSON.stringify(f.messages).includes("NEVER-PRINT"));
  });
}

for (const operation of ["inspect", "signal"]) {
  test(`persistent ${operation} failure reaches explicit eight-second resume deadline`, () => {
    const f = watchdogFixture(); f.controller.cut();
    f.io[operation] = () => { throw Object.assign(new Error("private details"), { code: "EPERM" }); };
    f.advance(7999);
    assert.equal(f.finished(), false);
    assert.equal(f.messages.some(message => message.phase === "restore"), false);
    f.advance(1);
    assert.equal(f.finished(), true);
    assert.equal(f.messages.at(-1).restored, false);
    assert.equal(f.messages.at(-1).resume_error, "resume_deadline");
    assert.equal(f.messages.at(-1).resume_errno, "EPERM");
    assert.equal(f.messages.at(-1).manual_recovery_required, true);
    assert.equal(f.messages.at(-1).stopped_ms, 8000);
    assert.deepEqual(f.signals, [[103, "SIGSTOP"]]);
  });
}

test("PID change during resume retry refuses CONT and reports manual recovery", () => {
  const f = watchdogFixture(); f.controller.cut();
  const inspect = f.io.inspect;
  f.io.inspect = () => { throw new Error("transient ps error"); };
  f.controller.restore("controller_eof");
  assert.equal(f.finished(), false);
  f.io.inspect = inspect; f.current.start += " changed";
  f.advance(50);
  assert.equal(f.messages.at(-1).resume_error, "identity_changed");
  assert.equal(f.messages.at(-1).manual_recovery_required, true);
  assert.deepEqual(f.signals, [[103, "SIGSTOP"]]);
});

test("STOP acknowledgement requires T and retains the resume obligation on inspection error", () => {
  const f = watchdogFixture(), inspect = f.io.inspect;
  let calls = 0;
  f.io.inspect = () => { if (++calls === 2) throw new Error("transient ps error"); return inspect(); };
  f.controller.cut();
  assert.equal(f.messages[0].cut, false);
  assert.equal(f.messages.at(-1).restored, true);
  assert.deepEqual(f.signals, [[103, "SIGSTOP"], [103, "SIGCONT"]]);
});

test("STOP acknowledgement rejects a matching identity that never enters T", () => {
  const f = watchdogFixture(); f.io.signal = (pid, signal) => f.signals.push([pid, signal]);
  f.controller.cut();
  assert.equal(f.messages[0].cut, false);
  assert.equal(f.messages.at(-1).restored, true);
  assert.deepEqual(f.signals, [[103, "SIGSTOP"]]);
});

test("driver exposes unconfirmed resume and reaped watchdog as requiring parent recovery", async () => {
  const { io } = fixture(), original = io.watchdog;
  io.watchdog = target => ({ ...original(target), restore: async () => ({ restored: false,
    stopped_ms: 8000, resume_error: "resume_deadline", resume_errno: "EPERM", watchdog_reaped: true,
    manual_recovery_required: true }) });
  const driver = await __test.createDriver(options, io);
  await driver.cut();
  await assert.rejects(driver.restore(), error => {
    assert.equal(error.message, "turn_restore_failed");
    assert.equal(error.evidence.manual_recovery_required, true);
    assert.equal(error.evidence.watchdog_reaped, true);
    assert.equal(error.evidence.resume_error, "resume_deadline");
    return true;
  });
});

test("real watchdog stops and resumes only a disposable Node child", { timeout: 8000 }, async t => {
  const dummy = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], { stdio: "ignore" });
  await once(dummy, "spawn");
  t.after(() => { try { dummy.kill("SIGCONT"); dummy.kill("SIGTERM"); } catch {} });
  const inspect = () => __test.parseProcess(execFileSync("/bin/ps", ["-ww", "-p", String(dummy.pid),
    "-o", "pid=", "-o", "ppid=", "-o", "uid=", "-o", "lstart=", "-o", "stat=", "-o", "command="],
  { encoding: "utf8", env: { ...process.env, LC_ALL: "C" } }));
  const original = inspect(), watchdog = __test.startWatchdog(original);
  try {
    await watchdog.ready(() => 1500);
    await watchdog.cut(() => 1500);
    assert.match(inspect().state, /T/);
  } finally {
    const receipt = await watchdog.restore(() => 1500);
    assert.equal(receipt.restored, true);
    assert.equal(receipt.watchdog_reaped, true);
  }
  assert.equal(__test.identity(inspect()), __test.identity(original));
  assert.ok(!inspect().state.includes("T"));
  const exited = once(dummy, "exit"); dummy.kill("SIGTERM"); await exited;
});
