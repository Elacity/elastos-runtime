export function stripTrickleCandidatesFromSdp(sdp) {
  return String(sdp || "")
    .split(/\r?\n/)
    .filter((line) => line !== "" && !line.startsWith("a=candidate:") && line !== "a=end-of-candidates")
    .join("\r\n")
    .concat("\r\n");
}

export function normalizeIceCandidateForRuntime(candidate) {
  if (!candidate || typeof candidate !== "object") {
    return null;
  }
  const normalized = { ...candidate };
  const line = String(normalized.candidate || "").trim();
  if (!line) {
    return null;
  }
  const tokens = line.split(/\s+/);
  if (tokens.length >= 2) {
    const filtered = [];
    for (let index = 0; index < tokens.length; index += 1) {
      const token = tokens[index];
      const key = token.toLowerCase();
      if ((key === "network-id" || key === "network-cost") && index + 1 < tokens.length) {
        index += 1;
        continue;
      }
      filtered.push(token);
    }
    normalized.candidate = filtered.join(" ");
  } else {
    normalized.candidate = line;
  }
  if (typeof normalized.sdpMid === "string") {
    const value = normalized.sdpMid.trim();
    normalized.sdpMid = value || undefined;
  }
  if (!Number.isInteger(normalized.sdpMLineIndex) || normalized.sdpMLineIndex < 0) {
    if (!normalized.sdpMid) {
      normalized.sdpMLineIndex = 0;
    } else {
      delete normalized.sdpMLineIndex;
    }
  }
  return normalized;
}

export function iceCandidateType(candidate) {
  const line = String(candidate?.candidate || candidate || "")
    .trim()
    .replace(/^a=/, "");
  const tokens = line.split(/\s+/);
  const typeIndex = tokens.findIndex(
    (token) => token.toLowerCase() === "typ",
  );
  return typeIndex >= 0 && typeIndex + 1 < tokens.length
    ? tokens[typeIndex + 1].toLowerCase()
    : "";
}

export function sdpHasOnlyRelayCandidates(sdp) {
  return String(sdp || "")
    .split(/\r?\n/)
    .filter((line) => /^a=candidate:/i.test(line.trim()))
    .every((line) => iceCandidateType(line) === "relay");
}

export function normalizeDisplayIceServers(value) {
  if (!Array.isArray(value)) {
    return [];
  }
  const normalized = [];
  for (const item of value) {
    if (!item || typeof item !== "object") {
      continue;
    }
    const urls = Array.isArray(item.urls) ? item.urls : [item.urls];
    const filteredUrls = urls
      .filter((entry) => typeof entry === "string")
      .map((entry) => entry.trim())
      .filter((entry) => entry.startsWith("stun:") || entry.startsWith("turn:") || entry.startsWith("turns:"));
    if (filteredUrls.length === 0) {
      continue;
    }
    const server = { urls: filteredUrls };
    if (typeof item.username === "string" && item.username.trim() !== "") {
      server.username = item.username.trim();
    }
    if (typeof item.credential === "string" && item.credential !== "") {
      server.credential = item.credential;
    }
    normalized.push(server);
  }
  return normalized;
}

function exactObjectKeys(value, expected) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }
  const keys = Object.keys(value).sort();
  return (
    keys.length === expected.length &&
    expected.slice().sort().every((key, index) => keys[index] === key)
  );
}

function isSha256Label(value) {
  return /^sha256:[0-9a-f]{64}$/i.test(String(value || ""));
}

function turnHost(host) {
  const value = String(host || "");
  return value.includes(":") && !value.startsWith("[") ? `[${value}]` : value;
}

async function sha256Label(value) {
  if (!globalThis.crypto?.subtle || typeof TextEncoder !== "function") {
    throw new Error("Browser Runtime TURN verification is unavailable.");
  }
  const digest = await globalThis.crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(value),
  );
  return `sha256:${Array.from(new Uint8Array(digest), (byte) =>
    byte.toString(16).padStart(2, "0")).join("")}`;
}

