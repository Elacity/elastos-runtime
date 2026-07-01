#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

find_node() {
  if [[ -n "${ELASTOS_NODE_BIN:-}" && -x "${ELASTOS_NODE_BIN}" ]]; then
    printf '%s\n' "${ELASTOS_NODE_BIN}"
    return 0
  fi
  if command -v node >/dev/null 2>&1; then
    command -v node
    return 0
  fi
  local bundled="${HOME}/.elastos/node/node-v22.13.1-darwin-arm64/bin/node"
  if [[ -x "$bundled" ]]; then
    printf '%s\n' "$bundled"
    return 0
  fi
  return 1
}

node_bin="$(find_node || true)"
if [[ -z "$node_bin" ]]; then
  echo "node not found. Install Node or set ELASTOS_NODE_BIN to an executable node binary." >&2
  exit 2
fi

tmp_dir="$(mktemp -d /tmp/elastos-remote-carrier-exit-operator-report-smoke-XXXXXX)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

template="$tmp_dir/template.json"
valid="$tmp_dir/valid.json"
leaky="$tmp_dir/leaky.json"
missing_artifact="$tmp_dir/missing-artifact.json"
missing_artifact_digest="$tmp_dir/missing-artifact-digest.json"
missing_endpoint="$tmp_dir/missing-endpoint.json"
same_endpoint="$tmp_dir/same-endpoint.json"
swapped_roles="$tmp_dir/swapped-roles.json"
missing_principal="$tmp_dir/missing-principal.json"
weak_evidence="$tmp_dir/weak-evidence.json"
local_artifact_hash_mismatch="$tmp_dir/local-artifact-hash-mismatch.json"
local_artifact_secret_leak="$tmp_dir/local-artifact-secret-leak.json"
local_browser_proof_target_mismatch="$tmp_dir/local-browser-proof-target-mismatch.json"
stale_artifact_readiness="$tmp_dir/stale-artifact-readiness.json"
stale_route_readiness="$tmp_dir/stale-route-readiness.json"

"$node_bin" "$repo_root/scripts/remote-carrier-exit-operator-report.mjs" \
  --template \
  >"$template"

