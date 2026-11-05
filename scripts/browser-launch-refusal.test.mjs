import assert from "node:assert/strict";
import test from "node:test";
import http from "node:http";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { fileURLToPath } from "node:url";

const refusal = { code: "insufficient_free_space", required_bytes: 8589934592, available_bytes: 8589934591 };
const typed = { schema: "elastos.browser.engine.launch-error/v1", code: "launch_refused", message: "Browser cannot open.", refusal };

async function controlServiceResponse(t, envelope) {
  // Exercise the real supervisor stderr -> control service -> HTTP body path.
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "browser-refusal-"));
  const socket = path.join(root, "control.sock");
  const launcher = path.join(root, "launcher.mjs");
  fs.writeFileSync(launcher, `
    for await (const chunk of process.stdin) {} // Drain the launch request.
    console.error(process.env.TEST_LAUNCH_ERROR);
    process.exit(73);
  `);
  const env = { ...process.env,
    TEST_LAUNCH_ERROR: JSON.stringify(envelope),
    ELASTOS_BROWSER_VM_CONTROL_SERVICE_CONFIG: JSON.stringify({
      schema: "elastos.browser.vm-control-service.config/v1",
      control_socket_path: socket, launcher_program: process.execPath,
      launcher_args: [launcher], persistent_launcher: true,
      launch_timeout_ms: 3000, shutdown_timeout_ms: 1000,
    }),
  };
  delete env.ELASTOS_UPDATE_PARENT_PIPE;
  const service = spawn(process.execPath, [fileURLToPath(new URL("./browser-vm-control-service.mjs", import.meta.url))], {
    env, stdio: ["ignore", "pipe", "pipe"],
  });
  let stderr = "";
  service.stderr.on("data", chunk => { stderr = (stderr + chunk).slice(-8192); });
  t.after(async () => {
    if (service.exitCode === null && service.signalCode === null) {
      const exited = once(service, "exit");
      service.kill("SIGTERM");
      const timer = setTimeout(() => service.kill("SIGKILL"), 3000);
      try { await exited; } finally { clearTimeout(timer); }
    }
    fs.rmSync(root, { recursive: true, force: true });
  });
  await new Promise((resolve, reject) => {
    let stdout = "";
    const timer = setTimeout(() => reject(new Error(`control service readiness timed out: ${stderr}`)), 5000);
    service.once("error", reject);
    service.once("exit", () => { clearTimeout(timer); reject(new Error(`control service exited: ${stderr}`)); });
    service.stdout.on("data", chunk => {
      stdout += chunk;
      if (stdout.includes('"schema":"elastos.browser.vm-control-service.ready/v1"')) {
        clearTimeout(timer);
        resolve();
      }
    });
  });
  const body = JSON.stringify({
    schema: "elastos.browser.vm-engine.open/v1",
    launch_request: {
      schema: "elastos.browser.engine.launch-request/v1",
      adapter: "browser-vm-product", engine: "chromium_microvm",
      url: "https://example.com/", stream_id: "stream:refusal",
      lifecycle_generation: "sha256:refusal", target: "tls://example.com:443",
      principal_id: "person:local:refusal", network_mode: "runtime_net_only",
      direct_network: false, wallet_injection: false,
      display_mode: "webrtc_remote_display", guarantee_level: "mechanism_microvm",
    },
    requirements: {
      substrate: "microvm", display_mode: "webrtc_remote_display",
      backend_class: "product_compositor", network_mode: "runtime_net_only", direct_network: false,
    },
  });
  return new Promise((resolve, reject) => {
    const request = http.request({ socketPath: socket, path: "/pages", method: "POST",
      headers: { "content-type": "application/json", "content-length": Buffer.byteLength(body) },
    }, response => {
      const chunks = [];
      response.on("data", chunk => chunks.push(chunk));
      response.on("error", reject);
      response.on("end", () => {
        try { resolve({ status: response.statusCode, body: JSON.parse(Buffer.concat(chunks)) }); }
        catch (error) { reject(error); }
      });
    });
    request.setTimeout(5000, () => request.destroy(new Error("control service response timed out")));
    request.on("error", reject);
    request.end(body);
  });
}

test("control service response carries typed refusal code and counts from supervisor stderr", async t => {
  const response = await controlServiceResponse(t, typed);
  assert.equal(response.status, 400);
  assert.equal(response.body.code, "launch_refused");
  assert.equal(response.body.message, "Browser cannot open.");
  assert.deepEqual(response.body.refusal, refusal);
});


for (const [name, bad] of [
  ["unknown code", { ...refusal, code: "unknown" }],
  ["string count", { ...refusal, required_bytes: "8589934592" }],
  ["unsafe count", { ...refusal, required_bytes: 2 ** 53 }],
  ["negative count", { ...refusal, available_bytes: -1 }],
  ["equal counts", { ...refusal, available_bytes: refusal.required_bytes }],
  ["extra field", { ...refusal, message: "private" }],
]) {
  test(`control service response falls back for refusal with ${name}`, async t => {
    const response = await controlServiceResponse(t, { ...typed, refusal: bad });
    assert.equal(response.status, 400);
    assert.equal(response.body.code, "launch_refused");
    assert.equal(response.body.message, "Browser cannot open.");
    assert.equal(response.body.refusal, undefined);
  });
}
