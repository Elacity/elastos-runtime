import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { waitForRetiringVmControlService } from "./browser-vm-engine-supervisor.mjs";
import { readinessWhileAccepting } from "./browser-vm-control-service.mjs";

test("a probe that finishes after retirement reports unavailable", async () => {
  let accepting = true;
  let finishProbe;
  const pending = readinessWhileAccepting(() => accepting,
    () => new Promise(resolve => { finishProbe = resolve; }));
  accepting = false;
  finishProbe({ schema: "elastos.browser.engine-readiness/v1", readiness: { state: "ready" } });
  assert.deepEqual(await pending, {
    schema: "elastos.browser.engine-readiness/v1",
    readiness: { state: "unavailable", reason: "control_unavailable" },
  });
});

test("a retiring service does not start another host probe", async () => {
  const result = await readinessWhileAccepting(() => false, () => assert.fail("host probe ran"));
  assert.equal(result.readiness.reason, "control_unavailable");
});

async function retiringService(t, beforeResponse = () => {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "bvm-retire-"));
  const socket = path.join(root, "control.sock");
  const requests = [];
  const status = {
    schema: "elastos.browser.vm-control-service.status/v1", ok: true,
    accepting_launches: false, config_fingerprint: "a".repeat(64), active_pages: 1,
  };
  const server = http.createServer((req, res) => {
    requests.push([req.method, req.url]);
    beforeResponse(status);
    res.end(JSON.stringify(status));
  });
  await new Promise(resolve => server.listen(socket, resolve));
  const stop = () => new Promise(resolve => server.close(resolve));
  t.after(async () => {
    server.closeAllConnections();
    if (server.listening) await stop();
    fs.rmSync(root, { recursive: true, force: true });
  });
  return { socket, requests, stop, observed: { kind: "verified", status } };
}

test("a retiring service keeps ownership until its socket disappears", async t => {
  const service = await retiringService(t);
  let retired = false;
  const timer = setTimeout(async () => { await service.stop(); retired = true; }, 250);
  t.after(() => clearTimeout(timer));
  const result = await waitForRetiringVmControlService(service.socket, service.observed, 2000);
  assert.equal(retired, true);
  assert.equal(result.kind, "absent");
  assert.ok(service.requests.length >= 1);
  assert.ok(service.requests.every(([method, url]) => method === "GET" && url === "/status"));
});

test("a delayed retirement remains bounded and preserves the live socket", async t => {
  const service = await retiringService(t);
  const before = fs.lstatSync(service.socket);
  await assert.rejects(waitForRetiringVmControlService(service.socket, service.observed, 150),
    error => error.code === "cleanup_pending");
  const after = fs.lstatSync(service.socket);
  assert.equal(after.ino, before.ino);
  assert.equal(after.dev, before.dev);
  assert.ok(service.requests.every(([method]) => method === "GET"));
});

test("a sleep that consumes the retirement deadline starts no new probe", async t => {
  const service = await retiringService(t);
  const before = fs.lstatSync(service.socket);
  t.mock.timers.enable({ apis: ["Date", "setTimeout"], now: 1000 });
  const result = waitForRetiringVmControlService(service.socket, service.observed, 150);
  t.mock.timers.tick(150);
  await assert.rejects(result, error => error.code === "cleanup_pending");
  assert.equal(service.requests.length, 0);
  const after = fs.lstatSync(service.socket);
  assert.equal(after.ino, before.ino);
  assert.equal(after.dev, before.dev);
  t.mock.timers.reset();
});

test("a ready status response after the retirement deadline stays cleanup pending", async t => {
  const service = await retiringService(t, status => {
    status.accepting_launches = true;
    t.mock.timers.setTime(1150);
  });
  const before = fs.lstatSync(service.socket);
  t.mock.timers.enable({ apis: ["Date"], now: 1000 });
  await assert.rejects(waitForRetiringVmControlService(service.socket, service.observed, 150),
    error => error.code === "cleanup_pending");
  assert.equal(service.requests.length, 1);
  const after = fs.lstatSync(service.socket);
  assert.equal(after.ino, before.ino);
  assert.equal(after.dev, before.dev);
  t.mock.timers.reset();
});

test("a substituted path remains unverified and intact after retirement", async t => {
  const service = await retiringService(t);
  const timer = setTimeout(async () => {
    await service.stop();
    fs.writeFileSync(service.socket, "foreign path");
  }, 150);
  t.after(() => clearTimeout(timer));
  const result = await waitForRetiringVmControlService(service.socket, service.observed, 2000);
  assert.equal(result.kind, "unverified");
  assert.equal(fs.readFileSync(service.socket, "utf8"), "foreign path");
});

test("an accepting service preserves its current readiness observation", async () => {
  const current = { kind: "verified", status: { accepting_launches: true } };
  assert.equal(await waitForRetiringVmControlService("/unused", current), current);
});