"$node_bin" - "$template" "$valid" "$leaky" "$missing_artifact" "$missing_artifact_digest" "$missing_endpoint" "$same_endpoint" "$swapped_roles" "$missing_principal" "$weak_evidence" "$local_artifact_hash_mismatch" "$local_artifact_secret_leak" "$local_browser_proof_target_mismatch" "$stale_artifact_readiness" "$stale_route_readiness" "$tmp_dir" <<'NODE'
const fs = require("node:fs");
const crypto = require("node:crypto");
const path = require("node:path");
const [
  templatePath,
  validPath,
  leakyPath,
  missingArtifactPath,
  missingArtifactDigestPath,
  missingEndpointPath,
  sameEndpointPath,
  swappedRolesPath,
  missingPrincipalPath,
  weakEvidencePath,
  localArtifactHashMismatchPath,
  localArtifactSecretLeakPath,
  localBrowserProofTargetMismatchPath,
  staleArtifactReadinessPath,
  staleRouteReadinessPath,
  artifactDir,
] = process.argv.slice(2);
const writeArtifact = (name, text) => {
  const artifactPath = path.join(artifactDir, name);
  fs.writeFileSync(artifactPath, text);
  return {
    path: artifactPath,
    sha256: crypto.createHash("sha256").update(fs.readFileSync(artifactPath)).digest("hex"),
  };
};
const report = JSON.parse(fs.readFileSync(templatePath, "utf8"));
report.ok = true;
report.reviewed_at = "2026-06-19T00:00:00Z";
report.reviewer = "remote-carrier-exit-operator-report-smoke";
report.source_runtime.did = "did:key:zSourceRuntime";
report.source_runtime.endpoint = "source runtime localhost";
report.exit_runtime.did = "did:key:zExitRuntime";
report.exit_runtime.endpoint = "exit runtime localhost";
report.route.principal = "principal:alice";
report.route.grant_id = "operator-grant:server-exit:alice";
report.route.target = "tls://example.com:443";
for (const key of Object.keys(report.checks)) {
  report.checks[key] = true;
  report.evidence[key] = `observed ${key} with redacted operator logs`;
}
report.evidence.remote_exit_discovery_observed = "principal-scoped discovery showed principal:alice grant operator-grant:server-exit:alice without private route material";
report.evidence.two_runtimes_distinct = "source runtime did:key:zSourceRuntime at source runtime localhost and exit runtime did:key:zExitRuntime at exit runtime localhost were observed as distinct runtimes";
report.evidence.installed_artifact_readiness_observed = "elastos.remote-carrier-exit.artifact-readiness/v1 proved browser_exit_stream and remote Carrier exit-provider contracts";
report.evidence.route_readiness_observed = "elastos.remote-carrier-exit.readiness/v1 proved config_sha256 for principal:alice grant operator-grant:server-exit:alice target tls://example.com:443";
report.evidence.carrier_stream_transport = "source Browser session used byte_transport carrier_stream";
report.evidence.browser_exit_stream_observed = "source runtime opened Carrier browser_exit_stream to the remote exit runtime";
report.evidence.remote_exit_provider_relay_ipc_observed = "remote exit-provider returned a redacted relay handoff before bytes flowed";
report.evidence.policy_target_allowlist_enforced = "target allowlist accepted tls://example.com:443 and rejected a private target";
report.evidence.principal_accounting_observed = "accounting recorded active stream usage for principal:alice";
report.evidence.quota_or_close_accounting_observed = "quota/close accounting decremented operator-grant:server-exit:alice after close";
report.evidence.cleanup_observed = "cleanup closed the operator-grant:server-exit:alice Carrier exit session and removed source/exit stream state";
report.artifacts.carrier_only_authority_check = {
  ...writeArtifact("redacted-carrier-only-authority-check.json", "carrier-only authority check redacted\n"),
};
report.artifacts.installed_artifact_readiness = {
  ...writeArtifact("redacted-installed-artifact-readiness.json", JSON.stringify({
    schema: "elastos.remote-carrier-exit.artifact-readiness/v1",
    ok: true,
    artifacts: {
      gateway: {
        required_strings: [
          { needle: "browser_exit_stream", present: true },
          { needle: "elastos.browser.carrier-stream/v1", present: true },
        ],
      },
      exit_provider: {
        required_strings: [
          { needle: "remote_carrier_exits", present: true },
          { needle: "elastos.exit.remote-carrier-session/v1", present: true },
        ],
      },
    },
  }, null, 2)),
};
report.artifacts.route_readiness = {
  ...writeArtifact("redacted-route-readiness.json", JSON.stringify({
    schema: "elastos.remote-carrier-exit.readiness/v1",
    ok: true,
    route: {
      carrier_service: "elastos://exit/open_stream",
      byte_transport: "carrier_stream",
      principal: "principal:alice",
      grant_id: "operator-grant:server-exit:alice",
      target: "tls://example.com:443",
    },
    source: {
      config_sha256: "1".repeat(64),
      remote_only: true,
      selected_remote_exit: {
        connect_ticket_present: true,
      },
    },
    exit: {
      config_sha256: "2".repeat(64),
      selected_stream_relay_backend: {
        adapter_ipc_present: true,
        relay_ipc_present: true,
      },
    },
  }, null, 2)),
};
report.artifacts.source_gateway_log = {
  ...writeArtifact("redacted-source-gateway.log", "source gateway log redacted\n"),
};
report.artifacts.exit_gateway_log = {
  ...writeArtifact("redacted-exit-gateway.log", "exit gateway log redacted\n"),
};
report.artifacts.browser_machine_proof = {
  ...writeArtifact("redacted-browser-proof.json", "browser machine proof redacted target tls://example.com:443\n"),
};
fs.writeFileSync(validPath, `${JSON.stringify(report, null, 2)}\n`);
const leaky = structuredClone(report);
leaky.evidence.connect_ticket_not_exposed_to_browser = "bad connect_ticket ticket:secret";
fs.writeFileSync(leakyPath, `${JSON.stringify(leaky, null, 2)}\n`);
const missingArtifact = structuredClone(report);
missingArtifact.artifacts.browser_machine_proof = "";
fs.writeFileSync(missingArtifactPath, `${JSON.stringify(missingArtifact, null, 2)}\n`);
const missingArtifactDigest = structuredClone(report);
missingArtifactDigest.artifacts.browser_machine_proof.sha256 = "";
fs.writeFileSync(missingArtifactDigestPath, `${JSON.stringify(missingArtifactDigest, null, 2)}\n`);
const missingEndpoint = structuredClone(report);
missingEndpoint.exit_runtime.endpoint = "";
fs.writeFileSync(missingEndpointPath, `${JSON.stringify(missingEndpoint, null, 2)}\n`);
const sameEndpoint = structuredClone(report);
sameEndpoint.exit_runtime.endpoint = sameEndpoint.source_runtime.endpoint;
fs.writeFileSync(sameEndpointPath, `${JSON.stringify(sameEndpoint, null, 2)}\n`);
const swappedRoles = structuredClone(report);
swappedRoles.source_runtime.role = "remote-exit";
swappedRoles.exit_runtime.role = "browser-source";
fs.writeFileSync(swappedRolesPath, `${JSON.stringify(swappedRoles, null, 2)}\n`);
const missingPrincipal = structuredClone(report);
missingPrincipal.route.principal = "";
fs.writeFileSync(missingPrincipalPath, `${JSON.stringify(missingPrincipal, null, 2)}\n`);
const weakEvidence = structuredClone(report);
weakEvidence.evidence.two_runtimes_distinct = "two runtimes looked good in logs";
weakEvidence.evidence.remote_exit_discovery_observed = "discovery looked good in logs";
weakEvidence.evidence.installed_artifact_readiness_observed = "artifact readiness looked good in logs";
weakEvidence.evidence.route_readiness_observed = "route readiness looked good in logs";
weakEvidence.evidence.carrier_stream_transport = "transport looked good in logs";
weakEvidence.evidence.browser_exit_stream_observed = "browser stream looked good in logs";
weakEvidence.evidence.remote_exit_provider_relay_ipc_observed = "relay looked good in logs";
weakEvidence.evidence.policy_target_allowlist_enforced = "policy looked good in logs";
weakEvidence.evidence.principal_accounting_observed = "accounting looked good in logs";
weakEvidence.evidence.quota_or_close_accounting_observed = "quota looked good in logs";
weakEvidence.evidence.cleanup_observed = "cleanup looked good in logs";
fs.writeFileSync(weakEvidencePath, `${JSON.stringify(weakEvidence, null, 2)}\n`);
const localArtifactHashMismatch = structuredClone(report);
localArtifactHashMismatch.artifacts.source_gateway_log.sha256 = "0".repeat(64);
fs.writeFileSync(localArtifactHashMismatchPath, `${JSON.stringify(localArtifactHashMismatch, null, 2)}\n`);
const localArtifactSecretLeak = structuredClone(report);
localArtifactSecretLeak.artifacts.source_gateway_log = {
  ...writeArtifact("leaky-source-gateway.log", "connect_ticket ticket:secret redaction failed\n"),
};
fs.writeFileSync(localArtifactSecretLeakPath, `${JSON.stringify(localArtifactSecretLeak, null, 2)}\n`);
const localBrowserProofTargetMismatch = structuredClone(report);
localBrowserProofTargetMismatch.artifacts.browser_machine_proof = {
  ...writeArtifact("redacted-browser-proof-wrong-target.json", "browser machine proof redacted target tls://wrong.example:443\n"),
};
fs.writeFileSync(localBrowserProofTargetMismatchPath, `${JSON.stringify(localBrowserProofTargetMismatch, null, 2)}\n`);
const staleArtifactReadiness = structuredClone(report);
staleArtifactReadiness.artifacts.installed_artifact_readiness = {
  ...writeArtifact("stale-installed-artifact-readiness.json", JSON.stringify({
    schema: "elastos.remote-carrier-exit.artifact-readiness/v1",
    ok: false,
    artifacts: {
      gateway: {
        required_strings: [
          { needle: "browser_exit_stream", present: false },
        ],
      },
      exit_provider: {
        required_strings: [
          { needle: "elastos.exit.remote-carrier-session/v1", present: false },
        ],
      },
    },
  }, null, 2)),
};
fs.writeFileSync(staleArtifactReadinessPath, `${JSON.stringify(staleArtifactReadiness, null, 2)}\n`);
const staleRouteReadiness = structuredClone(report);
staleRouteReadiness.artifacts.route_readiness = {
  ...writeArtifact("redacted-stale-route-readiness-artifact.json", JSON.stringify({
    schema: "elastos.remote-carrier-exit.readiness/v1",
    ok: false,
    route: {
      carrier_service: "elastos://exit/open_stream",
      byte_transport: "carrier_stream",
      principal: "principal:alice",
      grant_id: "operator-grant:server-exit:alice",
      target: "tls://wrong.example:443",
    },
    source: {
      remote_only: false,
      selected_remote_exit: {
        connect_ticket_present: false,
      },
    },
    exit: {
      selected_stream_relay_backend: {
        adapter_ipc_present: false,
        relay_ipc_present: false,
      },
    },
  }, null, 2)),
};
fs.writeFileSync(staleRouteReadinessPath, `${JSON.stringify(staleRouteReadiness, null, 2)}\n`);
NODE

