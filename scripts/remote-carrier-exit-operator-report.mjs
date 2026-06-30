#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import process from "node:process";

const REQUIRED_CHECKS = [
  "two_runtimes_distinct",
  "installed_artifact_readiness_observed",
  "route_readiness_observed",
  "carrier_stream_transport",
  "remote_exit_discovery_observed",
  "browser_exit_stream_observed",
  "remote_exit_provider_relay_ipc_observed",
  "connect_ticket_not_exposed_to_browser",
  "no_raw_socket_or_dns_authority_to_capsule",
  "policy_target_allowlist_enforced",
  "principal_accounting_observed",
  "quota_or_close_accounting_observed",
  "cleanup_observed",
];
const REQUIRED_ARTIFACTS = [
  "carrier_only_authority_check",
  "installed_artifact_readiness",
  "route_readiness",
  "source_gateway_log",
  "exit_gateway_log",
  "browser_machine_proof",
];

function usage() {
  console.error(`Usage:
  node scripts/remote-carrier-exit-operator-report.mjs --template
  node scripts/remote-carrier-exit-operator-report.mjs --input /path/to/evidence.json

Creates or validates operator evidence for a real remote Carrier Browser Exit
run. This records the human/operator proof that a Browser capsule on one runtime
used Carrier to reach an exit provider on another runtime without exposing raw
network authority or private route tickets to the capsule.
`);
}

function parseArgs(argv) {
  const args = { template: false, input: "" };
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
    } else if (arg === "--template") {
      args.template = true;
    } else if (arg === "--input") {
      args.input = next();
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }
  if (args.template === Boolean(args.input)) {
    throw new Error("use exactly one of --template or --input");
  }
  return args;
}

function template() {
  return {
    schema: "elastos.remote-carrier-exit.operator-evidence/v1",
    ok: false,
    reviewed_at: new Date(0).toISOString(),
    reviewer: "",
    source_runtime: {
      role: "browser-source",
      did: "",
      endpoint: "",
    },
    exit_runtime: {
      role: "remote-exit",
      did: "",
      endpoint: "",
    },
    route: {
      carrier_service: "elastos://exit/open_stream",
      byte_transport: "carrier_stream",
      principal: "",
      grant_id: "",
      target: "",
    },
    checks: Object.fromEntries(REQUIRED_CHECKS.map((name) => [name, false])),
    evidence: Object.fromEntries(REQUIRED_CHECKS.map((name) => [name, ""])),
    artifacts: Object.fromEntries(REQUIRED_ARTIFACTS.map((name) => [name, {
      path: "",
      sha256: "",
    }])),
    notes: [
      "Set ok=true only after observing a real source runtime opening Browser traffic through a distinct remote exit runtime over Carrier.",
      "Do not paste connect_ticket, relay_ipc paths, adapter_ipc paths, raw sockets, cookies, passkeys, or private provider config into this file.",
      "Evidence text should identify redacted log excerpts, command outputs, artifact paths, or operator observations for each check.",
      "remote_exit_discovery_observed should cite the principal-scoped discover_remote_carrier_exits response that showed the selected grant without connect_ticket or private IPC material.",
      "two_runtimes_distinct should cite both source/exit runtime DIDs and both redacted endpoint evidence strings.",
      "installed_artifact_readiness_observed should cite elastos.remote-carrier-exit.artifact-readiness/v1 and the Browser Carrier stream operation.",
      "route_readiness_observed should cite elastos.remote-carrier-exit.readiness/v1, config_sha256, and the reviewed route principal/grant/target.",
      "Artifact fields must include a redacted local or remote path plus the SHA-256 digest of the reviewed redacted artifact; keep secrets out of artifact paths and contents pasted here.",
    ],
  };
}

function nonEmpty(value) {
  return typeof value === "string" && value.trim() !== "";
}

