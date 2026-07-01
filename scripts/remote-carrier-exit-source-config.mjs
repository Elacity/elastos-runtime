#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const DEFAULT_CARRIER_SERVICE = "elastos://exit/open_stream";

function usage() {
  console.error(`Usage:
  node scripts/remote-carrier-exit-source-config.mjs \\
    --source-config /path/to/source/exit-provider.json \\
    --exit-config /path/to/remote/exit-provider.json \\
    --exit-ticket-file /path/to/private-ticket.txt-or-bootstrap.json \\
    --exit-peer-did <did:key-or-iroh-node-id> \\
    --principal person:local:alice \\
    --grant-id operator-grant:server-exit:alice \\
    --target tls://ela.city:443 \\
    --candidate-config /tmp/source-exit-provider.remote.json \\
    [--remote-exit-id seed-node] \\
    [--keep-local-backends] \\
    [--allowed-host ela.city] \\
    [--allowed-scheme tls] \\
    [--allowed-port 443] \\
    [--max-active-streams 2] \\
    [--max-active-streams-per-principal 1] \\
    [--receipt-out /tmp/readiness-receipt.json] \\
    [--install]

Builds a remote-only source exit-provider config for one Browser source runtime
and validates it with remote-carrier-exit-readiness.mjs. By default it writes a
candidate config only. With --install it backs up and replaces --source-config.
Use --keep-local-backends for product/runtime settings where Local Runtime exit
and one or more remote seed exits must be selectable side by side.
The receipt is redacted and never prints the private connect_ticket.
`);
}

function parseArgs(argv) {
  const args = {
    sourceConfig: "",
    exitConfig: "",
    exitTicketFile: "",
    exitPeerDid: "",
    principal: "",
    grantId: "",
    target: "",
    candidateConfig: "",
    remoteExitId: "remote-carrier-exit",
    receiptOut: "",
    allowedHosts: [],
    allowedSchemes: [],
    allowedPorts: [],
    maxActiveStreams: 2,
    maxActiveStreamsPerPrincipal: 1,
    keepLocalBackends: false,
    install: false,
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
    } else if (arg === "--exit-ticket-file") {
      args.exitTicketFile = next();
    } else if (arg === "--exit-peer-did") {
      args.exitPeerDid = next();
    } else if (arg === "--principal") {
      args.principal = next();
    } else if (arg === "--grant-id") {
      args.grantId = next();
    } else if (arg === "--target") {
      args.target = next();
    } else if (arg === "--candidate-config") {
      args.candidateConfig = next();
    } else if (arg === "--remote-exit-id") {
      args.remoteExitId = validateSafeId(next(), "--remote-exit-id");
    } else if (arg === "--receipt-out") {
      args.receiptOut = next();
    } else if (arg === "--allowed-host") {
      args.allowedHosts.push(next());
    } else if (arg === "--allowed-scheme") {
      args.allowedSchemes.push(validateAllowedScheme(next()));
    } else if (arg === "--allowed-port") {
      args.allowedPorts.push(parseTcpPort(next(), "--allowed-port"));
    } else if (arg === "--max-active-streams") {
      args.maxActiveStreams = parsePositiveInt(next(), "--max-active-streams");
    } else if (arg === "--max-active-streams-per-principal") {
      args.maxActiveStreamsPerPrincipal = parsePositiveInt(next(), "--max-active-streams-per-principal");
    } else if (arg === "--keep-local-backends") {
      args.keepLocalBackends = true;
    } else if (arg === "--install") {
      args.install = true;
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }
  for (const [name, value] of Object.entries({
    sourceConfig: args.sourceConfig,
    exitConfig: args.exitConfig,
    exitTicketFile: args.exitTicketFile,
    exitPeerDid: args.exitPeerDid,
    principal: args.principal,
    grantId: args.grantId,
    target: args.target,
    candidateConfig: args.candidateConfig,
  })) {
    if (!nonEmpty(value)) {
      throw new Error(`--${name.replace(/[A-Z]/g, (ch) => `-${ch.toLowerCase()}`)} is required`);
    }
  }
  return args;
}

function validateSafeId(value, label) {
  const text = String(value || "").trim();
  if (!text || text.length > 128 || !/^[A-Za-z0-9:_-]+$/.test(text)) {
    throw new Error(`${label} must be a safe identifier up to 128 bytes`);
  }
  return text;
}

