import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import vm from "node:vm";

import {
  directNetworkErrorProvesUnavailable,
  guestNetworkState,
  proveDirectNetworkUnavailable,
  validateAuthority,
  validateSecret,
  writeOwnerOnlyAtomic,
} from "./browser-vm-vz-transport-bootstrap.mjs";

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

function fixture(suffix = "a", expiresAtUnixMs = (Math.floor(Date.now() / 1000) + 300) * 1000) {
  const authSecret = crypto.randomBytes(32).toString("base64url");
  const username = `${expiresAtUnixMs / 1000}:${suffix.repeat(16)}`;
  const credential = crypto
    .createHmac("sha1", authSecret)
    .update(username)
    .digest("base64");
  const authority = {
    schema: "elastos.browser.vz-transport-authority/v1",
    generation: `sha256:${suffix.repeat(64)}`,
    page_id: `page:vz-${suffix}`,
    vm_id: `vm:vz-${suffix}`,
    principal_id: `person:local:${suffix}`,
    egress: {
      schema: "elastos.browser.vz-transport-stream/v1",
      stream_id: `stream:egress-${suffix}`,
      target: "tls://example.invalid:443",
      runtime_socket_path: `/tmp/vz-egress-${suffix}.sock`,
      vsock_port: 19091,
    },
    media: {
      schema: "elastos.browser.vz-transport-stream/v1",
      stream_id: `stream:media-${suffix}`,
      target: "tcp://127.0.0.1:49160",
      runtime_socket_path: `/tmp/vz-media-${suffix}.sock`,
      vsock_port: 19094,
    },
    turn: {
      schema: "elastos.browser.vz-turn-authority/v1",
      guest_url: "turn:127.0.0.1:3478?transport=tcp",
      guest_host: "127.0.0.1",
      guest_port: 3478,
      listen_host: "127.0.0.1",
      listen_port: 49160,
      advertised_host: "192.0.2.10",
      relay_host: "192.0.2.10",
      relay_port_min: 55000,
      relay_port_max: 55019,
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
    authority,
    secret: {
      schema: "elastos.browser.vz-transport-secret/v1",
      binding_hash: authority.binding_hash,
      credential,
      auth_secret: authSecret,
    },
  };
}

test("bootstrap rejects substitution, replay, and expiry", () => {
  const first = fixture("a");
  const second = fixture("b");
  assert.equal(validateAuthority(first.authority), first.authority);
  assert.equal(validateSecret(first.authority, first.secret), first.secret);

  const substituted = structuredClone(first.authority);
  substituted.vm_id = "vm:vz-substituted";
  assert.throws(() => validateAuthority(substituted), /hash mismatch/);
  assert.throws(
    () => validateSecret(first.authority, second.secret),
    /secret is invalid/,
  );

  const expired = fixture("c", 1_000);
  assert.throws(() => validateAuthority(expired.authority), /authority is invalid/);
});

test("bootstrap proves loopback-only guest state and owner-only outputs", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "vz-bootstrap-test-"));
  try {
    const netClass = path.join(root, "net");
    fs.mkdirSync(path.join(netClass, "lo"), { recursive: true });
    const ipv4 = path.join(root, "route");
    const ipv6 = path.join(root, "ipv6_route");
    fs.writeFileSync(
      ipv4,
      "Iface\tDestination\tGateway\tFlags\nlo\t0000007F\t00000000\t0001\n",
    );
    fs.writeFileSync(
      ipv6,
      [
        `${"0".repeat(32)} 00 ${"0".repeat(32)} 00 ${"0".repeat(
          32,
        )} ffffffff 00000001 00000000 00200200 lo`,
        `${"0".repeat(31)}1 80 ${"0".repeat(32)} 00 ${"0".repeat(
          32,
        )} 00000000 00000002 00000000 80200001 lo`,
        "",
      ].join("\n"),
    );
    assert.deepEqual(
      guestNetworkState({
        netClassPath: netClass,
        ipv4RoutePath: ipv4,
        ipv6RoutePath: ipv6,
      }),
      { interfaces: ["lo"], default_route_absent: true },
    );

    fs.writeFileSync(
      ipv6,
      `${"0".repeat(32)} 00 ${"0".repeat(32)} 00 ${"0".repeat(
        32,
      )} 00000000 00000001 00000000 00000001 lo\n`,
    );
    assert.throws(
      () =>
        guestNetworkState({
          netClassPath: netClass,
          ipv4RoutePath: ipv4,
          ipv6RoutePath: ipv6,
        }),
      /unexpectedly has a default route/,
    );

    fs.writeFileSync(ipv6, "");
    fs.mkdirSync(path.join(netClass, "eth0"));
    assert.throws(
      () =>
        guestNetworkState({
          netClassPath: netClass,
          ipv4RoutePath: ipv4,
          ipv6RoutePath: ipv6,
        }),
      /unexpected network interfaces/,
    );
    fs.rmSync(path.join(netClass, "eth0"), { recursive: true });

    const output = path.join(root, "authority.json");
    writeOwnerOnlyAtomic(output, fixture("d").authority);
    assert.equal(fs.statSync(output).mode & 0o077, 0);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("bootstrap direct-network probe fails closed on a reachable target", async () => {
  assert.equal(
    directNetworkErrorProvesUnavailable({ code: "ENETUNREACH" }),
    true,
  );
  assert.equal(
    directNetworkErrorProvesUnavailable({ code: "ECONNREFUSED" }),
    false,
  );
  assert.equal(
    directNetworkErrorProvesUnavailable({ code: "ETIMEDOUT" }),
    false,
  );
  const server = net.createServer((socket) => socket.end());
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  const launch = fixture("e").authority;
  launch.egress.target = `tcp://127.0.0.1:${address.port}`;
  await assert.rejects(
    proveDirectNetworkUnavailable(launch, 1_000),
    /unexpectedly reached/,
  );
  await new Promise((resolve) => server.close(resolve));
  await assert.rejects(
    proveDirectNetworkUnavailable(launch, 1_000),
    /indeterminate \(ECONNREFUSED\)/,
  );
});

test("bootstrap flushes its bound receipt and closes without waiting for the descriptor deadline", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "vz-bootstrap-close-"));
  const socketPath = path.join(root, "relay.sock");
  const descriptor = fixture("f");
  const sockets = new Set();
  let data = "";
  let peerEnded;
  const ended = new Promise(resolve => { peerEnded = resolve; });
  let clientClosed;
  const closed = new Promise(resolve => { clientClosed = resolve; });
  // Match the relay/host: receive until EOF while keeping its write side open.
  const server = net.createServer({ allowHalfOpen: true }, socket => {
    sockets.add(socket);
    socket.on("data", chunk => { data += chunk; });
    socket.on("end", peerEnded);
    socket.write(`${JSON.stringify({ schema: "elastos.browser.vz-transport-bootstrap/v1", ...descriptor })}\n`);
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(socketPath, resolve);
  });
  const source = fs.readFileSync(new URL("./browser-vm-vz-transport-bootstrap.mjs", import.meta.url), "utf8");
  const program = source.slice(source.indexOf("function readDescriptor("), source.indexOf("\nexport {"));
  const deadlineTimers = new Set();
  const context = vm.createContext({
    Buffer,
    JSON,
    net: { createConnection(options) {
      const socket = net.createConnection(options);
      sockets.add(socket);
      socket.once("close", clientClosed);
      return socket;
    } },
    MAX_DESCRIPTOR_BYTES: 64 * 1024,
    REQUEST_SCHEMA: "elastos.browser.vz-transport-bootstrap/v1",
    RECEIPT_SCHEMA: "elastos.browser.vz-transport-bootstrap-receipt/v1",
    setTimeout(callback, delay) { const timer = setTimeout(callback, delay); deadlineTimers.add(timer); return timer; },
    clearTimeout(timer) { clearTimeout(timer); deadlineTimers.delete(timer); },
    readConfig: () => ({ relay_socket_path: socketPath,
      authority_path: path.join(root, "authority.json"), ice_servers_path: path.join(root, "ice.json") }),
    exactObjectKeys: (value, keys) => Object.keys(value).length === keys.length && keys.every(key => Object.hasOwn(value, key)),
    validateAuthority, validateSecret, writeOwnerOnlyAtomic,
    guestNetworkState: () => ({ interfaces: ["lo"], default_route_absent: true }),
    proveDirectNetworkUnavailable: async () => true,
  });
  let failureTimer;
  try {
    await vm.runInContext(`${program}\nmain()`, context);
    await Promise.race([Promise.all([ended, closed]), new Promise((_, reject) => {
      failureTimer = setTimeout(() => reject(new Error("bootstrap connection remains open after receipt write")), 500);
    })]);
    const receipt = JSON.parse(data);
    assert.equal(receipt.schema, "elastos.browser.vz-transport-bootstrap-receipt/v1");
    assert.equal(receipt.binding_hash, descriptor.authority.binding_hash);
    assert.equal(receipt.generation, descriptor.authority.generation);
    assert.equal(receipt.terminal, true);
    assert.equal(deadlineTimers.size, 0, "a completed descriptor has no pending read deadline");
  } finally {
    clearTimeout(failureTimer);
    for (const timer of deadlineTimers) clearTimeout(timer);
    for (const socket of sockets) socket.destroy();
    await new Promise(resolve => server.close(resolve));
    fs.rmSync(root, { recursive: true, force: true });
  }
});
