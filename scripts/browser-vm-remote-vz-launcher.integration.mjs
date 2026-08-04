#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const request = JSON.parse(fs.readFileSync(0, "utf8"));
const launch = request.launch_request;
const authority = launch.transport_authority;
const bindingDigest = authority.binding_hash.slice("sha256:".length, 39);
const fixtureRoot = fs.mkdtempSync(
  path.join(os.tmpdir(), "browser-vz-wrapper-integration."),
);
const localRoot = path.join(
  fixtureRoot,
  ...Array.from({ length: 18 }, (_, index) => `long-evidence-${index}`),
  "sessions",
);
const remoteRoot = path.join(
  fixtureRoot,
  ...Array.from({ length: 18 }, (_, index) => `long-remote-${index}`),
  "sessions",
);
const nonce = `${process.pid}-${Date.now().toString(36)}`;
const localSocketRoot = `/tmp/evzli-${nonce}`;
const remoteSocketRoot = `/tmp/evzri-${nonce}`;
const fakeSshPath = path.join(fixtureRoot, "fake-ssh");
const childLedgerPath = path.join(fixtureRoot, "child-ledger.jsonl");
const stdinProofPath = path.join(fixtureRoot, "stdin-proof.json");
const wrapperPath = fileURLToPath(
  new URL("./browser-vm-remote-vz-launcher.mjs", import.meta.url),
);
const expectedFailedAbsence =
  process.env.ELASTOS_BROWSER_TEST_FAIL_ABSENCE_FIELD || "";

const fakeSshSource = String.raw`#!/usr/bin/env node
const fs = require("node:fs");
const process = require("node:process");

const args = process.argv.slice(2);
const command = args.at(-1) || "";
const ledger = process.env.ELASTOS_FAKE_SSH_LEDGER;
const proof = process.env.ELASTOS_FAKE_SSH_STDIN_PROOF;
const failAbsenceField =
  process.env.ELASTOS_BROWSER_TEST_FAIL_ABSENCE_FIELD || "";
let finished = false;
let kind = "command";

function record(event) {
  fs.appendFileSync(
    ledger,
    JSON.stringify({ event, pid: process.pid, kind }) + "\n",
    { mode: 0o600 },
  );
}

function finish(code) {
  if (finished) return;
  finished = true;
  record("exit");
  process.exit(code);
}

if (args.includes("-N")) {
  kind = "forward";
  record("start");
  process.on("SIGTERM", () => finish(0));
  process.on("SIGINT", () => finish(0));
  setInterval(() => {}, 1000);
} else if (args.includes("test") && args.includes("-S")) {
  kind = "socket-probe";
  record("start");
  finish(0);
} else if (command.includes("PREFLIGHT_OK")) {
  kind = "preflight";
  record("start");
  process.stdout.write("PREFLIGHT_OK\n", () => finish(0));
} else if (command.includes("s = socket.create_connection")) {
  kind = "port-probe";
  record("start");
  finish(0);
} else if (
  command.includes("bridge_pid=") &&
  command.includes("ulimit -n")
) {
  kind = "relay";
  record("start");
  process.on("SIGTERM", () => finish(0));
  process.on("SIGINT", () => finish(0));
  process.stdout.write("READY\n");
  setInterval(() => {}, 1000);
} else if (
  command.includes("REQUEST_FILE=$(mktemp") &&
  command.includes("browser-vz-engine-supervisor")
) {
  kind = "supervisor";
  record("start");
  let input = "";
  process.stdin.setEncoding("utf8");
  process.stdin.on("data", (chunk) => {
    input += chunk;
  });
  process.stdin.on("end", () => {
    const request = JSON.parse(input);
    const launch = request.launch_request;
    const authority = launch.transport_authority;
    fs.writeFileSync(
      proof,
      JSON.stringify({
        eof: true,
        bytes: Buffer.byteLength(input),
        binding_hash: authority.binding_hash,
        generation: launch.lifecycle_generation,
        page_id: launch.page_id,
        vm_id: launch.vm_id,
        stream_id: launch.stream_id,
        media_stream_id: authority.media.stream_id,
        secret_present: Boolean(launch.transport_secret),
        request_env_absent:
          !process.env.ELASTOS_BROWSER_ENGINE_REQUEST &&
          !process.env.ELASTOS_BROWSER_VM_OPEN_REQUEST,
        binding_marker_present: command.includes(
          "--elastos-vz-binding=" + authority.binding_hash,
        ),
      }),
      { mode: 0o600 },
    );
    const settlement = {
      schema: "elastos.browser.vz-launch-settlement/v1",
      state: "terminal_post_effect_cleanup",
      message: "injected native post-effect failure",
      binding_hash: authority.binding_hash,
      generation: launch.lifecycle_generation,
      page_id: launch.page_id,
      vm_id: launch.vm_id,
      stream_id: launch.stream_id,
      media_stream_id: authority.media.stream_id,
      effects: {
        session_directory: true,
        control_socket: false,
        ordinary_stream_bridge: false,
        media_stream_bridge: false,
        turn_process: false,
        supervisor_child: false,
        vm: true,
      },
      absence: {
        child_absent: true,
        supervisor_child_absent: true,
        control_socket_absent: true,
        route_absent: true,
        turn_listener_absent: true,
        turn_relay_ports_absent: true,
        ordinary_stream_bridge_absent: true,
        media_stream_bridge_absent: true,
        session_directory_absent: true,
        vm_absent: true,
      },
    };
    process.stderr.write(JSON.stringify(settlement) + "\n", () => finish(1));
  });
} else if (
  failAbsenceField &&
  command.includes("# elastos_absence_field=" + failAbsenceField)
) {
  kind = "absence-failure";
  record("start");
  finish(42);
} else {
  record("start");
  finish(0);
}
`;