function parsePositiveInt(value, label) {
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < 1) {
    throw new Error(`${label} must be a positive integer`);
  }
  return parsed;
}

function parseTcpPort(value, label) {
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < 1 || parsed > 65535) {
    throw new Error(`${label} must be a TCP port between 1 and 65535`);
  }
  return parsed;
}

function validateAllowedScheme(value) {
  const scheme = String(value || "").trim().replace(/:$/, "");
  if (!["tcp", "tls"].includes(scheme)) {
    throw new Error("--allowed-scheme must be tcp or tls");
  }
  return scheme;
}

function nonEmpty(value) {
  return typeof value === "string" && value.trim() !== "";
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function readTicket(file) {
  const text = fs.readFileSync(file, "utf8").trim();
  let ticket = text;
  if (text.startsWith("{")) {
    const parsed = JSON.parse(text);
    ticket = String(parsed.ticket || parsed.connect_ticket || "").trim();
  }
  if (!nonEmpty(ticket)) {
    throw new Error("--exit-ticket-file must contain a non-empty Carrier connect ticket or bootstrap JSON ticket");
  }
  return ticket;
}

function sha256Text(text) {
  return crypto.createHash("sha256").update(text).digest("hex");
}

function targetUrl(rawTarget) {
  let parsed;
  try {
    parsed = new URL(rawTarget);
  } catch {
    throw new Error("--target must be an absolute URL such as tls://ela.city:443");
  }
  if (!["tcp:", "tls:"].includes(parsed.protocol)) {
    throw new Error("--target must use tcp:// or tls://");
  }
  if (!nonEmpty(parsed.hostname) || !nonEmpty(parsed.port)) {
    throw new Error("--target requires host and explicit port");
  }
  return parsed;
}

function writeJson(file, value, mode = 0o600) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`, { mode });
  try {
    fs.chmodSync(file, mode);
  } catch {
    // Best-effort on filesystems that do not expose POSIX modes.
  }
}

function buildConfig(args, sourceConfig, ticket) {
  const target = targetUrl(args.target);
  const host = target.hostname;
  const scheme = target.protocol.replace(/:$/, "");
  const port = Number(target.port);
  const allowedHosts = args.allowedHosts.length > 0 ? args.allowedHosts : [host];
  const allowedSchemes = args.allowedSchemes.length > 0 ? [...new Set(args.allowedSchemes)] : [scheme];
  const allowedPorts = args.allowedPorts.length > 0 ? [...new Set(args.allowedPorts)] : [port];
  const priorRemoteExits = Array.isArray(sourceConfig.remote_carrier_exits)
    ? sourceConfig.remote_carrier_exits.filter((exit) =>
        exit?.id !== args.remoteExitId && exit?.grant_id !== args.grantId)
    : [];
  const remoteExit = {
    id: args.remoteExitId,
    grant_id: args.grantId,
    peer_did: args.exitPeerDid,
    carrier_service: DEFAULT_CARRIER_SERVICE,
    connect_ticket: ticket,
    allowed_principals: [args.principal],
    allowed_hosts: allowedHosts,
    allowed_schemes: allowedSchemes,
    allowed_ports: allowedPorts,
    max_active_streams: args.maxActiveStreams,
    max_active_streams_per_principal: args.maxActiveStreamsPerPrincipal,
  };
  return {
    timeout_secs: sourceConfig.timeout_secs ?? 10,
    backends: args.keepLocalBackends && Array.isArray(sourceConfig.backends)
      ? sourceConfig.backends
      : [],
    remote_carrier_exits: args.keepLocalBackends
      ? [...priorRemoteExits, remoteExit]
      : [remoteExit],
  };
}

function runReadiness(args, candidatePath) {
  const scriptDir = path.dirname(fileURLToPath(import.meta.url));
  const readinessScript = path.join(scriptDir, "remote-carrier-exit-readiness.mjs");
  const readinessArgs = [
    readinessScript,
    "--source-config",
    candidatePath,
    "--exit-config",
    args.exitConfig,
    "--principal",
    args.principal,
    "--grant-id",
    args.grantId,
    "--target",
    args.target,
    "--exit-did",
    args.exitPeerDid,
  ];
  if (args.keepLocalBackends) {
    readinessArgs.push("--allow-source-local-backends");
  }
  const child = spawnSync(process.execPath, readinessArgs, {
    encoding: "utf8",
  });
  let report = null;
  try {
    report = JSON.parse(child.stdout);
  } catch {
    report = {
      schema: "elastos.remote-carrier-exit.readiness/v1",
      ok: false,
      failures: ["readiness_report_parse_failed"],
      stderr: child.stderr,
    };
  }
  return {
    status: child.status,
    report,
    stderr: child.stderr,
  };
}

function backupPath(sourceConfigPath) {
  const timestamp = new Date().toISOString().replace(/[-:]/g, "").replace(/\..+/, "Z");
  return `${sourceConfigPath}.backup-remote-carrier-${timestamp}`;
}

function installConfig(sourceConfigPath, candidateConfigPath) {
  const backup = backupPath(sourceConfigPath);
  fs.copyFileSync(sourceConfigPath, backup);
  fs.copyFileSync(candidateConfigPath, sourceConfigPath);
  try {
    fs.chmodSync(sourceConfigPath, 0o600);
  } catch {
    // Best-effort.
  }
  return backup;
}

function redactedReceipt(args, ticket, candidatePath, readiness, installed, backup) {
  const report = readiness.report || {};
  return {
    schema: "elastos.remote-carrier-exit.source-config/v1",
    ok: report.ok === true && (args.install ? installed === true : true),
    generated_at: new Date().toISOString(),
    installed,
    source_config: args.sourceConfig,
    candidate_config: candidatePath,
    backup_config: backup || null,
    route: {
      principal: args.principal,
      grant_id: args.grantId,
      target: targetUrl(args.target).href,
      byte_transport: "carrier_stream",
      carrier_service: DEFAULT_CARRIER_SERVICE,
    },
    remote_exit: {
      id: args.remoteExitId,
      peer_did: args.exitPeerDid,
      connect_ticket_present: true,
      connect_ticket_sha256: sha256Text(ticket),
    },
    readiness: {
      ok: report.ok === true,
      failures: Array.isArray(report.failures) ? report.failures : [],
      source_config_sha256: report.source?.config_sha256 || null,
      exit_config_sha256: report.exit?.config_sha256 || null,
      source_remote_only: report.source?.remote_only === true,
      source_local_backends_allowed: report.source?.local_backends_allowed === true,
      source_local_backend_count: report.source?.local_backend_count ?? null,
      exit_relay_ipc_present: report.exit?.selected_stream_relay_backend?.relay_ipc_present === true,
      exit_adapter_ipc_present: report.exit?.selected_stream_relay_backend?.adapter_ipc_present === true,
    },
    next_steps: report.ok === true
      ? [
          args.install
            ? `Restart the source runtime/gateway so exit-provider reloads the ${args.keepLocalBackends ? "local-plus-remote" : "remote-only"} config.`
            : `Review the candidate config, then rerun with --install${args.keepLocalBackends ? " --keep-local-backends" : ""} to replace the source exit-provider config.`,
          "Run scripts/remote-carrier-exit-readiness.mjs against the installed source and exit configs.",
          "Open Browser through the remote grant and collect operator evidence.",
        ]
      : [
          "Fix the readiness failures before installing this source config.",
        ],
  };
}

try {
  const args = parseArgs(process.argv.slice(2));
  const sourceConfig = readJson(args.sourceConfig);
  const ticket = readTicket(args.exitTicketFile);
  const candidatePath = path.resolve(args.candidateConfig);
  const candidate = buildConfig(args, sourceConfig, ticket);
  writeJson(candidatePath, candidate);
  const readiness = runReadiness(args, candidatePath);
  if (readiness.report?.ok !== true) {
    const receipt = redactedReceipt(args, ticket, candidatePath, readiness, false, null);
    if (args.receiptOut) {
      writeJson(path.resolve(args.receiptOut), receipt, 0o644);
    }
    console.log(JSON.stringify(receipt, null, 2));
    process.exit(1);
  }
  let installed = false;
  let backup = null;
  if (args.install) {
    backup = installConfig(args.sourceConfig, candidatePath);
    installed = true;
  }
  const receipt = redactedReceipt(args, ticket, candidatePath, readiness, installed, backup);
  if (args.receiptOut) {
    writeJson(path.resolve(args.receiptOut), receipt, 0o644);
  }
  console.log(JSON.stringify(receipt, null, 2));
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  usage();
  process.exit(2);
}
