import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import vm from "node:vm";
import test from "node:test";

const source = fs.readFileSync(new URL("./browser-vm-local-crosvm-launcher.mjs", import.meta.url), "utf8");

test("Linux launcher uses ordinary commands for launch and cleanup", () => {
  assert.doesNotMatch(source, /sudo/i);
  assert.doesNotMatch(source, /prepareTap|prepareFirewall|assertFirewallToolAvailable/);
});

test("Runtime Browser helpers and adapter contain no escalation command", () => {
  for (const file of ["browser-vm-linux-network.py", "browser-vm-control-service.mjs", "browser-vm-engine-supervisor.mjs",
    "browser-vm-artifact-preflight.sh", "browser-vm-engine-preflight.sh", "browser-runtime-turn.mjs",
    "../capsules/browser-engine-adapter/src/main.rs", "../capsules/browser-engine-adapter/src/supervisor.rs",
    "../elastos/crates/elastos-server/src/api/gateway_browser_engine.rs"]) {
    assert.equal(/sudo/i.test(fs.readFileSync(new URL(file, import.meta.url), "utf8")), false, file);
  }
});

function networkApi(result) {
  const calls = [];
  const context = vm.createContext({ path, os: { userInfo: () => ({ username: "elastos-agent" }) }, process: { getuid: () => 1001, env: {} },
    runSync(command, args) { calls.push([command, args]); return result; },
  });
  const start = source.indexOf("function linuxNetworkCommand(");
  assert.ok(start >= 0, "launcher must check the prepared Linux network");
  const end = source.indexOf("\nfunction listen(", start);
  return { ...vm.runInContext(`${source.slice(start, end)}\n({linuxNetworkCommand, requireLinuxNetwork})`, context), calls };
}

test("missing Linux setup refuses launch with one root command", () => {
  const api = networkApi({ status: 1, stderr: "Browser network setup is required. As root, run: python3 /usr/local/lib/elastos/browser-vm-linux-network.py setup --user elastos-agent\n" });
  assert.throws(() => api.requireLinuxNetwork("/fixture/bin/browser-vm-local-crosvm-launcher.mjs"),
    /As root, run: python3 .* setup --user elastos-agent/);
  assert.equal(api.calls.length, 1);
  assert.deepEqual(JSON.parse(JSON.stringify(api.calls[0])), ["python3", ["/fixture/scripts/browser-vm-linux-network.py", "check"]]);
});

test("a failed helper command names the root-owned repair copy", () => {
  const api = networkApi({ status: 1, stderr: "" });
  assert.throws(() => api.requireLinuxNetwork("/fixture/bin/browser-vm-local-crosvm-launcher.mjs"),
    /As root, run: python3 \/usr\/local\/lib\/elastos\/browser-vm-linux-network.py setup --user elastos-agent/);
});

test("existing Browser CI step runs the Linux network tests", () => {
  const ci = fs.readFileSync(new URL("../.github/workflows/ci.yml", import.meta.url), "utf8");
  const step = ci.slice(ci.indexOf("- name: JavaScript behavior tests"), ci.indexOf("\n  # One input for every ARM consumer"));
  assert.match(step, /python3 scripts\/browser-vm-linux-network\.test\.py/);
  assert.match(step, /node --test/);
});