"$node_bin" "$repo_root/scripts/remote-carrier-exit-operator-report.mjs" \
  --input "$valid" \
  >"$tmp_dir/valid-result.json"

set +e
"$node_bin" "$repo_root/scripts/remote-carrier-exit-operator-report.mjs" \
  --input "$template" \
  >"$tmp_dir/template-result.json"
template_status=$?
"$node_bin" "$repo_root/scripts/remote-carrier-exit-operator-report.mjs" \
  --input "$leaky" \
  >"$tmp_dir/leaky-result.json"
leaky_status=$?
"$node_bin" "$repo_root/scripts/remote-carrier-exit-operator-report.mjs" \
  --input "$missing_artifact" \
  >"$tmp_dir/missing-artifact-result.json"
missing_artifact_status=$?
"$node_bin" "$repo_root/scripts/remote-carrier-exit-operator-report.mjs" \
  --input "$missing_artifact_digest" \
  >"$tmp_dir/missing-artifact-digest-result.json"
missing_artifact_digest_status=$?
"$node_bin" "$repo_root/scripts/remote-carrier-exit-operator-report.mjs" \
  --input "$missing_endpoint" \
  >"$tmp_dir/missing-endpoint-result.json"
missing_endpoint_status=$?
"$node_bin" "$repo_root/scripts/remote-carrier-exit-operator-report.mjs" \
  --input "$same_endpoint" \
  >"$tmp_dir/same-endpoint-result.json"
