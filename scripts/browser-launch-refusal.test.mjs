import assert from "node:assert/strict";
import test from "node:test";
import http from "node:http";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { launcherError, launchRefusalFields } from "./browser-vm-control-service.mjs";
import { postJsonOverUnix } from "./browser-vm-engine-supervisor.mjs";

const refusal = { code: "insufficient_free_space", required_bytes: 8589934592, available_bytes: 8589934591 };
const typed = { schema: "elastos.browser.engine.launch-error/v1", code: "launch_refused", message: "Browser cannot open.", refusal };

test("control service recovers typed space refusal from native cleanup receipt", () => {
  const settlement = {
    schema: "elastos.browser.vz-launch-settlement/v1", state: "terminal_post_effect_cleanup",
    message: JSON.stringify(typed), binding_hash: `sha256:${"a".repeat(64)}`,
    generation: `sha256:${"b".repeat(64)}`, page_id: "page:test", vm_id: "vm:test",
    stream_id: "stream:test", media_stream_id: "stream:media",
    effects: Object.fromEntries(["session_directory", "control_socket", "ordinary_stream_bridge", "media_stream_bridge", "turn_process", "supervisor_child", "vm"].map(k => [k, k === "session_directory"])),
    absence: Object.fromEntries(["child_absent", "supervisor_child_absent", "control_socket_absent", "route_absent", "turn_listener_absent", "turn_relay_ports_absent", "ordinary_stream_bridge_absent", "media_stream_bridge_absent", "session_directory_absent", "vm_absent"].map(k => [k, true])),
  };
  const error = launcherError("launch failed", JSON.stringify(settlement));
  assert.equal(error.code, "launch_refused");
  assert.deepEqual(error.refusal, refusal);
  assert.equal(error.launch_settlement, "terminal_post_effect_cleanup");
});

test("control service accepts only the closed, bounded refusal shape", () => {
  assert.deepEqual(launcherError("failed", JSON.stringify(typed)).refusal, refusal);
  for (const bad of [
    { ...refusal, code: "unknown" }, { ...refusal, required_bytes: "8589934592" },
    { ...refusal, required_bytes: 2 ** 53 }, { ...refusal, available_bytes: -1 },
    { ...refusal, available_bytes: refusal.required_bytes }, { ...refusal, message: "private" },
  ]) {
    assert.equal(launcherError("failed", JSON.stringify({ ...typed, refusal: bad })).refusal, undefined);
  }
  assert.equal(launcherError("failed", "arbitrary engine failure").refusal, undefined);
});

test("supervisor carries the control service's safe refusal through its Unix response", async t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "browser-refusal-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const socket = path.join(root, "control.sock");
  const server = http.createServer((_request, response) => {
    const error = launcherError("failed", JSON.stringify(typed));
    response.writeHead(400, { "content-type": "application/json" });
    response.end(JSON.stringify({ code: error.code, message: error.message, ...launchRefusalFields(error) }));
  });
  await new Promise(resolve => server.listen(socket, resolve));
  t.after(() => new Promise(resolve => server.close(resolve)));
  await assert.rejects(postJsonOverUnix(socket, "/pages", {}, 1000), error => {
    assert.equal(error.code, "launch_refused");
    assert.deepEqual(error.refusal, refusal);
    return true;
  });
});
