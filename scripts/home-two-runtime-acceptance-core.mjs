import {
  closeSync,
  constants,
  fstatSync,
  openSync,
  readFileSync,
} from "node:fs";
import { isAbsolute, relative, resolve } from "node:path";

export const REQUIRED_ACCEPTANCE_LEGS = Object.freeze([
  "provisioning_and_sign_in",
  "distinct_runtime_instances",
  "fresh_fixture_precondition",
  "system_recovery_before_profile",
  "distinct_profile_names",
  "overlapping_opt_in_discovery",
  "exactly_one_contact_request",
  "inbox_only_accept",
  "stable_contacts",
  "distinct_profile_identities",
  "direct_message_a_to_b",
  "direct_message_b_to_a",
  "rename_propagation",
  "bilateral_removal",
  "re_add_contact",
  "shared_room_before_restart",
  "both_runtime_restart",
  "direct_history_after_restart",
  "shared_room_after_restart",
  "identity_scan_people_a",
  "identity_scan_people_b",
  "identity_scan_chat_a",
  "identity_scan_chat_b",
]);

const REQUIRED_ENV_KEYS = Object.freeze([
  "ELASTOS_A_BASE_URL",
  "ELASTOS_A_PROFILE",
  "ELASTOS_B_BASE_URL",
  "ELASTOS_B_PROFILE",
  "ELASTOS_A_RESTART_CMD",
  "ELASTOS_B_RESTART_CMD",
  "ELASTOS_A_FIXTURE_MANIFEST",
  "ELASTOS_B_FIXTURE_MANIFEST",
]);

const RAW_IDENTITY_RE = /(?:did:(?:key|elastos):|\bz6Mk[1-9A-HJ-NP-Za-km-z]{20,}|\b(?:ElastOS user|ElastOS Home|Person)\b|\b(?:device(?:\s+did)?|peer did|carrier|connect ticket|route)\b)/i;

export class AcceptanceEvidenceError extends Error {}

function exactObjectKeys(value, keys, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new AcceptanceEvidenceError(`${label} must be an object`);
  }
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  if (actual.length !== expected.length || actual.some((key, index) => key !== expected[index])) {
    throw new AcceptanceEvidenceError(`${label} has an unsupported shape`);
  }
}

function boundedId(value, label) {
  if (
    typeof value !== "string"
    || !/^[A-Za-z0-9._:-]{1,128}$/.test(value)
  ) {
    throw new AcceptanceEvidenceError(`${label} is invalid`);
  }
  return value;
}

function isDidKey(value) {
  return typeof value === "string"
    && /^did:key:z[1-9A-HJ-NP-Za-km-z]{16,200}$/.test(value);
}

function ownerOnlyJson(path, label) {
  if (!isAbsolute(path)) {
    throw new AcceptanceEvidenceError(`${label} path must be absolute`);
  }
  let metadata;
  let bytes;
  let file;
  try {
    file = openSync(path, constants.O_RDONLY | (constants.O_NOFOLLOW || 0));
    metadata = fstatSync(file);
    if (!metadata.isFile()) {
      throw new AcceptanceEvidenceError(`${label} must be a regular non-symlink file`);
    }
    if ((metadata.mode & 0o077) !== 0) {
      throw new AcceptanceEvidenceError(`${label} must be owner-only`);
    }
    if (typeof process.getuid === "function" && metadata.uid !== process.getuid()) {
      throw new AcceptanceEvidenceError(`${label} must be owned by the current operator`);
    }
    bytes = readFileSync(file, "utf8");
  } catch (error) {
    if (error instanceof AcceptanceEvidenceError) {
      throw error;
    }
    throw new AcceptanceEvidenceError(`${label} is unavailable`);
  } finally {
    if (file !== undefined) {
      closeSync(file);
    }
  }
  if (!bytes || Buffer.byteLength(bytes) > 16_384) {
    throw new AcceptanceEvidenceError(`${label} has invalid size`);
  }
  try {
    return JSON.parse(bytes);
  } catch {
    throw new AcceptanceEvidenceError(`${label} is malformed`);
  }
}

function requiredEnv(env, key) {
  const value = typeof env[key] === "string" ? env[key].trim() : "";
  if (!value) {
    throw new AcceptanceEvidenceError(`${key} is required`);
  }
  return value;
}

