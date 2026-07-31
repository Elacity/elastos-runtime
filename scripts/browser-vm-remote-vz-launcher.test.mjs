import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

process.env.ELASTOS_BROWSER_REMOTE_VZ_SOCKET_ROOT = "/tmp/evzlt";
process.env.ELASTOS_BROWSER_REMOTE_VZ_REMOTE_SOCKET_ROOT = "/tmp/evzrt";
process.env.ELASTOS_BROWSER_REMOTE_VZ_ROOT =
  `/tmp/${"remote-vz-evidence/".repeat(8)}sessions`;

const launcher = await import("./browser-vm-remote-vz-launcher.mjs");
const launcherPath = fileURLToPath(
  new URL("./browser-vm-remote-vz-launcher.mjs", import.meta.url),
);
const launcherIntegrationPath = fileURLToPath(
  new URL(
    "./browser-vm-remote-vz-launcher.integration.mjs",
    import.meta.url,
  ),
);

function canonicalJson(value) {
  if (Array.isArray(value)) return value.map(canonicalJson);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, canonicalJson(value[key])]),
    );
  }
  return value;
}

function sha256Label(value) {
  return `sha256:${crypto.createHash("sha256").update(value).digest("hex")}`;
}

function transportRequest(egressPath, mediaPath) {
  const generation = sha256Label(Buffer.from("remote-wrapper-generation"));
  const expiresAtUnixMs = (Math.floor(Date.now() / 1000) + 300) * 1000;
  const username = `${expiresAtUnixMs / 1000}:remote-wrapper`;
  const authSecret = crypto.randomBytes(32).toString("base64url");
  const credential = crypto
    .createHmac("sha1", authSecret)
    .update(username)
    .digest("base64");
  const authority = {
    schema: "elastos.browser.vz-transport-authority/v1",
    generation,
    page_id: "page:remote-wrapper",
    vm_id: "vm:remote-wrapper",
    principal_id: "person:local:remote-wrapper",
    egress: {
      schema: "elastos.browser.vz-transport-stream/v1",
      stream_id: "stream:remote-wrapper",
      target: "tls://example.invalid:443",
      runtime_socket_path: egressPath,
      vsock_port: 19091,
    },
    media: {
      schema: "elastos.browser.vz-transport-stream/v1",
      stream_id: "stream:remote-wrapper-media",
      target: "tcp://127.0.0.1:49160",
      runtime_socket_path: mediaPath,
      vsock_port: 19094,
    },
    turn: {
      schema: "elastos.browser.vz-turn-authority/v1",
      guest_url: "turn:127.0.0.1:3478?transport=tcp",
      guest_host: "127.0.0.1",
      guest_port: 3478,
      listen_host: "127.0.0.1",
      listen_port: 49160,
      advertised_host: "127.0.0.1",
      relay_host: "127.0.0.1",
      relay_port_min: 55000,
      relay_port_max: 55003,
      protocols: ["turn", "tcp"],
      username,
      credential_hash: sha256Label(Buffer.from(credential)),
      auth_secret_hash: sha256Label(Buffer.from(authSecret)),
    },
    bootstrap_vsock_port: 19093,
    expires_at_unix_ms: expiresAtUnixMs,
  };
  authority.binding_hash = sha256Label(
    Buffer.from(JSON.stringify(canonicalJson(authority))),
  );
  return {
    schema: "elastos.browser.vm-engine.open/v1",
    launch_request: {
      schema: "elastos.browser.engine.launch-request/v1",
      adapter: "browser-engine-adapter",
      engine: "chromium_microvm",
      stream_id: authority.egress.stream_id,
      lifecycle_generation: generation,
      page_id: authority.page_id,
      vm_id: authority.vm_id,
      principal_id: authority.principal_id,
      target: authority.egress.target,
      display_mode: "webrtc_remote_display",
      guarantee_level: "mechanism_microvm",
      network_mode: "runtime_net_only",
      direct_network: false,
      wallet_injection: false,
      adapter_ipc: {
        kind: "unix_socket",
        runtime_stream_path: egressPath,
      },
      transport_authority: authority,
      transport_secret: {
        schema: "elastos.browser.vz-transport-secret/v1",
        binding_hash: authority.binding_hash,
        credential,
        auth_secret: authSecret,
      },
    },
  };
}

