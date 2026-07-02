#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import process from "node:process";
import { pathToFileURL } from "node:url";

export const GATEWAY_REQUIREMENTS = [
  ["browser_exit_stream", "Carrier Browser byte-stream operation"],
  ["elastos.browser.carrier-stream/v1", "typed Browser Carrier stream schema"],
  ["elastos://exit/open_stream", "remote Exit carrier service"],
];

export const EXIT_PROVIDER_REQUIREMENTS = [
  ["remote_carrier_exits", "source-side remote Carrier exit config"],
  ["elastos.exit.remote-carrier.discovery/v1", "principal-scoped discovery receipt"],
  ["elastos.exit.remote-carrier.quote/v1", "preview quote receipt"],
  ["elastos.exit.remote-carrier-session/v1", "source-side Carrier stream reservation receipt"],
  ["elastos.exit.relay-ipc/v1", "exit-side private relay IPC receipt"],
  ["max_active_streams_per_principal", "per-principal stream quota accounting"],
];

function usage() {
  console.error(`Usage:
  node scripts/remote-carrier-exit-artifact-readiness.mjs \\
    --gateway-bin /path/to/elastos \\
    --exit-provider-bin /path/to/exit-provider

Fails closed unless the installed artifacts contain the Browser Carrier stream
operation and the remote Carrier Exit provider contracts needed for a reviewed
operator-to-operator Browser exit lane. The report contains file hashes and
boolean capability checks only; it does not read or print route tickets.
`);
}

function parseArgs(argv) {
  const args = {
    gatewayBin: "",
    exitProviderBin: "",
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
    } else if (arg === "--gateway-bin") {
      args.gatewayBin = next();
    } else if (arg === "--exit-provider-bin") {
      args.exitProviderBin = next();
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }
  if (!args.gatewayBin) {
    throw new Error("--gateway-bin is required");
  }
  if (!args.exitProviderBin) {
    throw new Error("--exit-provider-bin is required");
  }
  return args;
}

function readArtifact(path) {
  let stat;
  try {
    stat = fs.statSync(path);
  } catch (error) {
    throw new Error(`artifact not readable: ${path}: ${error.message}`);
  }
  if (!stat.isFile()) {
    throw new Error(`artifact is not a regular file: ${path}`);
  }
  const bytes = fs.readFileSync(path);
  return {
    path,
    size_bytes: stat.size,
    sha256: crypto.createHash("sha256").update(bytes).digest("hex"),
    text: bytes.toString("latin1"),
  };
}

function analyzeArtifact(artifact, requirements) {
  const required_strings = requirements.map(([needle, label]) => ({
    label,
    needle,
    present: artifact.text.includes(needle),
  }));
  return {
    path: artifact.path,
    size_bytes: artifact.size_bytes,
    sha256: artifact.sha256,
    required_strings,
  };
}

function collectFailures(name, report) {
  return report.required_strings
    .filter((requirement) => !requirement.present)
    .map((requirement) => `${name}_missing_${requirement.needle}`);
}

export function analyzeRemoteCarrierExitArtifacts({
  gatewayBin,
  exitProviderBin,
  generatedAt = new Date().toISOString(),
}) {
  const gateway = analyzeArtifact(readArtifact(gatewayBin), GATEWAY_REQUIREMENTS);
  const exitProvider = analyzeArtifact(
    readArtifact(exitProviderBin),
    EXIT_PROVIDER_REQUIREMENTS,
  );
  const failures = [
    ...collectFailures("gateway", gateway),
    ...collectFailures("exit_provider", exitProvider),
  ];
  return {
    schema: "elastos.remote-carrier-exit.artifact-readiness/v1",
    ok: failures.length === 0,
    generated_at: generatedAt,
    artifacts: {
      gateway,
      exit_provider: exitProvider,
    },
    failures,
    next_steps: failures.length === 0 ? [
      "Run scripts/remote-carrier-exit-readiness.mjs against the installed source and exit configs for the reviewed route.",
      "Collect real Browser machine proof and operator evidence through the reviewed Carrier stream lane.",
    ] : [
      "Rebuild and install the gateway and exit-provider artifacts from the same reviewed commit before remote Carrier Browser Exit acceptance.",
      "Restart the runtimes so the installed artifacts and provider registry are reloaded, then rerun this check.",
    ],
  };
}

function main(argv) {
  const args = parseArgs(argv);
  const result = analyzeRemoteCarrierExitArtifacts({
    gatewayBin: args.gatewayBin,
    exitProviderBin: args.exitProviderBin,
  });
  console.log(JSON.stringify(result, null, 2));
  if (!result.ok) {
    process.exit(1);
  }
}

try {
  if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
    main(process.argv.slice(2));
  }
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  usage();
  process.exit(2);
}