function localBaseUrl(raw, key) {
  let url;
  try {
    url = new URL(raw);
  } catch {
    throw new AcceptanceEvidenceError(`${key} must be a valid local origin`);
  }
  const isLoopback = url.hostname === "localhost"
    || url.hostname === "127.0.0.1"
    || url.hostname === "::1";
  if (
    !isLoopback
    || !["http:", "https:"].includes(url.protocol)
    || url.username
    || url.password
    || url.pathname !== "/"
    || url.search
    || url.hash
  ) {
    throw new AcceptanceEvidenceError(
      `${key} must be an explicit loopback origin for a fixture-owned Home`,
    );
  }
  return url.origin;
}

function absolutePath(raw, key) {
  if (!isAbsolute(raw)) {
    throw new AcceptanceEvidenceError(`${key} must be an explicit absolute path`);
  }
  return resolve(raw);
}

function fixtureManifest(path, side) {
  const label = `${side} fixture manifest`;
  const value = ownerOnlyJson(path, label);
  exactObjectKeys(value, [
    "schema",
    "fixture_id",
    "origin",
    "browser_profile",
    "data_root",
    "restart_receipt",
    "expected_device_did",
  ], label);
  if (value.schema !== "elastos.home.acceptance-fixture/v1") {
    throw new AcceptanceEvidenceError(`${label} has an unsupported schema`);
  }
  const manifest = {
    fixtureId: boundedId(value.fixture_id, `${label} fixture_id`),
    origin: localBaseUrl(value.origin, `${label} origin`),
    browserProfile: absolutePath(value.browser_profile, `${label} browser_profile`),
    dataRoot: absolutePath(value.data_root, `${label} data_root`),
    restartReceipt: absolutePath(value.restart_receipt, `${label} restart_receipt`),
    expectedDeviceDid: typeof value.expected_device_did === "string"
      ? value.expected_device_did.trim()
      : "",
  };
  if (!isDidKey(manifest.expectedDeviceDid)) {
    throw new AcceptanceEvidenceError(`${label} expected_device_did is invalid`);
  }
  return manifest;
}

function isPathInsideRoot(root, path) {
  const relativePath = relative(root, path);
  return relativePath === ""
    || (!relativePath.startsWith("..") && !isAbsolute(relativePath));
}

export function loadAcceptanceConfig(env) {
  const values = Object.fromEntries(REQUIRED_ENV_KEYS.map((key) => [key, requiredEnv(env, key)]));
  const a = {
    prefix: "A",
    base: localBaseUrl(values.ELASTOS_A_BASE_URL, "ELASTOS_A_BASE_URL"),
    profile: absolutePath(values.ELASTOS_A_PROFILE, "ELASTOS_A_PROFILE"),
    restartCmd: values.ELASTOS_A_RESTART_CMD,
    name: (env.ELASTOS_A_NAME || "Alma Acceptance").trim(),
    fixture: fixtureManifest(values.ELASTOS_A_FIXTURE_MANIFEST, "A"),
  };
  const b = {
    prefix: "B",
    base: localBaseUrl(values.ELASTOS_B_BASE_URL, "ELASTOS_B_BASE_URL"),
    profile: absolutePath(values.ELASTOS_B_PROFILE, "ELASTOS_B_PROFILE"),
    restartCmd: values.ELASTOS_B_RESTART_CMD,
    name: (env.ELASTOS_B_NAME || "Bruno Acceptance").trim(),
    fixture: fixtureManifest(values.ELASTOS_B_FIXTURE_MANIFEST, "B"),
  };
  if (a.base === b.base) {
    throw new AcceptanceEvidenceError("A and B must use distinct fixture Home origins");
  }
  if (a.profile === b.profile) {
    throw new AcceptanceEvidenceError("A and B must use distinct browser profile paths");
  }
  if (!a.name || !b.name || a.name === b.name) {
    throw new AcceptanceEvidenceError("A and B must use distinct nonempty Profile names");
  }
  for (const side of [a, b]) {
    if (side.base !== side.fixture.origin || side.profile !== side.fixture.browserProfile) {
      throw new AcceptanceEvidenceError(`${side.prefix} fixture manifest does not bind the configured origin and browser profile`);
    }
  }
  if (
    a.fixture.fixtureId === b.fixture.fixtureId
    || a.fixture.dataRoot === b.fixture.dataRoot
    || a.fixture.restartReceipt === b.fixture.restartReceipt
    || a.fixture.expectedDeviceDid === b.fixture.expectedDeviceDid
  ) {
    throw new AcceptanceEvidenceError("A and B fixture manifests must bind distinct fixtures, data roots, receipts, and devices");
  }
  loadRestartReceipt(a);
  loadRestartReceipt(b);
  return { a, b };
}