export async function validateRuntimeLaunchTurn(
  displaySession,
  enginePage,
  nowUnixMs = Date.now(),
) {
  const capability = displaySession?.runtime_turn;
  const proof = enginePage?.transport_proof;
  const capabilityKeys = [
    "schema",
    "binding_hash",
    "generation",
    "page_id",
    "vm_id",
    "egress_stream_id",
    "media_stream_id",
    "expires_at_unix_ms",
    "credential_hash",
    "turn_url",
  ];
  if (
    displaySession?.ice_connection_policy !== "runtime_launch_relay_only" ||
    displaySession?.offerer !== "engine" ||
    displaySession?.media_transport !== "runtime_relay" ||
    capability?.schema !== "elastos.browser.vz-viewer-turn-capability/v1" ||
    !exactObjectKeys(capability, capabilityKeys) ||
    proof?.schema !== "elastos.browser.vz-transport-public-proof/v1" ||
    capability.binding_hash !== proof.binding_hash ||
    capability.generation !== proof.generation ||
    capability.page_id !== proof.page_id ||
    capability.vm_id !== proof.vm_id ||
    capability.egress_stream_id !== proof.egress?.stream_id ||
    capability.media_stream_id !== proof.media?.stream_id ||
    capability.expires_at_unix_ms !== proof.expires_at_unix_ms ||
    capability.credential_hash !== proof.credential_hash ||
    capability.page_id !== enginePage?.page_id ||
    capability.egress_stream_id !== enginePage?.stream_id ||
    !isSha256Label(capability.binding_hash) ||
    !isSha256Label(capability.generation) ||
    !isSha256Label(capability.credential_hash) ||
    !Number.isSafeInteger(nowUnixMs) ||
    !Number.isSafeInteger(capability.expires_at_unix_ms) ||
    capability.expires_at_unix_ms <= nowUnixMs ||
    capability.expires_at_unix_ms > nowUnixMs + 24 * 60 * 60 * 1000 ||
    proof?.effects?.turn_launch_owned !== true ||
    proof?.effects?.turn_listener_loopback !== true ||
    !Array.isArray(proof?.turn?.protocols) ||
    proof.turn.protocols.length !== 2 ||
    proof.turn.protocols[0] !== "turn" ||
    proof.turn.protocols[1] !== "tcp" ||
    !Number.isInteger(proof?.turn?.listen_port) ||
    proof.turn.listen_port < 1 ||
    proof.turn.listen_port > 65535
  ) {
    throw new Error("Browser Runtime TURN binding is invalid.");
  }
  const expectedTurnUrl =
    `turn:${turnHost(proof.turn.advertised_host)}:${proof.turn.listen_port}?transport=tcp`;
  if (capability.turn_url !== expectedTurnUrl) {
    throw new Error("Browser Runtime TURN endpoint is invalid.");
  }
  if (
    !Array.isArray(displaySession.ice_servers) ||
    displaySession.ice_servers.length !== 1 ||
    !exactObjectKeys(displaySession.ice_servers[0], [
      "urls",
      "username",
      "credential",
    ])
  ) {
    throw new Error("Browser Runtime TURN server is invalid.");
  }
  const server = displaySession.ice_servers[0];
  if (
    !Array.isArray(server.urls) ||
    server.urls.length !== 1 ||
    server.urls[0] !== capability.turn_url ||
    typeof server.username !== "string" ||
    !server.username ||
    typeof server.credential !== "string" ||
    !server.credential ||
    server.credential.length > 512 ||
    server.username.split(":", 1)[0] !==
      String(Math.floor(capability.expires_at_unix_ms / 1000)) ||
    await sha256Label(server.credential) !== capability.credential_hash
  ) {
    throw new Error("Browser Runtime TURN credential is invalid.");
  }
  const iceServers = normalizeDisplayIceServers(displaySession.ice_servers);
  if (
    iceServers.length !== 1 ||
    iceServers[0].urls.length !== 1 ||
    iceServers[0].username !== server.username ||
    iceServers[0].credential !== server.credential
  ) {
    throw new Error("Browser Runtime TURN projection is invalid.");
  }
  return iceServers;
}

export function normalizeEngineCandidate(candidate) {
  if (!candidate || typeof candidate !== "object") {
    return null;
  }
  const normalized = { ...candidate };
  const line = String(normalized.candidate || "").trim();
  if (!line) {
    return null;
  }
  normalized.candidate = line.startsWith("a=") ? line.slice(2) : line;
  if (typeof normalized.sdpMid === "string") {
    const value = normalized.sdpMid.trim();
    normalized.sdpMid = value || undefined;
  } else {
    delete normalized.sdpMid;
  }
  if (!Number.isInteger(normalized.sdpMLineIndex) || normalized.sdpMLineIndex < 0) {
    if (!normalized.sdpMid) {
      normalized.sdpMLineIndex = 0;
    } else {
      delete normalized.sdpMLineIndex;
    }
  }
  if (typeof normalized.usernameFragment === "string") {
    const value = normalized.usernameFragment.trim();
    if (value) {
      normalized.usernameFragment = value;
    } else {
      delete normalized.usernameFragment;
    }
  }
  return normalized;
}
