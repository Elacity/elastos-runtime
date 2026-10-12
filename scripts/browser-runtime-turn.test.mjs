import assert from "node:assert/strict";
import crypto from "node:crypto";
import dgram from "node:dgram";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import test from "node:test";
import { credentialedTurnAllocation, readTurnPacket, turnPacket } from "./browser-runtime-turn.mjs";

test("TURN replies require the transaction, packet bounds and valid integrity", () => {
  const transaction = crypto.randomBytes(12), key = crypto.randomBytes(16);
  const packet = turnPacket(0x0103, transaction, [], key);
  assert.equal(readTurnPacket(packet, transaction, key).type, 0x0103);
  assert.equal(readTurnPacket(packet, crypto.randomBytes(12), key), null);
  assert.equal(readTurnPacket(packet, transaction, crypto.randomBytes(16)), null);
  assert.equal(readTurnPacket(packet.subarray(0, 19), transaction, key), null);
  assert.equal(readTurnPacket(Buffer.concat([packet, Buffer.alloc(4)]), transaction, key), null);
  assert.equal(readTurnPacket(turnPacket(0x0103, transaction, []), transaction, key), null);
  const forged = Buffer.from(packet); forged[forged.length - 1] ^= 1;
  assert.equal(readTurnPacket(forged, transaction, key), null);
});

test("TURN credential probe times out and keeps its password out of errors", async t => {
  const server = dgram.createSocket("udp4");
  await new Promise(resolve => server.bind(0, "127.0.0.1", resolve));
  t.after(() => server.close());
  const start = Date.now();
  const result = await credentialedTurnAllocation("127.0.0.1", server.address().port, "private-test-password");
  assert.deepEqual(result, { ok: false, error: "TURN probe timed out" });
  assert.ok(Date.now() - start < 3000);
});

test("source Home TURN credential and config aliases are refused", t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "elastos-private-turn-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.mkdirSync(path.join(root, "runtime-turn"));
  const sentinel = path.join(root, "sentinel"); fs.writeFileSync(sentinel, "preserve");
  fs.symlinkSync(sentinel, path.join(root, "runtime-turn/turn-credentials.env"));
  const result = spawnSync(process.execPath, [new URL("./browser-runtime-turn.mjs", import.meta.url).pathname,
    "--data-dir", root, "--host-ip", "10.44.0.10", "--media-host-ip", "10.44.0.1", "--no-start"], { encoding: "utf8" });
  assert.equal(result.status, 1);
  assert.equal(fs.readFileSync(sentinel, "utf8"), "preserve");
});

test("credential probe allocates and releases against installed coturn", async t => {
  const binary = ["/opt/homebrew/bin/turnserver", "/usr/bin/turnserver"].find(file => fs.existsSync(file));
  if (!binary) return t.skip("coturn hardware-independent integration is optional on this host");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "elastos-turn-probe-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const reservation = dgram.createSocket("udp4");
  await new Promise(resolve => reservation.bind(0, "127.0.0.1", resolve));
  const port = reservation.address().port;
  await new Promise(resolve => reservation.close(resolve));
  const config = path.join(root, "turn.conf"), log = path.join(root, "turn.log");
  fs.writeFileSync(config, ["listening-ip=127.0.0.1", "relay-ip=127.0.0.1", `listening-port=${port}`,
    "realm=fixture", "lt-cred-mech", "user=elastos-browser:private-test-password", "no-cli", "no-tls", "no-dtls",
    "no-tcp-relay", "min-port=55000", "max-port=55002", "relay-threads=1", `pidfile=${root}/turn.pid`, `log-file=${log}`].join("\n"), { mode: 0o600 });
  const child = spawn(binary, ["-c", config], { stdio: "ignore" });
  t.after(async () => {
    if (child.exitCode === null) { child.kill("SIGTERM"); await new Promise(resolve => child.once("exit", resolve)); }
  });
  let result;
  for (let attempt = 0; attempt < 5; attempt++) {
    result = await credentialedTurnAllocation("127.0.0.1", port, "private-test-password");
    if (result.ok || child.exitCode !== null) break;
  }
  assert.equal(result.ok, true, result.error);
  const refused = await credentialedTurnAllocation("127.0.0.1", port, "wrong-private-password");
  assert.equal(refused.ok, false);
  assert.doesNotMatch(refused.error, /password/);
});
