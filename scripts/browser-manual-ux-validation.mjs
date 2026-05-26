import crypto from "node:crypto";
import fs from "node:fs";

import {
  HOSTED_WEBRTC_MANUAL_CHECKS,
  requiredManualChecksForSchema,
} from "./browser-manual-ux-checks.mjs";

export function sha256File(path) {
  if (!path) return "";
  return crypto.createHash("sha256").update(fs.readFileSync(path)).digest("hex");
}

function realpath(path) {
  try {
    return fs.realpathSync(path);
  } catch {
    return "";
  }
}

export function validateManualUxReport(
  report,
  { acceptedArtifacts = [], requireAcceptedArtifact = false } = {},
) {
  const errors = [];
  if (report?.schema !== "elastos.browser.manual-ux/v1") {
    errors.push("schema must be elastos.browser.manual-ux/v1");
  }
  if (report?.ok !== true) {
    errors.push("ok must be true after manual review passes");
  }
  const requiredChecks = requiredManualChecksForSchema(report?.machine_artifact?.schema);
  const checkKeys =
    report?.checks && typeof report.checks === "object" && !Array.isArray(report.checks)
      ? Object.keys(report.checks)
      : [];
  const evidenceKeys =
    report?.evidence && typeof report.evidence === "object" && !Array.isArray(report.evidence)
      ? Object.keys(report.evidence)
      : [];
  for (const name of checkKeys) {
    if (!requiredChecks.includes(name)) {
      errors.push(`checks.${name} is not valid for machine_artifact.schema`);
    }
  }
  for (const name of evidenceKeys) {
    if (!requiredChecks.includes(name)) {
      errors.push(`evidence.${name} is not valid for machine_artifact.schema`);
    }
  }
  for (const name of requiredChecks) {
    if (report?.checks?.[name] !== true) {
      errors.push(`checks.${name} must be true`);
    }
  }
  if (report?.machine_artifact?.schema === "elastos.browser.hosted-provider-bakeoff/v1") {
    for (const name of HOSTED_WEBRTC_MANUAL_CHECKS) {
      if (typeof report.evidence?.[name] !== "string" || report.evidence[name].trim().length === 0) {
        errors.push(`evidence.${name} must describe the observed hosted WebRTC audio proof`);
      }
    }
  }
  for (const field of ["reviewed_at", "reviewer", "provider", "target"]) {
    if (typeof report?.[field] !== "string" || report[field].trim().length === 0) {
      errors.push(`${field} must be a non-empty string`);
    }
  }

  const artifact = report?.machine_artifact;
  let artifactPath = "";
  if (
    ![
      "elastos.browser.hosted-provider-bakeoff/v1",
      "elastos.browser.native-target-preflight/v1",
    ].includes(artifact?.schema)
  ) {
    errors.push("machine_artifact.schema must identify the accepted hosted bake-off or native preflight schema");
  }
  if (typeof artifact?.sha256 !== "string" || !/^[a-f0-9]{64}$/i.test(artifact.sha256)) {
    errors.push("machine_artifact.sha256 must be a 64-character hex SHA-256 digest");
  }
  if (typeof artifact?.path !== "string" || artifact.path.trim().length === 0) {
    errors.push("machine_artifact.path must be a non-empty string");
  } else if (!fs.existsSync(artifact.path)) {
    errors.push("machine_artifact.path must point to the reviewed machine artifact JSON");
  } else if (sha256File(artifact.path).toLowerCase() !== String(artifact.sha256 || "").toLowerCase()) {
    errors.push("machine_artifact.sha256 must match machine_artifact.path");
  } else {
    try {
      const artifactFile = JSON.parse(fs.readFileSync(artifact.path, "utf8"));
      if (artifactFile.schema !== artifact.schema) {
        errors.push("machine_artifact.schema must match machine_artifact.path");
      }
      if (artifactFile.ok !== true) {
        errors.push("machine_artifact.path must point to a successful machine artifact");
      }
      artifactPath = realpath(artifact.path);
    } catch {
      errors.push("machine_artifact.path must be valid JSON");
    }
  }

  if (
    requireAcceptedArtifact &&
    !acceptedArtifacts.some(
      (accepted) =>
        accepted.schema === artifact?.schema &&
        accepted.sha256.toLowerCase() === String(artifact?.sha256 || "").toLowerCase() &&
        realpath(accepted.path) === artifactPath,
    )
  ) {
    errors.push("machine_artifact must match an accepted machine artifact passed to the objective audit");
  }

  if (errors.length > 0) {
    return {
      ok: false,
      schema: "elastos.browser.manual-ux.validation/v1",
      errors,
    };
  }
  return {
    ok: true,
    schema: "elastos.browser.manual-ux.validation/v1",
    provider: report.provider,
    target: report.target,
    machine_artifact: artifact,
  };
}