export function loadRestartReceipt(side) {
  const label = `${side.prefix} restart receipt`;
  const value = ownerOnlyJson(side.fixture.restartReceipt, label);
  exactObjectKeys(value, [
    "schema",
    "fixture_id",
    "device_did",
    "process_instance_id",
  ], label);
  if (value.schema !== "elastos.home.acceptance-fixture-restart/v1") {
    throw new AcceptanceEvidenceError(`${label} has an unsupported schema`);
  }
  const receipt = {
    fixtureId: boundedId(value.fixture_id, `${label} fixture_id`),
    deviceDid: typeof value.device_did === "string" ? value.device_did.trim() : "",
    processInstanceId: boundedId(value.process_instance_id, `${label} process_instance_id`),
  };
  if (
    receipt.fixtureId !== side.fixture.fixtureId
    || receipt.deviceDid !== side.fixture.expectedDeviceDid
  ) {
    throw new AcceptanceEvidenceError(`${label} does not match its fixture manifest`);
  }
  return receipt;
}

export function assertDistinctRuntimeEvidence(aDeviceDid, bDeviceDid, config) {
  if (
    typeof aDeviceDid !== "string"
    || typeof bDeviceDid !== "string"
    || !aDeviceDid
    || !bDeviceDid
    || aDeviceDid === bDeviceDid
    || aDeviceDid !== config.a.fixture.expectedDeviceDid
    || bDeviceDid !== config.b.fixture.expectedDeviceDid
  ) {
    throw new AcceptanceEvidenceError("System summaries do not prove two distinct fixture Runtimes");
  }
}

export function assertDistinctProfileContactEvidence(aContactId, bContactId) {
  if (
    typeof aContactId !== "string"
    || typeof bContactId !== "string"
    || !aContactId.startsWith("contact:")
    || !bContactId.startsWith("contact:")
    || aContactId === bContactId
  ) {
    throw new AcceptanceEvidenceError("opaque contact projections do not prove distinct Profile identities");
  }
}

export function assertRestartTransition({ before, after, side, systemDeviceDid }) {
  if (
    before.fixtureId !== side.fixture.fixtureId
    || after.fixtureId !== side.fixture.fixtureId
    || before.deviceDid !== side.fixture.expectedDeviceDid
    || after.deviceDid !== side.fixture.expectedDeviceDid
    || systemDeviceDid !== side.fixture.expectedDeviceDid
    || before.processInstanceId === after.processInstanceId
  ) {
    throw new AcceptanceEvidenceError(`${side.prefix} restart receipt does not prove a stable-device process restart`);
  }
  return {
    fixture_id: side.fixture.fixtureId,
    before_process_instance_id: before.processInstanceId,
    after_process_instance_id: after.processInstanceId,
    device_did: systemDeviceDid,
  };
}

export function createAcceptanceReport(config) {
  return {
    schema: "elastos.home.two-runtime-acceptance/v2",
    ok: false,
    sides: {
      a: { base: config.a.base, name: config.a.name },
      b: { base: config.b.base, name: config.b.name },
    },
    results: [],
  };
}

export function recordAcceptancePass(report, leg, evidence = {}) {
  if (!REQUIRED_ACCEPTANCE_LEGS.includes(leg)) {
    throw new AcceptanceEvidenceError(`unknown acceptance leg: ${leg}`);
  }
  if (report.results.some((result) => result.leg === leg)) {
    throw new AcceptanceEvidenceError(`acceptance leg was recorded twice: ${leg}`);
  }
  report.results.push({ leg, status: "passed", evidence });
}