same_endpoint_status=$?
"$node_bin" "$repo_root/scripts/remote-carrier-exit-operator-report.mjs" \
  --input "$swapped_roles" \
  >"$tmp_dir/swapped-roles-result.json"
swapped_roles_status=$?
"$node_bin" "$repo_root/scripts/remote-carrier-exit-operator-report.mjs" \
  --input "$missing_principal" \
  >"$tmp_dir/missing-principal-result.json"
missing_principal_status=$?
"$node_bin" "$repo_root/scripts/remote-carrier-exit-operator-report.mjs" \
  --input "$weak_evidence" \
  >"$tmp_dir/weak-evidence-result.json"
weak_evidence_status=$?
"$node_bin" "$repo_root/scripts/remote-carrier-exit-operator-report.mjs" \
  --input "$local_artifact_hash_mismatch" \
  >"$tmp_dir/local-artifact-hash-mismatch-result.json"
local_artifact_hash_mismatch_status=$?
"$node_bin" "$repo_root/scripts/remote-carrier-exit-operator-report.mjs" \
  --input "$local_artifact_secret_leak" \
  >"$tmp_dir/local-artifact-secret-leak-result.json"
local_artifact_secret_leak_status=$?
"$node_bin" "$repo_root/scripts/remote-carrier-exit-operator-report.mjs" \
  --input "$local_browser_proof_target_mismatch" \
  >"$tmp_dir/local-browser-proof-target-mismatch-result.json"
local_browser_proof_target_mismatch_status=$?
"$node_bin" "$repo_root/scripts/remote-carrier-exit-operator-report.mjs" \
  --input "$stale_artifact_readiness" \
  >"$tmp_dir/stale-artifact-readiness-result.json"
stale_artifact_readiness_status=$?
"$node_bin" "$repo_root/scripts/remote-carrier-exit-operator-report.mjs" \
  --input "$stale_route_readiness" \
  >"$tmp_dir/stale-route-readiness-result.json"
stale_route_readiness_status=$?
set -e

if [[ "$template_status" -eq 0 ]]; then
  echo "remote Carrier operator report accepted an unreviewed template" >&2
  cat "$tmp_dir/template-result.json" >&2
  exit 1
fi
if [[ "$leaky_status" -eq 0 ]]; then
  echo "remote Carrier operator report accepted leaked private route material" >&2
  cat "$tmp_dir/leaky-result.json" >&2
  exit 1