test("crosvm command consumes the prepared TAP and addresses", () => {
  const calls = [];
  const child = { once() {} };
  const context = vm.createContext({ path, os: { arch: () => "arm64" }, process: { env: {} },
    fs: { openSync: () => 10, mkdirSync() {} }, hasTurnServer: () => true, iceBootConfigHex: () => "",
    profileKey: () => "profile", defaultVmVcpus: () => "4",
    spawnTracked(command, args, options) { calls.push([command, args, options]); return child; },
  });
  const start = source.indexOf("function startCrosvm(");
  const end = source.indexOf("\nfunction tailFile(", start);
  const startCrosvm = vm.runInContext(`${source.slice(start, end)}\nstartCrosvm`, context);
  startCrosvm({ crosvm: "/fixture/crosvm", kernel: "/fixture/kernel", initrd: "/fixture/initrd",
    rootfs: "/fixture/rootfs", sessionDir: "/fixture/session", profile: { profileKey: "profile", diskPath: "/fixture/profile.ext4", initialize: true, lockFd: 42 }, iceConfig: {},
    launch: { display_mode: "webrtc_remote_display" },
    network: { tapName: "ebv3e9s0", hostIp: "192.168.200.1", guestIp: "192.168.200.2", prefix: 30, mac: "02:eb:00:03:e9:00" } });
  assert.equal(calls.length, 1);
  assert.deepEqual(JSON.parse(JSON.stringify(calls[0][2].stdio)), ["ignore", 10, 10, 42]);
  assert.equal(calls[0][0], "/fixture/crosvm");
  const args = calls[0][1];
  assert.equal(args[args.indexOf("--net") + 1], "tap-name=ebv3e9s0,mac=02:eb:00:03:e9:00");
  assert.deepEqual(Array.from(args.entries()).filter(([, v]) => v === "--block").map(([i]) => args[i + 1]), ["path=/fixture/rootfs,root=true", "path=/fixture/profile.ext4"]);
  assert.match(args[args.indexOf("-p") + 1], /elastos.browser_profile_disk=required/);
  assert.match(args[args.indexOf("-p") + 1], /elastos.browser_profile_initialize=new/);
  assert.match(args[args.indexOf("-p") + 1], /elastos.browser_host_ip=192\.168\.200\.1/);
  assert.match(args[args.indexOf("-p") + 1], /elastos.browser_guest_ip=192\.168\.200\.2/);
});

test("real launcher stops before VM commands when setup is absent", async t => {
  const root = fs.mkdtempSync("/tmp/ebv165-");
  const socket = path.join(root, "relay.sock");
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  // Only the request socket type is a fixture; this sandbox blocks listeners.
  const preload = path.join(root, "socket-fixture.mjs");
  fs.writeFileSync(preload, `import fs from "node:fs"; const original = fs.statSync;
    fs.statSync = (file, ...args) => file === ${JSON.stringify(socket)} ? { isSocket: () => true } : original(file, ...args);`);
  fs.mkdirSync(path.join(root, "bin"));
  fs.mkdirSync(path.join(root, "scripts"));
  const installed = path.join(root, "bin/browser-vm-local-crosvm-launcher.mjs");
  fs.writeFileSync(installed, source);
  const helper = fs.readFileSync(new URL("./browser-vm-linux-network.py", import.meta.url), "utf8");
  fs.writeFileSync(path.join(root, "scripts/browser-vm-linux-network.py"),
    helper.replace("RUN = Path('/run/elastos-browser')", `RUN = Path(${JSON.stringify(path.join(root, "absent-setup"))})`));
  for (const file of ["kernel", "rootfs", "initrd", "crosvm"]) fs.writeFileSync(path.join(root, file), "fixture");
  const requestFile = path.join(root, "request.json");
  fs.writeFileSync(requestFile, JSON.stringify({ schema: "elastos.browser.vm-engine.open/v1", launch_request: {
    schema: "elastos.browser.engine.launch-request/v1", adapter: "fixture", stream_id: "fixture",
    engine: "chromium_microvm", display_mode: "webrtc_remote_display", network_mode: "runtime_net_only",
    direct_network: false, wallet_injection: false, relay_ipc: { kind: "unix_socket", path: socket },
  } }));
  // A regular stdin file avoids a nonblocking pipe read race in the fixture.
  const requestFd = fs.openSync(requestFile, "r");
  t.after(() => fs.closeSync(requestFd));
  const result = spawnSync(process.execPath, ["--import", preload, installed], {
    env: { ...process.env, ELASTOS_BROWSER_VM_DATA_DIR: root, ELASTOS_BROWSER_VM_ROOT: path.join(root, "sessions"),
      ELASTOS_BROWSER_VM_KERNEL: path.join(root, "kernel"), ELASTOS_BROWSER_VM_ROOTFS: path.join(root, "rootfs"),
      ELASTOS_BROWSER_VM_INITRD: path.join(root, "initrd"), ELASTOS_BROWSER_VM_CROSVM_BIN: path.join(root, "crosvm") },
    stdio: [requestFd, "pipe", "pipe"], encoding: "utf8", timeout: 5000,
  });
  assert.equal(result.status, 1, result.stderr);
  // A root-run CI uses the explicit ordinary-user refusal instead.
  assert.match(result.stderr, process.getuid() === 0 ? /Run Browser as its ordinary Home user/ : /As root, run: python3 .* setup --user /);
  assert.equal(result.stdout, "");
  assert.deepEqual(fs.readdirSync(path.join(root, "sessions")), []);
});