function safePublicText(value) {
  if (value == null) {
    return true;
  }
  if (typeof value === "string") {
    return !/((["']?(connect_ticket|relay_ipc|adapter_ipc|runtime_stream_path)["']?\s*[:=])|ticket:[^\s,}]+)/i.test(value);
  }
  if (Array.isArray(value)) {
    return value.every(safePublicText);
  }
  if (typeof value === "object") {
    const privateFieldNames = new Set([
      "connect_ticket",
      "relay_ipc",
      "adapter_ipc",
      "runtime_stream_path",
    ]);
    return Object.entries(value).every(([key, child]) =>
      !privateFieldNames.has(key) && safePublicText(child)
    );
  }
  return true;
}

function sha256File(path) {
  return crypto.createHash("sha256").update(fs.readFileSync(path)).digest("hex");
}

function isSha256(value) {
  return typeof value === "string" && /^[a-f0-9]{64}$/i.test(value);
}

function localArtifactTextIsRedacted(path) {
  return safePublicText(fs.readFileSync(path, "utf8"));
}

function includesText(value, needle) {
  return (
    typeof value === "string" &&
    typeof needle === "string" &&
    needle.trim() !== "" &&
    value.toLowerCase().includes(needle.toLowerCase())
  );
}

function evidenceText(report, check) {
  const value = report.evidence?.[check];
  return typeof value === "string" ? value : "";
}

function validateArtifactReference(report, artifact, errors) {
  const value = report.artifacts?.[artifact];
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    errors.push(`artifacts.${artifact} must be an object with path and sha256`);
    return;
  }
  if (!nonEmpty(value.path)) {
    errors.push(`artifacts.${artifact}.path is required`);
  }
  if (typeof value.sha256 !== "string" || !/^[a-f0-9]{64}$/i.test(value.sha256)) {
    errors.push(`artifacts.${artifact}.sha256 must be a 64-character hex SHA-256 digest`);
  } else if (nonEmpty(value.path) && fs.existsSync(value.path)) {
    if (sha256File(value.path).toLowerCase() !== value.sha256.toLowerCase()) {
      errors.push(`artifacts.${artifact}.sha256 must match the local redacted artifact file`);
    }
    if (!localArtifactTextIsRedacted(value.path)) {
      errors.push(`artifacts.${artifact}.path must point to a redacted artifact without private route material`);
    }
  }
}

function routeTargetReferences(report) {
  const target = report.route?.target;
  if (!nonEmpty(target)) {
    return [];
  }
  const references = [target.trim()];
  try {
    const parsed = new URL(target);
    if (nonEmpty(parsed.hostname)) {
      references.push(parsed.hostname);
    }
    if (nonEmpty(parsed.host)) {
      references.push(parsed.host);
    }
  } catch {
    const host = target
      .replace(/^[a-z][a-z0-9+.-]*:\/\//i, "")
      .split(/[/?#]/)[0]
      .trim();
    if (host) {
      references.push(host);
      references.push(host.split(":")[0]);
    }
  }
  return [...new Set(references.filter(nonEmpty))];
}

function localBrowserMachineProofCitesRouteTarget(report) {
  const artifact = report.artifacts?.browser_machine_proof;
  if (!nonEmpty(artifact?.path) || !fs.existsSync(artifact.path)) {
    return true;
  }
  const text = fs.readFileSync(artifact.path, "utf8").toLowerCase();
  return routeTargetReferences(report).some((reference) =>
    text.includes(reference.toLowerCase())
  );
}

function validateLocalArtifactReadiness(report, errors) {
  const artifact = report.artifacts?.installed_artifact_readiness;
  if (!nonEmpty(artifact?.path) || !fs.existsSync(artifact.path)) {
    return;
  }
  let parsed;
  try {
    parsed = JSON.parse(fs.readFileSync(artifact.path, "utf8"));
  } catch {
    errors.push("artifacts.installed_artifact_readiness.path must contain JSON");
    return;
  }
  if (parsed.schema !== "elastos.remote-carrier-exit.artifact-readiness/v1") {
    errors.push("artifacts.installed_artifact_readiness.path must be an artifact-readiness report");
  }
  if (parsed.ok !== true) {
    errors.push("artifacts.installed_artifact_readiness.path must report ok=true");
  }
  const gatewayStrings = parsed.artifacts?.gateway?.required_strings;
  const exitProviderStrings = parsed.artifacts?.exit_provider?.required_strings;
  const allPresent = (values) => Array.isArray(values) &&
    values.length > 0 &&
    values.every((value) => value?.present === true);
  if (!allPresent(gatewayStrings)) {
    errors.push("artifacts.installed_artifact_readiness.path must prove gateway Browser Carrier stream strings");
  }
  if (!allPresent(exitProviderStrings)) {
    errors.push("artifacts.installed_artifact_readiness.path must prove exit-provider remote Carrier strings");
  }
}

function validateLocalRouteReadiness(report, errors) {
  const artifact = report.artifacts?.route_readiness;
  if (!nonEmpty(artifact?.path) || !fs.existsSync(artifact.path)) {
    return;
  }
  let parsed;
  try {
    parsed = JSON.parse(fs.readFileSync(artifact.path, "utf8"));
  } catch {
    errors.push("artifacts.route_readiness.path must contain JSON");
    return;
  }
  if (parsed.schema !== "elastos.remote-carrier-exit.readiness/v1") {
    errors.push("artifacts.route_readiness.path must be a route-readiness report");
  }
  if (parsed.ok !== true) {
    errors.push("artifacts.route_readiness.path must report ok=true");
  }
  if (parsed.route?.carrier_service !== report.route?.carrier_service) {
    errors.push("artifacts.route_readiness.path route.carrier_service must match report.route.carrier_service");
  }
  if (parsed.route?.byte_transport !== report.route?.byte_transport) {
    errors.push("artifacts.route_readiness.path route.byte_transport must match report.route.byte_transport");
  }
  for (const field of ["principal", "grant_id", "target"]) {
    if (parsed.route?.[field] !== report.route?.[field]) {
      errors.push(`artifacts.route_readiness.path route.${field} must match report.route.${field}`);
    }
  }
  if (parsed.source?.remote_only !== true) {
    errors.push("artifacts.route_readiness.path must prove source.remote_only=true");
  }
  if (!isSha256(parsed.source?.config_sha256)) {
    errors.push("artifacts.route_readiness.path must include source.config_sha256");
  }
  if (!isSha256(parsed.exit?.config_sha256)) {
    errors.push("artifacts.route_readiness.path must include exit.config_sha256");
  }
  if (parsed.source?.selected_remote_exit?.connect_ticket_present !== true) {
    errors.push("artifacts.route_readiness.path must prove selected remote exit ticket presence without exposing it");
  }
  if (parsed.exit?.selected_stream_relay_backend?.adapter_ipc_present !== true) {
    errors.push("artifacts.route_readiness.path must prove exit adapter IPC presence without exposing it");
  }
  if (parsed.exit?.selected_stream_relay_backend?.relay_ipc_present !== true) {
    errors.push("artifacts.route_readiness.path must prove exit relay IPC presence without exposing it");
  }
}

function validate(report) {
  const errors = [];
  if (report.schema !== "elastos.remote-carrier-exit.operator-evidence/v1") {
    errors.push("schema must be elastos.remote-carrier-exit.operator-evidence/v1");
  }
  if (report.ok !== true) {
    errors.push("ok must be true only after real operator review");
  }
  if (!nonEmpty(report.reviewed_at) || Number.isNaN(Date.parse(report.reviewed_at))) {
    errors.push("reviewed_at must be an ISO timestamp");
  }
  if (!nonEmpty(report.reviewer)) {
    errors.push("reviewer is required");
  }
  if (!nonEmpty(report.source_runtime?.did)) {
    errors.push("source_runtime.did is required");
  }
  if (report.source_runtime?.role !== "browser-source") {
    errors.push("source_runtime.role must be browser-source");
  }
  if (!nonEmpty(report.source_runtime?.endpoint)) {
    errors.push("source_runtime.endpoint is required");
  }
  if (!nonEmpty(report.exit_runtime?.did)) {
    errors.push("exit_runtime.did is required");
  }
  if (report.exit_runtime?.role !== "remote-exit") {
    errors.push("exit_runtime.role must be remote-exit");
  }
  if (!nonEmpty(report.exit_runtime?.endpoint)) {
    errors.push("exit_runtime.endpoint is required");
  }
  if (
    nonEmpty(report.source_runtime?.did) &&
    nonEmpty(report.exit_runtime?.did) &&
    report.source_runtime.did === report.exit_runtime.did
  ) {
    errors.push("source_runtime.did and exit_runtime.did must be distinct");
  }
  if (
    nonEmpty(report.source_runtime?.endpoint) &&
    nonEmpty(report.exit_runtime?.endpoint) &&
    report.source_runtime.endpoint === report.exit_runtime.endpoint
  ) {
    errors.push("source_runtime.endpoint and exit_runtime.endpoint must be distinct");
  }
  if (report.route?.carrier_service !== "elastos://exit/open_stream") {
    errors.push("route.carrier_service must be elastos://exit/open_stream");
  }
  if (report.route?.byte_transport !== "carrier_stream") {
    errors.push("route.byte_transport must be carrier_stream");
  }
  for (const field of ["principal", "grant_id", "target"]) {
    if (!nonEmpty(report.route?.[field])) {
      errors.push(`route.${field} is required`);
    }
  }
  for (const check of REQUIRED_CHECKS) {
    if (report.checks?.[check] !== true) {
      errors.push(`checks.${check} must be true`);
    }
    if (!nonEmpty(report.evidence?.[check])) {
      errors.push(`evidence.${check} is required`);
    }
  }
  const unknownChecks = Object.keys(report.checks || {}).filter((key) => !REQUIRED_CHECKS.includes(key));
  const unknownEvidence = Object.keys(report.evidence || {}).filter((key) => !REQUIRED_CHECKS.includes(key));
  if (unknownChecks.length > 0) {
    errors.push(`unknown checks: ${unknownChecks.join(", ")}`);
  }
  if (unknownEvidence.length > 0) {
    errors.push(`unknown evidence fields: ${unknownEvidence.join(", ")}`);
  }
  if (
    nonEmpty(report.route?.principal) &&
    nonEmpty(report.route?.grant_id) &&
    (
      !includesText(evidenceText(report, "remote_exit_discovery_observed"), report.route.principal) ||
      !includesText(evidenceText(report, "remote_exit_discovery_observed"), report.route.grant_id)
    )
  ) {
    errors.push("evidence.remote_exit_discovery_observed must cite route.principal and route.grant_id");
  }
  if (
    nonEmpty(report.source_runtime?.did) &&
    nonEmpty(report.exit_runtime?.did) &&
    (
      !includesText(evidenceText(report, "two_runtimes_distinct"), report.source_runtime.did) ||
      !includesText(evidenceText(report, "two_runtimes_distinct"), report.exit_runtime.did)
    )
  ) {
    errors.push("evidence.two_runtimes_distinct must cite source_runtime.did and exit_runtime.did");
  }
  if (
    nonEmpty(report.source_runtime?.endpoint) &&
    nonEmpty(report.exit_runtime?.endpoint) &&
    (
      !includesText(evidenceText(report, "two_runtimes_distinct"), report.source_runtime.endpoint) ||
      !includesText(evidenceText(report, "two_runtimes_distinct"), report.exit_runtime.endpoint)
    )
  ) {
    errors.push("evidence.two_runtimes_distinct must cite source_runtime.endpoint and exit_runtime.endpoint");
  }
  if (!includesText(evidenceText(report, "carrier_stream_transport"), "carrier_stream")) {
    errors.push("evidence.carrier_stream_transport must cite carrier_stream");
  }
  if (
    !includesText(
      evidenceText(report, "installed_artifact_readiness_observed"),
      "elastos.remote-carrier-exit.artifact-readiness/v1",
    ) ||
    !includesText(evidenceText(report, "installed_artifact_readiness_observed"), "browser_exit_stream")
  ) {
    errors.push("evidence.installed_artifact_readiness_observed must cite artifact readiness and browser_exit_stream");
  }
  if (
    !includesText(evidenceText(report, "route_readiness_observed"), "elastos.remote-carrier-exit.readiness/v1") ||
    !includesText(evidenceText(report, "route_readiness_observed"), "config_sha256")
  ) {
    errors.push("evidence.route_readiness_observed must cite route readiness and config_sha256");
  }
  if (
    nonEmpty(report.route?.principal) &&
    nonEmpty(report.route?.grant_id) &&
    nonEmpty(report.route?.target) &&
    (
      !includesText(evidenceText(report, "route_readiness_observed"), report.route.principal) ||
      !includesText(evidenceText(report, "route_readiness_observed"), report.route.grant_id) ||
      !includesText(evidenceText(report, "route_readiness_observed"), report.route.target)
    )
  ) {
    errors.push("evidence.route_readiness_observed must cite route.principal, route.grant_id, and route.target");
  }
  if (!includesText(evidenceText(report, "browser_exit_stream_observed"), "browser_exit_stream")) {
    errors.push("evidence.browser_exit_stream_observed must cite browser_exit_stream");
  }
  if (
    !includesText(evidenceText(report, "remote_exit_provider_relay_ipc_observed"), "exit-provider") ||
    !includesText(evidenceText(report, "remote_exit_provider_relay_ipc_observed"), "relay")
  ) {
    errors.push("evidence.remote_exit_provider_relay_ipc_observed must cite the remote exit-provider relay handoff");
  }
  if (
    nonEmpty(report.route?.target) &&
    !includesText(evidenceText(report, "policy_target_allowlist_enforced"), report.route.target)
  ) {
    errors.push("evidence.policy_target_allowlist_enforced must cite route.target");
  }
  if (
    nonEmpty(report.route?.principal) &&
    !includesText(evidenceText(report, "principal_accounting_observed"), report.route.principal)
  ) {
    errors.push("evidence.principal_accounting_observed must cite route.principal");
  }
  if (
    nonEmpty(report.route?.principal) &&
    nonEmpty(report.route?.grant_id) &&
    !includesText(evidenceText(report, "quota_or_close_accounting_observed"), report.route.principal) &&
    !includesText(evidenceText(report, "quota_or_close_accounting_observed"), report.route.grant_id)
  ) {
    errors.push("evidence.quota_or_close_accounting_observed must cite route.principal or route.grant_id");
  }
  if (
    nonEmpty(report.route?.principal) &&
    nonEmpty(report.route?.grant_id) &&
    !includesText(evidenceText(report, "cleanup_observed"), report.route.principal) &&
    !includesText(evidenceText(report, "cleanup_observed"), report.route.grant_id)
  ) {
    errors.push("evidence.cleanup_observed must cite route.principal or route.grant_id");
  }
  for (const artifact of REQUIRED_ARTIFACTS) {
    validateArtifactReference(report, artifact, errors);
  }
  validateLocalArtifactReadiness(report, errors);
  validateLocalRouteReadiness(report, errors);
  if (!localBrowserMachineProofCitesRouteTarget(report)) {
    errors.push("artifacts.browser_machine_proof.path must cite route.target or its host");
  }
  const unknownArtifacts = Object.keys(report.artifacts || {}).filter((key) => !REQUIRED_ARTIFACTS.includes(key));
  if (unknownArtifacts.length > 0) {
    errors.push(`unknown artifacts: ${unknownArtifacts.join(", ")}`);
  }
  if (!safePublicText(report)) {
    errors.push("report must not contain connect_ticket, relay_ipc, adapter_ipc, runtime_stream_path, or ticket secrets");
  }
  return {
    schema: "elastos.remote-carrier-exit.operator-evidence.validation/v1",
    ok: errors.length === 0,
    required_checks: REQUIRED_CHECKS,
    required_artifacts: REQUIRED_ARTIFACTS,
    errors,
  };
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.template) {
    console.log(JSON.stringify(template(), null, 2));
    return;
  }
  const report = JSON.parse(fs.readFileSync(args.input, "utf8"));
  const result = validate(report);
  console.log(JSON.stringify(result, null, 2));
  if (!result.ok) {
    process.exit(1);
  }
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  usage();
  process.exit(2);
}