function listenUnix(socketPath) {
  fs.mkdirSync(path.dirname(socketPath), { recursive: true, mode: 0o700 });
  const server = net.createServer();
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(socketPath, () => resolve(server));
  });
}

function closeServer(server) {
  return new Promise((resolve) => server.close(resolve));
}

function parseSettlements(stderr) {
  return String(stderr)
    .split(/\r?\n/)
    .flatMap((line) => {
      const candidate = line
        .replace(/^\[remote-vz supervisor\]\s*/, "")
        .trim();
      if (!candidate.startsWith("{")) return [];
      try {
        const value = JSON.parse(candidate);
        return value.schema === "elastos.browser.vz-launch-settlement/v1"
          ? [value]
          : [];
      } catch {
        return [];
      }
    });
}

let egressServer;
let mediaServer;
try {
  assert.ok(
    Buffer.byteLength(localRoot) > 100 &&
      Buffer.byteLength(remoteRoot) > 100,
    "integration roots must exercise long evidence/session paths",
  );
  fs.writeFileSync(fakeSshPath, fakeSshSource, { mode: 0o700 });
  egressServer = await listenUnix(authority.egress.runtime_socket_path);
  mediaServer = await listenUnix(authority.media.runtime_socket_path);

  const result = spawnSync(process.execPath, [wrapperPath], {
    input: `${JSON.stringify(request)}\n`,
    encoding: "utf8",
    timeout: 30_000,
    env: {
      ...process.env,
      ELASTOS_BROWSER_REMOTE_VZ_SSH: "fake-vz-host",
      ELASTOS_BROWSER_REMOTE_VZ_SSH_BIN: fakeSshPath,
      ELASTOS_BROWSER_REMOTE_VZ_LOCAL_ROOT: localRoot,
      ELASTOS_BROWSER_REMOTE_VZ_ROOT: remoteRoot,
      ELASTOS_BROWSER_REMOTE_VZ_SOCKET_ROOT: localSocketRoot,
      ELASTOS_BROWSER_REMOTE_VZ_REMOTE_SOCKET_ROOT: remoteSocketRoot,
      ELASTOS_BROWSER_REMOTE_VZ_DATA_DIR: "/tmp/fake-vz-data",
      ELASTOS_BROWSER_REMOTE_VZ_PROFILE_ROOT: "/tmp/fake-vz-profiles",
      ELASTOS_BROWSER_REMOTE_VZ_TURN_PROGRAM: "/usr/bin/true",
      ELASTOS_BROWSER_REMOTE_VZ_LAUNCH_TIMEOUT_MS: "5000",
      ELASTOS_BROWSER_REMOTE_VZ_SOCKET_TIMEOUT_MS: "1500",
      ELASTOS_FAKE_SSH_LEDGER: childLedgerPath,
      ELASTOS_FAKE_SSH_STDIN_PROOF: stdinProofPath,
    },
  });
  assert.equal(result.error, undefined, result.error?.message);
  assert.equal(result.status, 1, result.stderr);

  const settlements = parseSettlements(result.stderr);
  const terminal = settlements.at(-1);
  assert.equal(
    terminal.state,
    expectedFailedAbsence
      ? "cleanup_pending"
      : "terminal_post_effect_cleanup",
  );
  assert.equal(terminal.binding_hash, authority.binding_hash);
  assert.equal(terminal.generation, launch.lifecycle_generation);
  for (const [field, value] of Object.entries(terminal.absence)) {
    assert.equal(
      value,
      field === expectedFailedAbsence ? false : true,
      `${field} was coupled to ${expectedFailedAbsence || "no failed field"}`,
    );
  }
  assert.equal(terminal.effects.session_directory, true, result.stderr);
  assert.equal(terminal.effects.ordinary_stream_bridge, true, result.stderr);
  assert.equal(terminal.effects.media_stream_bridge, true, result.stderr);
  assert.equal(terminal.effects.turn_process, true, result.stderr);
  assert.equal(terminal.effects.supervisor_child, true, result.stderr);
  assert.equal(terminal.effects.vm, true, result.stderr);

  const stdinProof = JSON.parse(fs.readFileSync(stdinProofPath, "utf8"));
  assert.deepEqual(
    {
      eof: stdinProof.eof,
      binding_hash: stdinProof.binding_hash,
      generation: stdinProof.generation,
      page_id: stdinProof.page_id,
      vm_id: stdinProof.vm_id,
      stream_id: stdinProof.stream_id,
      media_stream_id: stdinProof.media_stream_id,
      secret_present: stdinProof.secret_present,
      request_env_absent: stdinProof.request_env_absent,
      binding_marker_present: stdinProof.binding_marker_present,
    },
    {
      eof: true,
      binding_hash: authority.binding_hash,
      generation: launch.lifecycle_generation,
      page_id: launch.page_id,
      vm_id: launch.vm_id,
      stream_id: launch.stream_id,
      media_stream_id: authority.media.stream_id,
      secret_present: true,
      request_env_absent: true,
      binding_marker_present: true,
    },
  );

  const events = fs
    .readFileSync(childLedgerPath, "utf8")
    .trim()
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line) => JSON.parse(line));
  const started = new Set(
    events.filter(({ event }) => event === "start").map(({ pid }) => pid),
  );
  const exited = new Set(
    events.filter(({ event }) => event === "exit").map(({ pid }) => pid),
  );
  assert.ok(started.size >= 10, "fixture did not exercise the full wrapper handoff");
  assert.deepEqual(exited, started, "wrapper left a fake SSH child unreaped");

  const localBindingDir = path.join(localSocketRoot, bindingDigest);
  const localSessionDir = path.join(localRoot, `bvm-${bindingDigest}`);
  assert.equal(fs.existsSync(localBindingDir), false);
  assert.equal(fs.existsSync(localSessionDir), false);
  assert.equal(fs.existsSync(authority.egress.runtime_socket_path), true);
  assert.equal(fs.existsSync(authority.media.runtime_socket_path), true);

  process.stdout.write(
    `${JSON.stringify({
      schema: "elastos.browser.vz-wrapper-integration-proof/v1",
      terminal: !expectedFailedAbsence,
      generation: launch.lifecycle_generation,
      binding_hash: authority.binding_hash,
      long_roots: true,
      private_stdin_eof: true,
      post_effect_cleanup: true,
      zero_owned_residue: true,
      failed_absence_field: expectedFailedAbsence || null,
    })}\n`,
  );
} finally {
  if (egressServer) await closeServer(egressServer);
  if (mediaServer) await closeServer(mediaServer);
  for (const socketPath of [
    authority.egress.runtime_socket_path,
    authority.media.runtime_socket_path,
  ]) {
    try {
      fs.unlinkSync(socketPath);
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
    }
  }
  fs.rmSync(localSocketRoot, { recursive: true, force: true });
  fs.rmSync(remoteSocketRoot, { recursive: true, force: true });
  fs.rmSync(fixtureRoot, { recursive: true, force: true });
}