function transportResult(authority) {
  return {
    page_id: authority.page_id,
    vm_id: authority.vm_id,
    stream_id: authority.egress.stream_id,
    transport_authority: structuredClone(authority),
    transport_receipt: {
      schema: "elastos.browser.vz-transport-effect-receipt/v1",
      binding_hash: authority.binding_hash,
      generation: authority.generation,
      page_id: authority.page_id,
      vm_id: authority.vm_id,
      expires_at_unix_ms: authority.expires_at_unix_ms,
      terminal: true,
      effects: {
        vz_network_devices_zero: true,
        guest_bootstrap_validated: true,
        guest_loopback_only: true,
        guest_interfaces: ["lo"],
        guest_default_route_absent: true,
        guest_direct_network_absent: true,
        ordinary_stream_fixed_target: true,
        media_stream_fixed_target: true,
        turn_launch_owned: true,
        turn_listener_loopback: true,
        hibernation_disabled: true,
      },
    },
  };
}

async function listenUnix(socketPath) {
  const server = net.createServer();
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(socketPath, resolve);
  });
  return server;
}

test("wrapper emits exact did_not_act only after complete identity validation", async () => {
  const malformed = spawnSync(process.execPath, [launcherPath], {
    input: '{"schema":"malformed"}\n',
    encoding: "utf8",
    env: {
      ...process.env,
      ELASTOS_BROWSER_REMOTE_VZ_SSH: "",
    },
  });
  assert.equal(malformed.status, 1);
  assert.doesNotMatch(
    malformed.stderr,
    /elastos\.browser\.vz-launch-settlement\/v1/,
  );
  assert.doesNotMatch(malformed.stderr, /"binding_hash":null/);

  const root = fs.mkdtempSync(path.join(os.tmpdir(), "remote-vz-identity."));
  const egressPath = path.join(root, "egress.sock");
  const mediaPath = path.join(root, "media.sock");
  const egress = await listenUnix(egressPath);
  const media = await listenUnix(mediaPath);
  try {
    const request = transportRequest(egressPath, mediaPath);
    const validated = spawnSync(process.execPath, [launcherPath], {
      input: `${JSON.stringify(request)}\n`,
      encoding: "utf8",
      env: {
        ...process.env,
        ELASTOS_BROWSER_REMOTE_VZ_SSH: "",
      },
    });
    assert.equal(validated.status, 1);
    const settlement = launcher.parseVzLaunchSettlement(validated.stderr);
    assert.equal(settlement.state, "did_not_act");
    assert.equal(
      settlement.binding_hash,
      request.launch_request.transport_authority.binding_hash,
    );
    assert.equal(
      settlement.generation,
      request.launch_request.lifecycle_generation,
    );
    assert.ok(Object.values(settlement.effects).every((value) => value === false));
    assert.ok(Object.values(settlement.absence).every((value) => value === true));

    const invalidSsh = spawnSync(process.execPath, [launcherPath], {
      input: `${JSON.stringify(request)}\n`,
      encoding: "utf8",
      env: {
        ...process.env,
        ELASTOS_BROWSER_REMOTE_VZ_SSH: "unreached-fake-host",
        ELASTOS_BROWSER_REMOTE_VZ_SSH_BIN: path.join(root, "missing-ssh"),
      },
    });
    assert.equal(invalidSsh.status, 1);
    const invalidSshSettlement = launcher.parseVzLaunchSettlement(
      invalidSsh.stderr,
    );
    assert.equal(invalidSshSettlement.state, "did_not_act");
    assert.equal(
      invalidSshSettlement.binding_hash,
      request.launch_request.transport_authority.binding_hash,
    );
    assert.match(
      invalidSshSettlement.message,
      /must resolve to an executable regular file/,
    );
    assert.ok(
      Object.values(invalidSshSettlement.effects).every(
        (value) => value === false,
      ),
    );
  } finally {
    await Promise.all([
      new Promise((resolve) => egress.close(resolve)),
      new Promise((resolve) => media.close(resolve)),
    ]);
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("pre-effect exact path collision preserves the foreign path and creates no residue", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "remote-vz-preflight."));
  const egressPath = path.join(root, "egress.sock");
  const mediaPath = path.join(root, "media.sock");
  const localSocketRoot = path.join(root, "socket-root");
  const localSessionRoot = path.join(
    root,
    ...Array.from({ length: 12 }, (_, index) => `long-session-${index}`),
  );
  const egress = await listenUnix(egressPath);
  const media = await listenUnix(mediaPath);
  try {
    const request = transportRequest(egressPath, mediaPath);
    const digest = request.launch_request.transport_authority.binding_hash.slice(
      "sha256:".length,
      39,
    );
    const collision = path.join(localSocketRoot, digest);
    fs.mkdirSync(collision, { recursive: true, mode: 0o700 });
    fs.writeFileSync(path.join(collision, "foreign-evidence"), "preserve\n", {
      mode: 0o600,
    });

    const result = spawnSync(process.execPath, [launcherPath], {
      input: `${JSON.stringify(request)}\n`,
      encoding: "utf8",
      env: {
        ...process.env,
        ELASTOS_BROWSER_REMOTE_VZ_SSH: "unreached-fake-host",
        ELASTOS_BROWSER_REMOTE_VZ_SSH_BIN: "/usr/bin/false",
        ELASTOS_BROWSER_REMOTE_VZ_DATA_DIR: "/tmp/unreached-vz-data",
        ELASTOS_BROWSER_REMOTE_VZ_TURN_PROGRAM: "/usr/bin/true",
        ELASTOS_BROWSER_REMOTE_VZ_LOCAL_ROOT: localSessionRoot,
        ELASTOS_BROWSER_REMOTE_VZ_SOCKET_ROOT: localSocketRoot,
      },
    });
    assert.equal(result.status, 1);
    const settlement = launcher.parseVzLaunchSettlement(result.stderr);
    assert.equal(settlement.state, "did_not_act");
    assert.ok(Object.values(settlement.effects).every((value) => value === false));
    assert.ok(Object.values(settlement.absence).every((value) => value === true));
    assert.equal(
      fs.readFileSync(path.join(collision, "foreign-evidence"), "utf8"),
      "preserve\n",
    );
    assert.equal(fs.existsSync(localSessionRoot), false);
  } finally {
    await Promise.all([
      new Promise((resolve) => egress.close(resolve)),
      new Promise((resolve) => media.close(resolve)),
    ]);
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("mixed legacy VZ configuration fails after exact binding but before effects", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "remote-vz-mixed."));
  const egressPath = path.join(root, "egress.sock");
  const mediaPath = path.join(root, "media.sock");
  const egress = await listenUnix(egressPath);
  const media = await listenUnix(mediaPath);
  try {
    const request = transportRequest(egressPath, mediaPath);
    const result = spawnSync(process.execPath, [launcherPath], {
      input: `${JSON.stringify(request)}\n`,
      encoding: "utf8",
      env: {
        ...process.env,
        ELASTOS_BROWSER_REMOTE_VZ_SSH: "unreached-fake-host",
        ELASTOS_BROWSER_REMOTE_VZ_SSH_BIN: "/usr/bin/false",
        ELASTOS_BROWSER_VM_HIBERNATION_MAX_ENTRIES: "4",
      },
    });
    assert.equal(result.status, 1);
    const settlement = launcher.parseVzLaunchSettlement(result.stderr);
    assert.equal(settlement.state, "did_not_act");
    assert.match(settlement.message, /legacy or mixed transport configuration/);
    assert.ok(Object.values(settlement.effects).every((value) => value === false));
    assert.ok(Object.values(settlement.absence).every((value) => value === true));
  } finally {
    await Promise.all([
      new Promise((resolve) => egress.close(resolve)),
      new Promise((resolve) => media.close(resolve)),
    ]);
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("remote VZ wrapper fail-closes transport and preflights a short exact binding", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "remote-vz-wrapper."));
  const egressPath = path.join(root, "egress.sock");
  const mediaPath = path.join(root, "media.sock");
  const egress = await listenUnix(egressPath);
  const media = await listenUnix(mediaPath);
  try {
    const request = transportRequest(egressPath, mediaPath);
    const launch = launcher.validateOpenRequest(request);
    const paths = launcher.boundSocketPaths(launch.transport_authority);
    assert.ok(Buffer.byteLength(paths.local_control) < 100);
    assert.ok(Buffer.byteLength(paths.remote_control) < 100);
    assert.ok(Buffer.byteLength(paths.remote_session) > 100);
    assert.match(
      path.basename(paths.local_directory),
      /^[0-9a-f]{32}$/,
    );
    assert.equal(
      path.basename(paths.local_directory),
      launch.transport_authority.binding_hash.slice(7, 39),
    );

    const preflight = launcher.remotePreflightCommand({
      remoteDataDir:
        "/Users/test/Library/Application Support/elastos",
      remoteVmEnv: {
        ELASTOS_BROWSER_VM_TURN_PROGRAM: "/opt/homebrew/bin/turnserver",
      },
      paths,
      authority: launch.transport_authority,
      relayTcpPorts: [31001, 31002],
      pidPaths: [
        "/tmp/evzst/relay.pid",
        "/tmp/evzst/media-relay.pid",
        "/tmp/evzst/supervisor.pid",
      ],
    });
    for (const required of [
      "browser-vz-engine-supervisor",
      "--elastos-vz-binding=",
      "browser-vm-engine-supervisor.mjs",
      "browser-vm/rootfs.ext4",
      "codesign --verify --strict",
      "com.apple.security.virtualization",
      "command -v lsof",
      "command -v python3",
      "stat -f %Lp",
      "ps -ww -axo command=",
      paths.remote_control,
      paths.remote_session,
    ]) {
      assert.ok(preflight.includes(required), `missing preflight: ${required}`);
    }
    assert.ok(
      !preflight.includes(launch.transport_secret.auth_secret),
      "preflight exposed a private transport secret",
    );
    const absenceChecks = launcher.remoteTransportAbsenceChecks(
      launch.transport_authority,
      paths,
      [
        "/tmp/evzst/relay.pid",
        "/tmp/evzst/media-relay.pid",
        "/tmp/evzst/supervisor.pid",
      ],
      [31001, 31002],
    );
    assert.deepEqual(
      absenceChecks.map(({ field }) => field),
      [
        "supervisor_child_absent",
        "control_socket_absent",
        "turn_listener_absent",
        "turn_relay_ports_absent",
        "ordinary_stream_bridge_absent",
        "media_stream_bridge_absent",
        "session_directory_absent",
      ],
    );
    const ordinaryAbsence = absenceChecks.find(
      ({ field }) => field === "ordinary_stream_bridge_absent",
    ).command;
    const mediaAbsence = absenceChecks.find(
      ({ field }) => field === "media_stream_bridge_absent",
    ).command;
    assert.ok(ordinaryAbsence.includes("31001"));
    assert.ok(!ordinaryAbsence.includes("31002"));
    assert.ok(mediaAbsence.includes("31002"));
    assert.ok(!mediaAbsence.includes("31001"));
    const supervisorAbsence = absenceChecks.find(
      ({ field }) => field === "supervisor_child_absent",
    ).command;
    assert.ok(supervisorAbsence.includes("ps -ww -axo command="));
    assert.ok(supervisorAbsence.includes("--elastos-vz-binding="));
    assert.ok(
      supervisorAbsence.includes(launch.transport_authority.binding_hash),
    );
    const cleanup = launcher.remoteRelayCleanupCommand(
      "/tmp/evzst/relay.pid",
      paths.remote_control,
    );
    assert.ok(cleanup.includes('proc_command=$(/bin/ps -ww -p "$pid"'));
    assert.ok(cleanup.includes('while kill -0 "$pid"'));

    const missing = structuredClone(request);
    delete missing.launch_request.transport_authority;
    assert.throws(
      () => launcher.validateOpenRequest(missing),
      /invalid or stale Browser VZ transport authority/,
    );

    const stale = structuredClone(request);
    stale.launch_request.transport_authority.expires_at_unix_ms = 1;
    assert.throws(
      () => launcher.validateOpenRequest(stale),
      /invalid or stale Browser VZ transport authority/,
    );

    const mixed = structuredClone(request);
    mixed.launch_request.transport_authority.media.runtime_socket_path =
      mixed.launch_request.transport_authority.egress.runtime_socket_path;
    assert.throws(
      () => launcher.validateOpenRequest(mixed),
      /bindings are not distinct/,
    );

    const result = transportResult(launch.transport_authority);
    assert.equal(
      launcher.validateRemoteTransportResult(
        result,
        launch.transport_authority,
      ),
      result,
    );
    const substituted = structuredClone(result);
    substituted.transport_receipt.effects.vz_network_devices_zero = false;
    assert.throws(
      () =>
        launcher.validateRemoteTransportResult(
          substituted,
          launch.transport_authority,
        ),
      /invalid exact transport effect receipt/,
    );
    const exposed = structuredClone(result);
    exposed.transport_secret = { auth_secret: "not-public" };
    assert.throws(
      () =>
        launcher.validateRemoteTransportResult(
          exposed,
          launch.transport_authority,
        ),
      /invalid exact transport effect receipt/,
    );
  } finally {
    await Promise.all([
      new Promise((resolve) => egress.close(resolve)),
      new Promise((resolve) => media.close(resolve)),
    ]);
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("remote supervisor cleanup finds and reaps the exact binding without a pid file", () => {
  const root = fs.mkdtempSync(
    path.join(os.tmpdir(), "remote-vz-supervisor-owner."),
  );
  const pidPath = path.join(root, "intentionally-missing.pid");
  const probeOwnerPath = path.join(root, "probe-owner.pid");
  const bindingHash = `sha256:${crypto.randomBytes(32).toString("hex")}`;
  const cleanup = launcher.remoteSupervisorCleanupCommand(
    pidPath,
    bindingHash,
    process.execPath,
  );
  try {
    const result = spawnSync(
      "/bin/bash",
      [
        "-c",
        [
          "set -euo pipefail",
          '"$PROBE_NODE" -e \'setInterval(() => {}, 1000)\' browser-vz-engine-supervisor "--elastos-vz-binding=$PROBE_BINDING" &',
          "owned_pid=$!",
          'printf \'%s\\n\' "$owned_pid" >"$PROBE_OWNER_PATH"',
          "sleep 0.1",
          cleanup,
          'if kill -0 "$owned_pid" 2>/dev/null; then exit 90; fi',
          'wait "$owned_pid" 2>/dev/null || true',
          '[ ! -e "$PROBE_PID_FILE" ]',
        ].join("\n"),
      ],
      {
        encoding: "utf8",
        timeout: 10_000,
        env: {
          ...process.env,
          PROBE_NODE: process.execPath,
          PROBE_BINDING: bindingHash,
          PROBE_PID_FILE: pidPath,
          PROBE_OWNER_PATH: probeOwnerPath,
        },
      },
    );
    assert.equal(result.error, undefined, result.error?.message);
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.equal(fs.existsSync(pidPath), false);
  } finally {
    if (fs.existsSync(probeOwnerPath)) {
      const pid = Number(fs.readFileSync(probeOwnerPath, "utf8").trim());
      if (Number.isInteger(pid) && pid > 1) {
        try {
          process.kill(pid, "SIGKILL");
        } catch {}
      }
    }
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("remote VZ typed settlement parser preserves the exact request binding", () => {
  const settlement = {
    schema: "elastos.browser.vz-launch-settlement/v1",
    state: "terminal_post_effect_cleanup",
    message: "injected post-effect failure",
    binding_hash: `sha256:${"a".repeat(64)}`,
    generation: `sha256:${"b".repeat(64)}`,
    page_id: "page:settlement",
    vm_id: "vm:settlement",
    stream_id: "stream:settlement",
    media_stream_id: "stream:settlement-media",
    effects: {
      session_directory: true,
      control_socket: true,
      ordinary_stream_bridge: true,
      media_stream_bridge: true,
      turn_process: true,
      supervisor_child: true,
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
  assert.deepEqual(
    launcher.parseVzLaunchSettlement(
      `[remote-vz supervisor] ${JSON.stringify(settlement)}\n`,
    ),
    settlement,
  );
});

test("one failed absence proof does not contaminate independent cleanup fields", () => {
  const suffix = `${process.pid}-${Date.now().toString(36)}`;
  const request = transportRequest(
    `/tmp/evzfi-${suffix}-egress.sock`,
    `/tmp/evzfi-${suffix}-media.sock`,
  );
  request.profile = {
    schema: "elastos.browser.profile/v1",
    uri: "localhost://Users/wrapper-field-proof/BrowserProfiles/default/profile.ext4",
    persistent: true,
  };
  const result = spawnSync(process.execPath, [launcherIntegrationPath], {
    input: `${JSON.stringify(request)}\n`,
    encoding: "utf8",
    timeout: 30_000,
    env: {
      ...process.env,
      ELASTOS_BROWSER_TEST_FAIL_ABSENCE_FIELD:
        "media_stream_bridge_absent",
    },
  });
  assert.equal(result.status, 0, result.stderr);
  const proof = JSON.parse(result.stdout);
  assert.equal(proof.terminal, false);
  assert.equal(
    proof.failed_absence_field,
    "media_stream_bridge_absent",
  );
  assert.equal(proof.zero_owned_residue, true);
});