function profileFixture(t) {
  const root = fs.mkdtempSync("/tmp/ebv-profile-");
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.mkdirSync(path.join(root, "bin"));
  fs.symlinkSync("/usr/bin/python3", path.join(root, "bin/python3"));
  const profile = { schema: "elastos.browser.profile/v1", scope: "active_principal",
    storage: "principal_owned_profile_disk", profile_key: `profile-${"a".repeat(64)}`,
    disk_path: path.join(root, "principal/BrowserProfiles/default/profile.ext4") };
  const context = vm.createContext({ fs, path, Buffer, spawnSync,
    process: { env: { ELASTOS_BROWSER_VM_PROFILE_DISK_MIB: "128" } },
    validateAbsolutePath(file) { assert.ok(path.isAbsolute(file)); },
    requireFile(file) { assert.ok(fs.statSync(file).isFile()); },
  });
  const start = source.indexOf("function profileFailure(");
  const end = source.indexOf("\nfunction requireFile(", start);
  const prepare = vm.runInContext(`${source.slice(start, end)}\nprepareProfileDisk`, context);
  return { root, profile, context, prepare: () => prepare(profile, root) };
}

test("profile disk is sparse, private and created once; unfinished initialization stays preserved", t => {
  const f = profileFixture(t), prepared = f.prepare();
  try {
    const stat = fs.statSync(f.profile.disk_path);
    assert.equal(stat.size, 128 * 1024 * 1024);
    assert.equal(stat.mode & 0o777, 0o600);
    assert.ok(stat.blocks * 512 < stat.size);
    assert.equal(prepared.initialize, true);
    assert.throws(f.prepare, /resources_in_use/);
  } finally { fs.closeSync(prepared.lockFd); }
  assert.throws(f.prepare, /profile_recovery_required/);
  assert.equal(fs.statSync(f.profile.disk_path).size, 128 * 1024 * 1024);
});

test("existing profile bytes survive reopening and disk aliases are refused", t => {
  const f = profileFixture(t);
  fs.mkdirSync(path.dirname(f.profile.disk_path), { recursive: true });
  const bytes = Buffer.from("committed ext4 fixture");
  fs.writeFileSync(f.profile.disk_path, bytes);
  const prepared = f.prepare();
  assert.equal(prepared.initialize, false);
  fs.closeSync(prepared.lockFd);
  assert.deepEqual(fs.readFileSync(f.profile.disk_path), bytes);
  const alias = path.join(f.root, "alias");
  fs.linkSync(f.profile.disk_path, alias);
  assert.throws(f.prepare, /one regular-file identity/);
  fs.unlinkSync(alias);
  fs.renameSync(f.profile.disk_path, alias);
  fs.symlinkSync(alias, f.profile.disk_path);
  assert.throws(f.prepare);
  assert.deepEqual(fs.readFileSync(alias), bytes);
});

test("invalid profile sizing stops before disk creation", t => {
  const f = profileFixture(t);
  f.context.process.env.ELASTOS_BROWSER_VM_PROFILE_DISK_MIB = "garbage";
  assert.throws(f.prepare, /128..65536/);
  assert.equal(fs.existsSync(f.profile.disk_path), false);
});

test("kernel profile lock follows an inherited writer descriptor after launcher ownership ends", async t => {
  const f = profileFixture(t);
  fs.mkdirSync(path.dirname(f.profile.disk_path), { recursive: true });
  fs.writeFileSync(f.profile.disk_path, "committed ext4 fixture");
  const prepared = f.prepare();
  const writer = spawn(process.execPath, ["-e", "process.stdout.write('ready'); setInterval(() => {}, 1000)"],
    { stdio: ["ignore", "pipe", "pipe", prepared.lockFd] });
  t.after(() => writer.kill("SIGKILL"));
  await new Promise((resolve, reject) => { writer.once("error", reject); writer.stdout.once("data", resolve); });
  fs.closeSync(prepared.lockFd);
  assert.throws(f.prepare, /resources_in_use/);
  const exited = new Promise(resolve => writer.once("exit", resolve));
  writer.kill("SIGKILL");
  await exited;
  const reopened = f.prepare();
  assert.equal(reopened.initialize, false);
  fs.closeSync(reopened.lockFd);
});
