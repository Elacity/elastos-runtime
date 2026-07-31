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