fi
if [[ "$missing_artifact_status" -eq 0 ]]; then
  echo "remote Carrier operator report accepted missing artifact evidence" >&2
  cat "$tmp_dir/missing-artifact-result.json" >&2
  exit 1
fi
if [[ "$missing_artifact_digest_status" -eq 0 ]]; then
  echo "remote Carrier operator report accepted missing artifact digest" >&2
  cat "$tmp_dir/missing-artifact-digest-result.json" >&2
  exit 1
fi
if [[ "$missing_endpoint_status" -eq 0 ]]; then
  echo "remote Carrier operator report accepted missing runtime endpoint evidence" >&2
  cat "$tmp_dir/missing-endpoint-result.json" >&2
  exit 1
fi
if [[ "$same_endpoint_status" -eq 0 ]]; then
  echo "remote Carrier operator report accepted identical source/exit endpoints" >&2
  cat "$tmp_dir/same-endpoint-result.json" >&2
  exit 1
fi
if [[ "$swapped_roles_status" -eq 0 ]]; then
  echo "remote Carrier operator report accepted swapped source/exit roles" >&2
  cat "$tmp_dir/swapped-roles-result.json" >&2
  exit 1
fi
if [[ "$missing_principal_status" -eq 0 ]]; then
  echo "remote Carrier operator report accepted missing principal evidence" >&2
  cat "$tmp_dir/missing-principal-result.json" >&2
  exit 1
fi
if [[ "$weak_evidence_status" -eq 0 ]]; then
  echo "remote Carrier operator report accepted evidence not bound to route nouns" >&2
  cat "$tmp_dir/weak-evidence-result.json" >&2
  exit 1
fi
if [[ "$local_artifact_hash_mismatch_status" -eq 0 ]]; then
  echo "remote Carrier operator report accepted a local redacted artifact with a mismatched digest" >&2
  cat "$tmp_dir/local-artifact-hash-mismatch-result.json" >&2
  exit 1
fi
if [[ "$local_artifact_secret_leak_status" -eq 0 ]]; then
  echo "remote Carrier operator report accepted a local redacted artifact that still contains private route material" >&2
  cat "$tmp_dir/local-artifact-secret-leak-result.json" >&2
  exit 1
fi
if [[ "$local_browser_proof_target_mismatch_status" -eq 0 ]]; then
  echo "remote Carrier operator report accepted a local Browser proof for the wrong route target" >&2
  cat "$tmp_dir/local-browser-proof-target-mismatch-result.json" >&2
  exit 1
fi
if [[ "$stale_artifact_readiness_status" -eq 0 ]]; then
  echo "remote Carrier operator report accepted stale installed-artifact readiness" >&2
  cat "$tmp_dir/stale-artifact-readiness-result.json" >&2
  exit 1
fi
if [[ "$stale_route_readiness_status" -eq 0 ]]; then
  echo "remote Carrier operator report accepted stale route readiness" >&2
  cat "$tmp_dir/stale-route-readiness-result.json" >&2
  exit 1
fi

