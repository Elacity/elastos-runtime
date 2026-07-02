#!/usr/bin/env node
import childProcess from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const repoRoot = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "elastos-browser-runtime-turn-"));

try {
  const output = childProcess.execFileSync(
    process.execPath,
    [
      path.join(repoRoot, "scripts/browser-runtime-turn.mjs"),
      "--data-dir",
      tmpDir,
      "--host-ip",
      "10.44.0.10",
      "--media-host-ip",
      "10.44.0.1",
      "--media-guest-ip",
      "10.44.0.2",
      "--no-start",
    ],
    { encoding: "utf8" },
  );
  const result = JSON.parse(output);
  if (result.schema !== "elastos.browser.runtime-turn/v1") {
    throw new Error("wrong runtime TURN smoke schema");
  }
  if (result.running !== false) {
    throw new Error("write-only runtime TURN smoke must not start turnserver");
  }
  const env = fs.readFileSync(result.env_file, "utf8");
  const conf = fs.readFileSync(result.config_file, "utf8");
  if (!env.includes("turn:10.44.0.10:3478?transport=udp")) {
    throw new Error("runtime TURN env did not include host UDP relay URL");
  }
  if (!env.includes("turn:10.44.0.10:3478?transport=tcp")) {
    throw new Error("runtime TURN env did not include host TCP relay URL");
  }
  if (!env.includes("turn:10.44.0.1:3478?transport=udp")) {
    throw new Error("runtime TURN env did not include UDP relay URL");
  }
  if (!env.includes("turn:10.44.0.1:3478?transport=tcp")) {
    throw new Error("runtime TURN env did not include TCP relay URL");
  }
  if (env.includes("ELASTOS_BROWSER_VM_ICE_SERVER=")) {
    throw new Error("runtime TURN env must use only credentialed ICE_SERVERS_JSON");
  }
  if (!env.includes("ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY=relay")) {
    throw new Error("runtime TURN env did not force relay ICE policy");
  }
  if (!env.includes("ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4=10.44.0.1")) {
    throw new Error("runtime TURN env did not expose media relay host IPv4");
  }
  if (!env.includes("ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4=10.44.0.2")) {
    throw new Error("runtime TURN env did not expose media relay guest IPv4");
  }
  if (!env.includes("ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX=24")) {
    throw new Error("runtime TURN env did not expose media relay prefix");
  }
  if (!conf.includes("lt-cred-mech") || !conf.includes("relay-ip=10.44.0.10")) {
    throw new Error("runtime TURN config did not enable credentialed relay");
  }
  console.log(JSON.stringify({
    ok: true,
    schema: "elastos.browser.runtime-turn-smoke/v1",
    env_file: result.env_file,
    config_file: result.config_file,
  }));
} finally {
  fs.rmSync(tmpDir, { recursive: true, force: true });
}
