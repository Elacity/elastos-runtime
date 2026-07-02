#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import process from "node:process";

const DEFAULT_CARRIER_SERVICE = "elastos://exit/open_stream";
const BASE32_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const BASE58_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

function usage() {
  console.error(`Usage:
  node scripts/remote-carrier-exit-readiness.mjs \\
    --source-config /path/to/source/exit-provider.json \\
    --exit-config /path/to/exit/exit-provider.json \\
    --principal person:local:alice \\
    --grant-id operator-grant:server-exit:alice \\
    --target tls://example.com:443 \\
    [--exit-did did:elastos:server] \\
    [--allow-source-local-backends]

Checks whether an installed Browser source runtime is configured to use a
remote Carrier Exit grant for one target, and whether the remote exit runtime
has a private relay IPC stream backend for that target. The report is redacted:
it records whether private route material exists, never the material itself.
`);
}

function parseArgs(argv) {
  const args = {
    sourceConfig: "",
    exitConfig: "",
    principal: "",
    grantId: "",
    target: "",
    exitDid: "",
    allowSourceLocalBackends: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const next = () => {
      index += 1;
      if (index >= argv.length || argv[index].startsWith("--")) {
        throw new Error(`${arg} requires a value`);
      }
      return argv[index];
    };
    if (arg === "--help" || arg === "-h") {
      usage();
      process.exit(0);
    } else if (arg === "--source-config") {
      args.sourceConfig = next();
    } else if (arg === "--exit-config") {
      args.exitConfig = next();
    } else if (arg === "--principal") {
      args.principal = next();
    } else if (arg === "--grant-id") {
      args.grantId = next();
    } else if (arg === "--target") {
      args.target = next();
    } else if (arg === "--exit-did") {
      args.exitDid = next();
    } else if (arg === "--allow-source-local-backends") {
      args.allowSourceLocalBackends = true;
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }
  for (const [name, value] of Object.entries(args)) {
    if (name !== "exitDid" && name !== "allowSourceLocalBackends" && !nonEmpty(value)) {
      throw new Error(`--${name.replace(/[A-Z]/g, (ch) => `-${ch.toLowerCase()}`)} is required`);
    }
  }
  return args;
}

function nonEmpty(value) {
  return typeof value === "string" && value.trim() !== "";
}

function base32NoPadDecode(value) {
  const clean = String(value || "").trim().toUpperCase();
  if (!/^[A-Z2-7]+$/.test(clean)) {
    throw new Error("not base32");
  }
  let bits = 0;
  let bitCount = 0;
  const out = [];
  for (const ch of clean) {
    const n = BASE32_ALPHABET.indexOf(ch);
    if (n < 0) throw new Error("invalid base32");
    bits = (bits << 5) | n;
    bitCount += 5;
    while (bitCount >= 8) {
      bitCount -= 8;
      out.push((bits >> bitCount) & 0xff);
    }
  }
  return Buffer.from(out);
}

function base58Decode(value) {
  let n = 0n;
  for (const ch of String(value || "")) {
    const digit = BASE58_ALPHABET.indexOf(ch);
    if (digit < 0) throw new Error("invalid base58");
    n = n * 58n + BigInt(digit);
  }
  const bytes = [];
  while (n > 0n) {
    bytes.unshift(Number(n & 0xffn));
    n >>= 8n;
  }
  for (const ch of String(value || "")) {
    if (ch !== "1") break;
    bytes.unshift(0);
  }
  return Buffer.from(bytes);
}

function nodeIdFromPeer(value) {
  const peer = String(value || "").trim();
  if (/^[0-9a-fA-F]{64}$/.test(peer)) {
    return peer.toLowerCase();
  }
  const didKey = peer.match(/^did:key:(z[1-9A-HJ-NP-Za-km-z]+)$/);
  if (didKey) {
    const decoded = base58Decode(didKey[1].slice(1));
    if (decoded.length === 34 && decoded[0] === 0xed && decoded[1] === 0x01) {
      return decoded.slice(2).toString("hex");
    }
  }
  return "";
}

function connectTicketEndpointIds(ticket) {
  const decoded = base32NoPadDecode(ticket);
  const parsed = JSON.parse(decoded.toString("utf8"));
  const endpoints = Array.isArray(parsed.endpoints) ? parsed.endpoints : [];
  return endpoints
    .map((endpoint) => String(endpoint?.id || "").trim().toLowerCase())
    .filter((id) => /^[0-9a-f]{64}$/.test(id));
}

function readConfig(path) {
  const parsed = JSON.parse(fs.readFileSync(path, "utf8"));
  if (parsed && typeof parsed === "object" && parsed.extra && typeof parsed.extra === "object") {
    return parsed.extra;
  }
  return parsed;
}

function sha256File(path) {
  return crypto.createHash("sha256").update(fs.readFileSync(path)).digest("hex");
}

function normalizeHost(host) {
  return String(host || "").trim().replace(/^\[/, "").replace(/\]$/, "").toLowerCase();
}

function targetUrl(rawTarget) {
  let parsed;
  try {
    parsed = new URL(rawTarget);
  } catch {
    throw new Error("--target must be an absolute URL such as tls://example.com:443");
  }
  if (!["tcp:", "tls:"].includes(parsed.protocol)) {
    throw new Error("--target must use tcp:// or tls:// for Browser stream exit readiness");
  }
  if (!nonEmpty(parsed.hostname)) {
    throw new Error("--target requires a host");
  }
  if (!nonEmpty(parsed.port)) {
    throw new Error("--target requires an explicit port");
  }
  return parsed;
}

function hostAllowed(allowedHosts, host) {
  const normalized = normalizeHost(host);
  return Array.isArray(allowedHosts) && allowedHosts.some((allowedRaw) => {
    const allowed = normalizeHost(allowedRaw);
    if (allowed === "*") {
      return true;
    }
    if (allowed.startsWith("*.")) {
      return normalized.endsWith(`.${allowed.slice(2)}`);
    }
    return normalized === allowed;
  });
}

function schemeAllowed(allowedSchemes, scheme, defaultSchemes) {
  const normalized = String(scheme || "").replace(/:$/, "");
  const schemes = Array.isArray(allowedSchemes) && allowedSchemes.length > 0
    ? allowedSchemes
    : defaultSchemes;
  return schemes.includes(normalized);
}

function portAllowed(allowedPorts, port) {
  const parsed = Number(port);
  return Number.isInteger(parsed)
    && parsed > 0
    && parsed <= 65535
    && (!Array.isArray(allowedPorts) || allowedPorts.length === 0 || allowedPorts.includes(parsed));
}

function principalAllowed(allowedPrincipals, principal) {
  return Array.isArray(allowedPrincipals)
    && allowedPrincipals.some((allowed) => allowed === "*" || allowed === principal);
}

function looksPrivateIp(host) {
  const normalized = normalizeHost(host);
  const version = net.isIP(normalized);
  if (version === 0) {
    return false;
  }
  if (version === 4) {
    const parts = normalized.split(".").map((part) => Number(part));
    return parts[0] === 10
      || parts[0] === 127
      || (parts[0] === 172 && parts[1] >= 16 && parts[1] <= 31)
      || (parts[0] === 192 && parts[1] === 168)
      || (parts[0] === 169 && parts[1] === 254);
  }
  return normalized === "::1"
    || normalized.startsWith("fc")
    || normalized.startsWith("fd")
    || normalized.startsWith("fe80:");
}

function nowSeconds() {
  return Math.floor(Date.now() / 1000);
}

function remoteExitAllowsTarget(exit, target) {
  return hostAllowed(exit.allowed_hosts, target.hostname)
    && schemeAllowed(exit.allowed_schemes, target.protocol, ["tcp", "tls"])
    && portAllowed(exit.allowed_ports, target.port);
}

function backendAllowsTarget(backend, target) {
  return hostAllowed(backend.allowed_hosts, target.hostname)
    && schemeAllowed(backend.allowed_schemes, target.protocol, ["tcp", "tls"])
    && portAllowed(backend.allowed_ports, target.port);
}

function publicRemoteExit(exit, ticketProof = null) {
  if (!exit) {
    return null;
  }
  return {
    id: exit.id || null,
    grant_id: exit.grant_id || null,
    peer_did: exit.peer_did || null,
    carrier_service: exit.carrier_service || DEFAULT_CARRIER_SERVICE,
    allowed_hosts: Array.isArray(exit.allowed_hosts) ? exit.allowed_hosts : [],
    allowed_schemes: Array.isArray(exit.allowed_schemes) ? exit.allowed_schemes : [],
    allowed_ports: Array.isArray(exit.allowed_ports) ? exit.allowed_ports : [],
    max_active_streams: exit.max_active_streams ?? 8,
    max_active_streams_per_principal: exit.max_active_streams_per_principal ?? exit.max_active_streams ?? 8,
    expires_at: exit.expires_at ?? null,
    connect_ticket_present: nonEmpty(exit.connect_ticket),
    connect_ticket_endpoint_count: ticketProof?.endpoint_count ?? null,
    connect_ticket_peer_match: ticketProof?.peer_match ?? null,
  };
}

function publicBackend(backend) {
  if (!backend) {
    return null;
  }
  return {
    id: backend.id || null,
    kind: backend.kind || null,
    allowed_hosts: Array.isArray(backend.allowed_hosts) ? backend.allowed_hosts : [],
    allowed_schemes: Array.isArray(backend.allowed_schemes) ? backend.allowed_schemes : [],
    allowed_ports: Array.isArray(backend.allowed_ports) ? backend.allowed_ports : [],
    allow_private_targets: backend.allow_private_targets === true,
    adapter_ipc_present: backend.adapter_ipc?.kind === "unix_socket" && nonEmpty(backend.adapter_ipc?.path),
    relay_ipc_present: backend.relay_ipc?.kind === "unix_socket" && nonEmpty(backend.relay_ipc?.path),
  };
}

function analyze(args) {
  const target = targetUrl(args.target);
  const sourceConfigSha256 = sha256File(args.sourceConfig);
  const exitConfigSha256 = sha256File(args.exitConfig);
  const source = readConfig(args.sourceConfig);
  const exit = readConfig(args.exitConfig);
  const failures = [];
  const sourceBackends = Array.isArray(source.backends) ? source.backends : [];
  const sourceRemoteExits = Array.isArray(source.remote_carrier_exits) ? source.remote_carrier_exits : [];
  const exitBackends = Array.isArray(exit.backends) ? exit.backends : [];
  const selectedRemoteExit = sourceRemoteExits.find((candidate) => candidate?.grant_id === args.grantId) || null;
  const selectedExitBackend = exitBackends.find((backend) =>
    backend?.kind === "stream_relay" && backendAllowsTarget(backend, target)
  ) || null;
  let ticketProof = null;

  if (sourceBackends.length > 0 && !args.allowSourceLocalBackends) {
    failures.push("source_config_must_not_keep_local_exit_backends_for_remote_carrier_acceptance");
  }
  if (sourceRemoteExits.length === 0) {
    failures.push("source_config_has_no_remote_carrier_exits");
  }
  if (!selectedRemoteExit) {
    failures.push("source_config_missing_requested_grant_id");
  }
  if (selectedRemoteExit) {
    if (!nonEmpty(selectedRemoteExit.id)) {
      failures.push("source_remote_exit_id_required");
    }
    if (!nonEmpty(selectedRemoteExit.peer_did)) {
      failures.push("source_remote_exit_peer_did_required");
    }
    const sourcePeerNodeId = nodeIdFromPeer(selectedRemoteExit.peer_did);
    const exitNodeId = nodeIdFromPeer(args.exitDid);
    if (
      nonEmpty(args.exitDid) &&
      selectedRemoteExit.peer_did !== args.exitDid &&
      (!sourcePeerNodeId || !exitNodeId || sourcePeerNodeId !== exitNodeId)
    ) {
      failures.push("source_remote_exit_peer_did_must_match_exit_runtime");
    }
    if ((selectedRemoteExit.carrier_service || DEFAULT_CARRIER_SERVICE) !== DEFAULT_CARRIER_SERVICE) {
      failures.push("source_remote_exit_carrier_service_must_be_elastos_exit_open_stream");
    }
    if (!nonEmpty(selectedRemoteExit.connect_ticket)) {
      failures.push("source_remote_exit_connect_ticket_required_but_redacted_from_reports");
    } else {
      try {
        const endpointIds = connectTicketEndpointIds(selectedRemoteExit.connect_ticket);
        const peerMatch = Boolean(sourcePeerNodeId && endpointIds.includes(sourcePeerNodeId));
        ticketProof = {
          endpoint_count: endpointIds.length,
          peer_match: peerMatch,
        };
        if (endpointIds.length === 0) {
          failures.push("source_remote_exit_connect_ticket_has_no_endpoints");
        }
        if (sourcePeerNodeId && !peerMatch) {
          failures.push("source_remote_exit_peer_did_must_match_connect_ticket");
        }
      } catch {
        failures.push("source_remote_exit_connect_ticket_must_decode");
      }
    }
    if (!principalAllowed(selectedRemoteExit.allowed_principals, args.principal)) {
      failures.push("source_remote_exit_principal_not_allowed");
    }
    if (!remoteExitAllowsTarget(selectedRemoteExit, target)) {
      failures.push("source_remote_exit_target_policy_does_not_allow_target");
    }
    if (selectedRemoteExit.expires_at != null && Number(selectedRemoteExit.expires_at) <= nowSeconds()) {
      failures.push("source_remote_exit_grant_expired");
    }
    if (Number(selectedRemoteExit.max_active_streams ?? 8) < 1) {
      failures.push("source_remote_exit_max_active_streams_must_be_positive");
    }
    if (selectedRemoteExit.max_active_streams_per_principal != null && Number(selectedRemoteExit.max_active_streams_per_principal) < 1) {
      failures.push("source_remote_exit_principal_quota_must_be_positive");
    }
  }
  if (looksPrivateIp(target.hostname)) {
    failures.push("target_must_not_be_private_or_loopback_for_remote_browser_exit_acceptance");
  }
  if (!selectedExitBackend) {
    failures.push("exit_config_missing_stream_relay_backend_for_target");
  }
  if (selectedExitBackend) {
    const publicBackendShape = publicBackend(selectedExitBackend);
    if (publicBackendShape.allow_private_targets) {
      failures.push("exit_backend_must_not_enable_private_targets_for_public_browser_exit");
    }
    if (!publicBackendShape.adapter_ipc_present) {
      failures.push("exit_backend_adapter_ipc_required_for_runtime_bridge");
    }
    if (!publicBackendShape.relay_ipc_present) {
      failures.push("exit_backend_relay_ipc_required_for_remote_carrier_handoff");
    }
  }

  return {
    schema: "elastos.remote-carrier-exit.readiness/v1",
    ok: failures.length === 0,
    generated_at: new Date().toISOString(),
    route: {
      principal: args.principal,
      grant_id: args.grantId,
      target: target.href,
      byte_transport: "carrier_stream",
      carrier_service: DEFAULT_CARRIER_SERVICE,
    },
    source: {
      config_path: args.sourceConfig,
      config_sha256: sourceConfigSha256,
      local_backend_count: sourceBackends.length,
      remote_carrier_exit_count: sourceRemoteExits.length,
      remote_only: sourceBackends.length === 0,
      local_backends_allowed: args.allowSourceLocalBackends,
      selected_remote_exit: publicRemoteExit(selectedRemoteExit, ticketProof),
    },
    exit: {
      config_path: args.exitConfig,
      config_sha256: exitConfigSha256,
      stream_relay_backend_count: exitBackends.filter((backend) => backend?.kind === "stream_relay").length,
      selected_stream_relay_backend: publicBackend(selectedExitBackend),
    },
    failures,
    next_steps: failures.length === 0 ? [
      "Run a real Browser open through the reviewed grant and collect redacted source/exit gateway logs.",
      "Validate the operator-filled evidence with node scripts/remote-carrier-exit-operator-report.mjs --input <evidence.json>.",
    ] : [
      "Configure the Browser source exit-provider with a matching remote_carrier_exits grant and no source-side local exit backend for this acceptance lane.",
      "Configure the remote exit runtime with a stream_relay backend for the target and private adapter_ipc plus relay_ipc descriptors.",
      "Rerun this readiness check before collecting operator evidence.",
    ],
  };
}

try {
  const args = parseArgs(process.argv.slice(2));
  const result = analyze(args);
  console.log(JSON.stringify(result, null, 2));
  if (!result.ok) {
    process.exit(1);
  }
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  usage();
  process.exit(2);
}