"$node_bin" - "$tmp_dir/valid-result.json" "$tmp_dir/template-result.json" "$tmp_dir/leaky-result.json" "$tmp_dir/missing-artifact-result.json" "$tmp_dir/missing-artifact-digest-result.json" "$tmp_dir/missing-endpoint-result.json" "$tmp_dir/same-endpoint-result.json" "$tmp_dir/swapped-roles-result.json" "$tmp_dir/missing-principal-result.json" "$tmp_dir/weak-evidence-result.json" "$tmp_dir/local-artifact-hash-mismatch-result.json" "$tmp_dir/local-artifact-secret-leak-result.json" "$tmp_dir/local-browser-proof-target-mismatch-result.json" "$tmp_dir/stale-artifact-readiness-result.json" "$tmp_dir/stale-route-readiness-result.json" <<'NODE'
const fs = require("node:fs");
const [
  validPath,
  templatePath,
  leakyPath,
  missingArtifactPath,
  missingArtifactDigestPath,
  missingEndpointPath,
  sameEndpointPath,
  swappedRolesPath,
  missingPrincipalPath,
  weakEvidencePath,
  localArtifactHashMismatchPath,
  localArtifactSecretLeakPath,
  localBrowserProofTargetMismatchPath,
  staleArtifactReadinessPath,
  staleRouteReadinessPath,
] = process.argv.slice(2);
const valid = JSON.parse(fs.readFileSync(validPath, "utf8"));
const template = JSON.parse(fs.readFileSync(templatePath, "utf8"));
const leaky = JSON.parse(fs.readFileSync(leakyPath, "utf8"));
const missingArtifact = JSON.parse(fs.readFileSync(missingArtifactPath, "utf8"));
const missingArtifactDigest = JSON.parse(fs.readFileSync(missingArtifactDigestPath, "utf8"));
const missingEndpoint = JSON.parse(fs.readFileSync(missingEndpointPath, "utf8"));
const sameEndpoint = JSON.parse(fs.readFileSync(sameEndpointPath, "utf8"));
const swappedRoles = JSON.parse(fs.readFileSync(swappedRolesPath, "utf8"));
const missingPrincipal = JSON.parse(fs.readFileSync(missingPrincipalPath, "utf8"));
const weakEvidence = JSON.parse(fs.readFileSync(weakEvidencePath, "utf8"));
const localArtifactHashMismatch = JSON.parse(fs.readFileSync(localArtifactHashMismatchPath, "utf8"));
const localArtifactSecretLeak = JSON.parse(fs.readFileSync(localArtifactSecretLeakPath, "utf8"));
const localBrowserProofTargetMismatch = JSON.parse(fs.readFileSync(localBrowserProofTargetMismatchPath, "utf8"));
const staleArtifactReadiness = JSON.parse(fs.readFileSync(staleArtifactReadinessPath, "utf8"));
const staleRouteReadiness = JSON.parse(fs.readFileSync(staleRouteReadinessPath, "utf8"));
if (valid.ok !== true ||
    valid.required_checks.length !== 13 ||
    valid.required_artifacts.length !== 6 ||
    !valid.required_checks.includes("installed_artifact_readiness_observed") ||
    !valid.required_checks.includes("route_readiness_observed") ||
    !valid.required_artifacts.includes("installed_artifact_readiness") ||
    !valid.required_artifacts.includes("route_readiness")) {
  throw new Error("valid remote Carrier operator evidence was not accepted");
}
if (template.ok !== false || !template.errors.some((error) => error.includes("ok must be true"))) {
  throw new Error("unreviewed template rejection did not explain ok=true requirement");
}
if (leaky.ok !== false || !leaky.errors.some((error) => error.includes("must not contain connect_ticket"))) {
  throw new Error("leaky evidence rejection did not explain private route material");
}
if (missingArtifact.ok !== false || !missingArtifact.errors.some((error) => error.includes("artifacts.browser_machine_proof must be an object with path and sha256"))) {
  throw new Error("missing artifact rejection did not explain the required browser proof artifact");
}
if (missingArtifactDigest.ok !== false || !missingArtifactDigest.errors.some((error) => error.includes("artifacts.browser_machine_proof.sha256 must be a 64-character hex SHA-256 digest"))) {
  throw new Error("missing artifact digest rejection did not explain the required browser proof digest");
}
if (missingEndpoint.ok !== false || !missingEndpoint.errors.some((error) => error.includes("exit_runtime.endpoint is required"))) {
  throw new Error("missing endpoint rejection did not explain the required remote runtime endpoint");
}
if (sameEndpoint.ok !== false || !sameEndpoint.errors.some((error) => error.includes("source_runtime.endpoint and exit_runtime.endpoint must be distinct"))) {
  throw new Error("same endpoint rejection did not explain the distinct runtime endpoint requirement");
}
if (swappedRoles.ok !== false || !swappedRoles.errors.some((error) => error.includes("source_runtime.role must be browser-source"))) {
  throw new Error("swapped role rejection did not explain the source runtime role requirement");
}
if (missingPrincipal.ok !== false || !missingPrincipal.errors.some((error) => error.includes("route.principal is required"))) {
  throw new Error("missing principal rejection did not explain the route principal requirement");
}
if (weakEvidence.ok !== false || !weakEvidence.errors.some((error) => error.includes("evidence.remote_exit_discovery_observed must cite route.principal and route.grant_id"))) {
  throw new Error("weak evidence rejection did not explain route-bound discovery evidence");
}
if (!weakEvidence.errors.some((error) => error.includes("evidence.two_runtimes_distinct must cite source_runtime.did and exit_runtime.did"))) {
  throw new Error("weak evidence rejection did not explain DID-bound runtime evidence");
}
if (!weakEvidence.errors.some((error) => error.includes("evidence.installed_artifact_readiness_observed must cite artifact readiness and browser_exit_stream"))) {
  throw new Error("weak evidence rejection did not explain artifact-readiness evidence");
}
if (!weakEvidence.errors.some((error) => error.includes("evidence.route_readiness_observed must cite route readiness and config_sha256"))) {
  throw new Error("weak evidence rejection did not explain hash-bound route-readiness evidence");
}
if (!weakEvidence.errors.some((error) => error.includes("evidence.route_readiness_observed must cite route.principal, route.grant_id, and route.target"))) {
  throw new Error("weak evidence rejection did not explain route-bound readiness evidence");
}
if (!weakEvidence.errors.some((error) => error.includes("evidence.two_runtimes_distinct must cite source_runtime.endpoint and exit_runtime.endpoint"))) {
  throw new Error("weak evidence rejection did not explain endpoint-bound runtime evidence");
}
if (!weakEvidence.errors.some((error) => error.includes("evidence.policy_target_allowlist_enforced must cite route.target"))) {
  throw new Error("weak evidence rejection did not explain target-bound policy evidence");
}
if (!weakEvidence.errors.some((error) => error.includes("evidence.principal_accounting_observed must cite route.principal"))) {
  throw new Error("weak evidence rejection did not explain principal-bound accounting evidence");
}
if (!weakEvidence.errors.some((error) => error.includes("evidence.cleanup_observed must cite route.principal or route.grant_id"))) {
  throw new Error("weak evidence rejection did not explain route-bound cleanup evidence");
}
if (localArtifactHashMismatch.ok !== false || !localArtifactHashMismatch.errors.some((error) => error.includes("artifacts.source_gateway_log.sha256 must match the local redacted artifact file"))) {
  throw new Error("local artifact hash mismatch rejection did not explain the required redacted artifact digest match");
}
if (localArtifactSecretLeak.ok !== false || !localArtifactSecretLeak.errors.some((error) => error.includes("artifacts.source_gateway_log.path must point to a redacted artifact without private route material"))) {
  throw new Error("local artifact secret leak rejection did not explain the required redaction");
}
if (localBrowserProofTargetMismatch.ok !== false || !localBrowserProofTargetMismatch.errors.some((error) => error.includes("artifacts.browser_machine_proof.path must cite route.target or its host"))) {
  throw new Error("local Browser proof route-target mismatch rejection did not explain the required target binding");
}
if (staleArtifactReadiness.ok !== false ||
    !staleArtifactReadiness.errors.some((error) => error.includes("artifacts.installed_artifact_readiness.path must report ok=true")) ||
    !staleArtifactReadiness.errors.some((error) => error.includes("must prove gateway Browser Carrier stream strings"))) {
  throw new Error("stale installed-artifact readiness rejection did not explain the artifact capability requirements");
}
if (staleRouteReadiness.ok !== false ||
    !staleRouteReadiness.errors.some((error) => error.includes("artifacts.route_readiness.path must report ok=true")) ||
    !staleRouteReadiness.errors.some((error) => error.includes("artifacts.route_readiness.path route.target must match report.route.target")) ||
    !staleRouteReadiness.errors.some((error) => error.includes("artifacts.route_readiness.path must include source.config_sha256")) ||
    !staleRouteReadiness.errors.some((error) => error.includes("artifacts.route_readiness.path must prove source.remote_only=true"))) {
  throw new Error("stale route-readiness rejection did not explain the route/config requirements");
}
NODE

printf '{"schema":"elastos.remote-carrier-exit.operator-report-smoke/v1","ok":true,"template_rejected":true,"leaky_evidence_rejected":true,"missing_artifact_rejected":true,"missing_artifact_digest_rejected":true,"missing_endpoint_rejected":true,"same_endpoint_rejected":true,"swapped_roles_rejected":true,"missing_principal_rejected":true,"weak_evidence_rejected":true,"local_artifact_hash_mismatch_rejected":true,"local_artifact_secret_leak_rejected":true,"local_browser_proof_target_mismatch_rejected":true,"stale_artifact_readiness_rejected":true,"stale_route_readiness_rejected":true,"valid_report_accepted":true}\n'