export function finalizeAcceptanceReport(report) {
  report.ok = false;
  const byLeg = new Map();
  for (const result of report.results) {
    if (!result || typeof result.leg !== "string" || byLeg.has(result.leg)) {
      throw new AcceptanceEvidenceError("acceptance report has an ambiguous result");
    }
    byLeg.set(result.leg, result);
  }
  const missing = REQUIRED_ACCEPTANCE_LEGS.filter((leg) => !byLeg.has(leg));
  const nonPassing = REQUIRED_ACCEPTANCE_LEGS.filter((leg) => byLeg.get(leg)?.status !== "passed");
  if (missing.length || nonPassing.length || byLeg.size !== REQUIRED_ACCEPTANCE_LEGS.length) {
    throw new AcceptanceEvidenceError(
      `acceptance report is incomplete (missing=${missing.join(",") || "none"}; nonpassing=${nonPassing.join(",") || "none"})`,
    );
  }
  report.ok = true;
  return report;
}

export function assertFreshFixturePrecondition(aContacts, bContacts) {
  if (!Array.isArray(aContacts) || !Array.isArray(bContacts)) {
    throw new AcceptanceEvidenceError("fresh fixture contact projections are unavailable");
  }
  if (aContacts.length || bContacts.length) {
    throw new AcceptanceEvidenceError(
      "pre-existing contact state is not valid acceptance evidence; use fresh fixture-owned Homes",
    );
  }
}

export function assertRecoverySetupEvidence(config, evidence) {
  exactObjectKeys(evidence, ["a", "b"], "recovery setup evidence");
  const validateSide = (side, summary) => {
    exactObjectKeys(summary, [
      "download_count",
      "download_path",
      "before_status",
      "blocked_status",
      "after_status",
    ], `${side.prefix} recovery evidence`);
    const downloadPath = absolutePath(
      summary.download_path,
      `${side.prefix} recovery evidence download_path`,
    );
    if (
      summary.download_count !== 1
      || summary.before_status !== "setup_required"
      || summary.blocked_status !== "recovery_required"
      || summary.after_status !== "setup_required"
      || !isPathInsideRoot(side.fixture.dataRoot, downloadPath)
    ) {
      throw new AcceptanceEvidenceError(
        `${side.prefix} recovery evidence is not a single verified fixture-owned Recovery setup`,
      );
    }
    return {
      download_count: summary.download_count,
      download_path: downloadPath,
      before_status: summary.before_status,
      blocked_status: summary.blocked_status,
      after_status: summary.after_status,
    };
  };
  const a = validateSide(config.a, evidence.a);
  const b = validateSide(config.b, evidence.b);
  return { a, b };
}

export function assertExactDirectConversation({
  expectedConversationId,
  availableConversationIds,
  selectedConversationId,
  chatMode,
}) {
  if (typeof expectedConversationId !== "string" || !expectedConversationId) {
    throw new AcceptanceEvidenceError("the accepted contact has no opaque direct conversation id");
  }
  const matches = availableConversationIds.filter((value) => value === expectedConversationId);
  if (
    matches.length !== 1
    || selectedConversationId !== expectedConversationId
    || chatMode !== "direct"
  ) {
    throw new AcceptanceEvidenceError("the exact accepted-contact conversation is not selected");
  }
}

export function assertIdentityFrame({ baseUrl, target, frameUrl, text }) {
  if (!new Set(["people", "chat-room"]).has(target)) {
    throw new AcceptanceEvidenceError(`unsupported identity scan target: ${target}`);
  }
  let expectedOrigin;
  let actual;
  try {
    expectedOrigin = new URL(baseUrl).origin;
    actual = new URL(frameUrl);
  } catch {
    throw new AcceptanceEvidenceError(`${target} identity scan did not receive a valid frame URL`);
  }
  if (actual.origin !== expectedOrigin || !actual.pathname.startsWith(`/apps/${target}/`)) {
    throw new AcceptanceEvidenceError(`${target} identity scan received the wrong nested frame`);
  }
  const visibleText = typeof text === "string" ? text.replace(/\s+/g, " ").trim() : "";
  if (!visibleText) {
    throw new AcceptanceEvidenceError(`${target} identity scan received an empty frame`);
  }
  const match = visibleText.match(RAW_IDENTITY_RE)?.[0];
  if (match) {
    throw new AcceptanceEvidenceError(`${target} exposed raw or fallback identity: ${match}`);
  }
  return { characters: visibleText.length };
}
