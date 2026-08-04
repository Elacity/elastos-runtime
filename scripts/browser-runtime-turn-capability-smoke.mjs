#!/usr/bin/env node

import assert from "node:assert/strict";
import { createHash, webcrypto } from "node:crypto";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

if (!globalThis.crypto) {
  globalThis.crypto = webcrypto;
}

const scriptRoot = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptRoot, "..");
const {
  validateRuntimeLaunchTurn,
} = await import(
  path.join(repoRoot, "capsules/browser/browser/browser-webrtc.js")
);

const now = 1_785_423_600_000;
const expiresAt = now + 5 * 60_000;
const credential = "ephemeral-viewer-credential";
const sha256 = (value) =>
  `sha256:${createHash("sha256").update(value).digest("hex")}`;
const generation =
  "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const bindingHash =
  "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const credentialHash = sha256(credential);
const pageId = "page:vz-runtime-turn-smoke";
const vmId = "browser-vm-runtime-turn-smoke";
const egressStreamId = "stream:runtime-turn-egress";
const mediaStreamId = "stream:runtime-turn-media";
const turnUrl = "turn:127.0.0.1:49186?transport=tcp";

const transportProof = {
  schema: "elastos.browser.vz-transport-public-proof/v1",
  binding_hash: bindingHash,
  generation,
  page_id: pageId,
  vm_id: vmId,
  expires_at_unix_ms: expiresAt,
  credential_hash: credentialHash,
  egress: { stream_id: egressStreamId },
  media: { stream_id: mediaStreamId },
  turn: {
    advertised_host: "127.0.0.1",
    listen_port: 49186,
    protocols: ["turn", "tcp"],
  },
  effects: {
    turn_launch_owned: true,
    turn_listener_loopback: true,
  },
};
const runtimeTurn = {
  schema: "elastos.browser.vz-viewer-turn-capability/v1",
  binding_hash: bindingHash,
  generation,
  page_id: pageId,
  vm_id: vmId,
  egress_stream_id: egressStreamId,
  media_stream_id: mediaStreamId,
  expires_at_unix_ms: expiresAt,
  credential_hash: credentialHash,
  turn_url: turnUrl,
};
const displaySession = {
  schema: "elastos.browser.display-session/v1",
  mode: "webrtc_remote_display",
  offerer: "engine",
  media_transport: "runtime_relay",
  ice_connection_policy: "runtime_launch_relay_only",
  runtime_turn: runtimeTurn,
  ice_servers: [{
    urls: [turnUrl],
    username: `${Math.floor(expiresAt / 1000)}:aaaaaaaaaaaaaaaaaaaaaaaa`,
    credential,
  }],
};
const enginePage = {
  schema: "elastos.browser.engine.page/v1",
  page_id: pageId,
  stream_id: egressStreamId,
  transport_proof: transportProof,
};

const validated = await validateRuntimeLaunchTurn(
  displaySession,
  enginePage,
  now,
);
assert.deepEqual(validated, displaySession.ice_servers);

for (const mutate of [
  (session) => {
    session.runtime_turn.page_id = "page:vz-substituted";
  },
  (session) => {
    session.runtime_turn.vm_id = "browser-vm-substituted";
  },
  (session) => {
    session.runtime_turn.media_stream_id = "stream:substituted";
  },
  (session) => {
    session.runtime_turn.binding_hash =
      "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
  },
  (session) => {
    session.ice_servers[0].credential = "substituted";
  },
  (session) => {
    session.ice_servers[0].urls = ["turn:127.0.0.1:49187?transport=tcp"];
  },
]) {
  const substituted = structuredClone(displaySession);
  mutate(substituted);
  await assert.rejects(
    validateRuntimeLaunchTurn(substituted, enginePage, now),
    /Browser Runtime TURN/,
  );
}

await assert.rejects(
  validateRuntimeLaunchTurn(displaySession, enginePage, expiresAt),
  /Browser Runtime TURN/,
);

const browserMain = await readFile(
  path.join(repoRoot, "capsules/browser/browser/browser.js"),
  "utf8",
);
const remoteDisplay = await readFile(
  path.join(repoRoot, "capsules/browser/browser/browser-remote-display.js"),
  "utf8",
);
const productProof = await readFile(
  path.join(repoRoot, "scripts/home-passkey-virtual-auth-smoke.mjs"),
  "utf8",
);
assert.match(
  browserMain,
  /window\.__elastosBrowserCurrentPageId = page\?\.page_id \|\| "";/,
);
assert.doesNotMatch(browserMain, /localStorage|sessionStorage/);
assert.match(
  productProof,
  /\["credential", "auth_secret", "transport_secret"\]\.includes\(key\)/,
);
assert.match(
  productProof,
  /JSON\.stringify\(redactSensitive\(error\.details\), null, 2\)/,
);
assert.match(
  remoteDisplay,
  /displaySession\.ice_connection_policy === "runtime_launch_relay_only"/,
);
assert.match(
  remoteDisplay,
  /runtimeLaunchRelayOnly \|\|\s*\(displaySession\.media_transport === "runtime_relay"/,
);
assert.match(
  remoteDisplay,
  /iceCandidateType\(normalized\) !== "relay"/,
);

console.log(JSON.stringify({
  schema: "elastos.browser.runtime-turn-capability-smoke/v1",
  positive: true,
  substitution_rejected: true,
  expiry_rejected: true,
  credential_hash_verified: true,
  home_persistence_absent: true,
  relay_only_peers: true,
}));
