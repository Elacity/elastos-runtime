#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import process from "node:process";
import { pathToFileURL } from "node:url";

const SCHEMA = "elastos.home-shell.manual-ux/v1";
const REQUIRED_CHECKS = Object.freeze([
  "signed_in_with_passkey",
  "system_shell_picker_switches_to_home_cli",
  "home_cli_owns_full_viewport",
  "no_home_gui_chrome_bleed_through",
  "no_desktop_first_paint_before_cli",
  "switch_back_to_home_gui",
  "home_cli_hides_gui_only_browser_from_default_menu",
  "reload_does_not_enter_passkey_loop",
]);
const REQUIRED_ARTIFACT_KINDS = new Set(["screenshot", "screen_recording", "manual_notes"]);
const REDACTED_SECRET_PATTERNS = Object.freeze([
  /home_token=/i,
  /"home_token"\s*:/i,
  /x-elastos-home-token/i,
  /authorization\s*[:=]/i,
  /set-cookie\s*[:=]/i,
  /\bcookie\s*[:=]/i,
  /bearer\s+[a-z0-9._~+/=-]{12,}/i,
  /person:local:[a-z0-9]{8,}/i,
  /did:key:[a-z0-9]{8,}/i,
]);

function usage() {
  console.error(`Usage:
  node scripts/home-shell-manual-ux-report.mjs --template
  node scripts/home-shell-manual-ux-report.mjs --template --out /tmp/home-shell-manual-ux.json
  node scripts/home-shell-manual-ux-report.mjs --notes-template --out /tmp/home-shell-manual-notes.md
  node scripts/home-shell-manual-ux-report.mjs --artifact-entry /tmp/home-shell-manual-notes.md
  node scripts/home-shell-manual-ux-report.mjs --report-from-notes /tmp/home-shell-manual-notes.md --out /tmp/home-shell-manual-ux.json
  node scripts/home-shell-manual-ux-report.mjs --input /tmp/home-shell-manual-ux.json
  node scripts/home-shell-manual-ux-report.mjs --self-test

Creates or validates the operator-profile manual UX evidence for the Home shell
switch. The template is intentionally not accepted until a human fills every
check and attaches at least one redacted, hash-bound review artifact.
The notes template is a screen-capture-free artifact option; review it and set
redacted=true in the report only after confirming it contains no secrets.
The notes-to-report path stays fail-closed: it only writes a report when every
required check is filled and the notes file says it was reviewed for secrets.
`);
}

function parseArgs(argv) {
  const args = {
    template: false,
    notesTemplate: false,
    artifactEntry: "",
    reportFromNotes: "",
    input: "",
    out: "",
    selfTest: false,
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
    } else if (arg === "--template") {
      args.template = true;
    } else if (arg === "--notes-template") {
      args.notesTemplate = true;
    } else if (arg === "--artifact-entry") {
      args.artifactEntry = next();
    } else if (arg === "--report-from-notes") {
      args.reportFromNotes = next();
    } else if (arg === "--input") {
      args.input = next();
    } else if (arg === "--out") {
      args.out = next();
    } else if (arg === "--self-test") {
      args.selfTest = true;
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }
  const modes = [
    args.template,
    args.notesTemplate,
    Boolean(args.artifactEntry),
    Boolean(args.reportFromNotes),
    Boolean(args.input),
    args.selfTest,
  ].filter(Boolean).length;
  if (modes !== 1) {
    throw new Error(
      "use exactly one of --template, --notes-template, --artifact-entry, --report-from-notes, --input, or --self-test",
    );
  }
  if (args.out && !args.template && !args.notesTemplate && !args.reportFromNotes) {
    throw new Error("--out is only valid with --template, --notes-template, or --report-from-notes");
  }
  if (args.reportFromNotes && !args.out) {
    throw new Error("--report-from-notes requires --out");
  }
  return args;
}

function gitValue(args) {
  try {
    return execFileSync("git", args, { encoding: "utf8" }).trim();
  } catch (_error) {
    return "";
  }
}

function template() {
  return {
    schema: SCHEMA,
    ok: false,
    generated_at: new Date().toISOString(),
    reviewed_at: new Date(0).toISOString(),
    reviewer: "",
    source: {
      branch: gitValue(["branch", "--show-current"]),
      commit: gitValue(["rev-parse", "HEAD"]),
      home_url: process.env.HOME_URL || "http://localhost:61180/apps/home/",
    },
    operator_profile: {
      kind: "human-operator-browser-profile",
      passkey_sign_in: false,
      notes: "",
    },
    checks: Object.fromEntries(
      REQUIRED_CHECKS.map((name) => [name, { ok: false, evidence: "" }]),
    ),
    review_artifacts: [
      {
        kind: "",
        path: "",
        sha256: "",
        redacted: false,
        description: "",
      },
    ],
    notes: [
      "Set ok=true only after testing on the real operator profile in a browser.",
      "Do not paste Home launch tokens, session headers, raw DIDs, or local person ids.",
      "Use a redacted screenshot, screen recording, or manual-notes artifact and record its SHA-256.",
      "This receipt does not replace the automated Home shell smokes.",
    ],
  };
}

function notesTemplate() {
  const branch = gitValue(["branch", "--show-current"]);
  const commit = gitValue(["rev-parse", "HEAD"]);
  const homeUrl = process.env.HOME_URL || "http://localhost:61180/apps/home/";
  return `# Home Shell Manual UX Notes

schema: elastos.home-shell.manual-notes/v1
reviewed_at:
reviewer:
source_branch: ${branch}
source_commit: ${commit}
home_url: ${homeUrl}

Do not include Home launch tokens, cookies, raw DIDs, local person ids, wallet
secrets, screenshots with private content, or browser/session headers.

## Operator Profile

- Browser:
- Passkey sign-in completed: no
- Profile notes:

## Required Checks

- signed_in_with_passkey:
  evidence:

- system_shell_picker_switches_to_home_cli:
  evidence:

- home_cli_owns_full_viewport:
  evidence:

- no_home_gui_chrome_bleed_through:
  evidence:

- no_desktop_first_paint_before_cli:
  evidence:

- switch_back_to_home_gui:
  evidence:

- home_cli_hides_gui_only_browser_from_default_menu:
  evidence:

- reload_does_not_enter_passkey_loop:
  evidence:

## Artifact Review

- I reviewed this note file for secrets before hashing it: no
- Suggested report artifact kind: manual_notes
`;
}

function sha256File(path) {
  const bytes = readFileSync(path);
  return createHash("sha256").update(bytes).digest("hex");
}

function artifactEntry(path) {
  return {
    kind: "manual_notes",
    path,
    sha256: sha256File(path),
    redacted: false,
    description: "Redacted Home shell operator-profile manual UX notes.",
    note: "Set redacted=true in the report only after reviewing the file for secrets.",
  };
}

function acceptedYes(value) {
  return /^(yes|true|ok|done|complete)$/i.test(String(value || "").trim());
}

function escapeRegExp(value) {
  return String(value).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function topLevelValue(notes, key) {
  const match = notes.match(new RegExp(`^${escapeRegExp(key)}:\\s*(.*)$`, "m"));
  return match ? match[1].trim() : "";
}

function bulletValue(notes, label) {
  const match = notes.match(new RegExp(`^-\\s+${escapeRegExp(label)}:\\s*(.*)$`, "mi"));
  return match ? match[1].trim() : "";
}

function sectionText(notes, heading) {
  const headingMatch = new RegExp(`^##\\s+${escapeRegExp(heading)}\\s*$`, "m").exec(notes);
  if (!headingMatch) {
    return "";
  }
  const rest = notes.slice(headingMatch.index + headingMatch[0].length);
  const nextHeading = rest.search(/\n##\s+/);
  return nextHeading >= 0 ? rest.slice(0, nextHeading) : rest;
}

function parseCheckEvidence(notes) {
  const section = sectionText(notes, "Required Checks");
  const result = new Map();
  let current = "";
  let collecting = false;
  let buffer = [];
  const finish = () => {
    if (current) {
      result.set(current, buffer.join(" ").replace(/\s+/g, " ").trim());
    }
    buffer = [];
    collecting = false;
  };
  for (const line of section.split(/\r?\n/)) {
    const check = line.match(/^- ([a-z0-9_]+):\s*(.*)$/);
    if (check) {
      finish();
      current = check[1];
      const rest = check[2].trim();
      if (rest && !rest.startsWith("evidence:")) {
        buffer.push(rest);
      }
      continue;
    }
    const evidence = line.match(/^\s*evidence:\s*(.*)$/);
    if (evidence) {
      collecting = true;
      if (evidence[1].trim()) {
        buffer.push(evidence[1].trim());
      }
      continue;
    }
    if (collecting && line.trim()) {
      buffer.push(line.trim());
    }
  }
  finish();
  return result;
}

function assertNoSensitiveNotes(notes) {
  const errors = [];
  assertNoSensitiveText({ notes }, errors);
  if (errors.length > 0) {
    throw new Error(errors.join("; "));
  }
}

function reportFromNotes(path) {
  const notes = readFileSync(path, "utf8");
  assertNoSensitiveNotes(notes);
  const checks = parseCheckEvidence(notes);
  const report = template();
  report.ok = true;
  report.reviewed_at = topLevelValue(notes, "reviewed_at");
  report.reviewer = topLevelValue(notes, "reviewer");
  report.source.branch = topLevelValue(notes, "source_branch");
  report.source.commit = topLevelValue(notes, "source_commit");
  report.source.home_url = topLevelValue(notes, "home_url");
  report.operator_profile.passkey_sign_in =
    acceptedYes(bulletValue(notes, "Passkey sign-in completed"));
  report.operator_profile.notes = [
    `Browser: ${bulletValue(notes, "Browser")}`,
    bulletValue(notes, "Profile notes"),
  ].filter((value) => hasText(value, 1)).join("; ");
  report.checks = Object.fromEntries(
    REQUIRED_CHECKS.map((name) => [
      name,
      {
        ok: hasText(checks.get(name)),
        evidence: checks.get(name) || "",
      },
    ]),
  );
  const artifact = artifactEntry(path);
  delete artifact.note;
  artifact.redacted = acceptedYes(
    bulletValue(notes, "I reviewed this note file for secrets before hashing it"),
  );
  report.review_artifacts = [artifact];
  const validation = validateHomeShellManualUxReport(report);
  if (!validation.ok) {
    throw new Error(`manual notes are incomplete: ${validation.errors.join("; ")}`);
  }
  return report;
}

function pushError(errors, message) {
  errors.push(message);
}

function hasText(value, min = 8) {
  return typeof value === "string" && value.trim().length >= min;
}

function validIsoDate(value) {
  if (!hasText(value, 10)) {
    return false;
  }
  const time = Date.parse(value);
  return Number.isFinite(time) && time > 0;
}

function assertNoSensitiveText(report, errors) {
  const serialized = JSON.stringify(report);
  for (const pattern of REDACTED_SECRET_PATTERNS) {
    if (pattern.test(serialized)) {
      pushError(errors, `report contains unredacted sensitive text matching ${pattern}`);
    }
  }
}

function validateReviewArtifacts(report, errors) {
  const artifacts = Array.isArray(report.review_artifacts) ? report.review_artifacts : [];
  const accepted = artifacts.filter((artifact) => {
    const kind = String(artifact?.kind || "");
    return (
      REQUIRED_ARTIFACT_KINDS.has(kind) &&
      hasText(artifact.path) &&
      /^[a-f0-9]{64}$/i.test(String(artifact.sha256 || "")) &&
      artifact.redacted === true &&
      hasText(artifact.description)
    );
  });
  if (accepted.length < 1) {
    pushError(
      errors,
      "at least one redacted review_artifacts entry with kind/path/sha256/description is required",
    );
  }
}

export function validateHomeShellManualUxReport(report) {
  const errors = [];
  if (!report || typeof report !== "object") {
    return { schema: SCHEMA, ok: false, errors: ["report must be a JSON object"] };
  }
  if (report.schema !== SCHEMA) {
    pushError(errors, `schema must be ${SCHEMA}`);
  }
  if (report.ok !== true) {
    pushError(errors, "ok must be true after the human review is complete");
  }
  if (!validIsoDate(report.reviewed_at) || report.reviewed_at === new Date(0).toISOString()) {
    pushError(errors, "reviewed_at must be the real human review timestamp");
  }
  if (!hasText(report.reviewer, 2)) {
    pushError(errors, "reviewer is required");
  }
  if (!hasText(report.source?.commit, 7)) {
    pushError(errors, "source.commit is required");
  }
  if (!hasText(report.source?.home_url, 12) || !String(report.source.home_url).includes("localhost")) {
    pushError(errors, "source.home_url must record the localhost Home URL reviewed");
  }
  if (report.operator_profile?.kind !== "human-operator-browser-profile") {
    pushError(errors, "operator_profile.kind must be human-operator-browser-profile");
  }
  if (report.operator_profile?.passkey_sign_in !== true) {
    pushError(errors, "operator_profile.passkey_sign_in must be true");
  }
  for (const name of REQUIRED_CHECKS) {
    const check = report.checks?.[name];
    if (!check || check.ok !== true || !hasText(check.evidence)) {
      pushError(errors, `checks.${name} must have ok=true and evidence text`);
    }
  }
  const extraChecks = Object.keys(report.checks || {}).filter((name) => !REQUIRED_CHECKS.includes(name));
  if (extraChecks.length > 0) {
    pushError(errors, `unknown checks are not accepted: ${extraChecks.join(", ")}`);
  }
  validateReviewArtifacts(report, errors);
  assertNoSensitiveText(report, errors);
  return {
    schema: SCHEMA,
    ok: errors.length === 0,
    required_checks: REQUIRED_CHECKS,
    errors,
  };
}

function validFixture() {
  const report = template();
  report.ok = true;
  report.reviewed_at = new Date("2026-07-03T12:00:00.000Z").toISOString();
  report.reviewer = "operator";
  report.source.commit = "abcdef1234567890";
  report.operator_profile.passkey_sign_in = true;
  for (const name of REQUIRED_CHECKS) {
    report.checks[name] = {
      ok: true,
      evidence: `Reviewed ${name} on localhost without GUI bleed-through.`,
    };
  }
  report.review_artifacts = [
    {
      kind: "screenshot",
      path: "/tmp/redacted-home-shell-review.png",
      sha256: "a".repeat(64),
      redacted: true,
      description: "Redacted Home shell review screenshot.",
    },
  ];
  return report;
}

function validNotesFixture() {
  const reviewedAt = new Date("2026-07-03T12:00:00.000Z").toISOString();
  return `# Home Shell Manual UX Notes

schema: elastos.home-shell.manual-notes/v1
reviewed_at: ${reviewedAt}
reviewer: operator
source_branch: feat/elastos-shell-protocol
source_commit: abcdef1234567890
home_url: http://localhost:61180/apps/home/

## Operator Profile

- Browser: Brave operator profile
- Passkey sign-in completed: yes
- Profile notes: Reviewed on the normal operator profile.

## Required Checks

${REQUIRED_CHECKS.map((name) => `- ${name}:\n  evidence: Reviewed ${name} on localhost without GUI bleed-through.`).join("\n\n")}

## Artifact Review

- I reviewed this note file for secrets before hashing it: yes
- Suggested report artifact kind: manual_notes
`;
}

function selfTest() {
  const accepted = validateHomeShellManualUxReport(validFixture());
  if (!accepted.ok) {
    throw new Error(`valid fixture rejected: ${accepted.errors.join("; ")}`);
  }
  const missingCheck = validFixture();
  missingCheck.checks.no_home_gui_chrome_bleed_through.ok = false;
  if (validateHomeShellManualUxReport(missingCheck).ok) {
    throw new Error("validator accepted a report with a missing GUI bleed-through check");
  }
  const sensitive = validFixture();
  sensitive.checks.signed_in_with_passkey.evidence = "loaded with home_token=secret";
  if (validateHomeShellManualUxReport(sensitive).ok) {
    throw new Error("validator accepted unredacted sensitive evidence");
  }
  const missingArtifact = validFixture();
  missingArtifact.review_artifacts = [];
  if (validateHomeShellManualUxReport(missingArtifact).ok) {
    throw new Error("validator accepted a report without a hash-bound artifact");
  }
  const staleBrowserOpenCheck = validFixture();
  staleBrowserOpenCheck.checks.home_cli_open_browser_returns_to_home_gui_window =
    staleBrowserOpenCheck.checks.home_cli_hides_gui_only_browser_from_default_menu;
  delete staleBrowserOpenCheck.checks.home_cli_hides_gui_only_browser_from_default_menu;
  if (validateHomeShellManualUxReport(staleBrowserOpenCheck).ok) {
    throw new Error("validator accepted the stale Home CLI Browser-open check");
  }
  const notesReportPath = "/tmp/home-shell-manual-ux-valid-notes.md";
  writeFileSync(notesReportPath, validNotesFixture());
  const notesReport = reportFromNotes(notesReportPath);
  if (!validateHomeShellManualUxReport(notesReport).ok) {
    throw new Error("validator rejected report generated from valid manual notes");
  }
  const unreviewedPath = "/tmp/home-shell-manual-ux-unreviewed-notes.md";
  writeFileSync(
    unreviewedPath,
    validNotesFixture().replace(
      "I reviewed this note file for secrets before hashing it: yes",
      "I reviewed this note file for secrets before hashing it: no",
    ),
  );
  try {
    reportFromNotes(unreviewedPath);
    throw new Error("notes-to-report accepted unreviewed manual notes");
  } catch (error) {
    if (!String(error?.message || error).includes("manual notes are incomplete")) {
      throw error;
    }
  }
  const missingReviewTimePath = "/tmp/home-shell-manual-ux-missing-review-time-notes.md";
  writeFileSync(
    missingReviewTimePath,
    validNotesFixture().replace(/^reviewed_at: .*$/m, "reviewed_at:"),
  );
  try {
    reportFromNotes(missingReviewTimePath);
    throw new Error("notes-to-report accepted manual notes without reviewed_at");
  } catch (error) {
    if (!String(error?.message || error).includes("reviewed_at must be the real human review timestamp")) {
      throw error;
    }
  }
  return {
    schema: "elastos.home-shell.manual-ux-report.self-test/v1",
    ok: true,
    valid_fixture_accepted: true,
    missing_check_rejected: true,
    sensitive_text_rejected: true,
    missing_artifact_rejected: true,
    stale_overlay_check_rejected: true,
    valid_notes_report_accepted: true,
    unreviewed_notes_rejected: true,
    missing_review_time_notes_rejected: true,
  };
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.selfTest) {
    console.log(JSON.stringify(selfTest(), null, 2));
    return;
  }
  if (args.template) {
    const report = JSON.stringify(template(), null, 2);
    if (args.out) {
      writeFileSync(args.out, `${report}\n`);
      console.log(JSON.stringify({ schema: SCHEMA, ok: true, out: args.out }, null, 2));
      return;
    }
    console.log(report);
    return;
  }
  if (args.notesTemplate) {
    const notes = notesTemplate();
    if (args.out) {
      writeFileSync(args.out, notes);
      console.log(JSON.stringify({ schema: SCHEMA, ok: true, out: args.out }, null, 2));
      return;
    }
    console.log(notes);
    return;
  }
  if (args.artifactEntry) {
    console.log(JSON.stringify(artifactEntry(args.artifactEntry), null, 2));
    return;
  }
  if (args.reportFromNotes) {
    const report = reportFromNotes(args.reportFromNotes);
    writeFileSync(args.out, `${JSON.stringify(report, null, 2)}\n`);
    console.log(JSON.stringify({ schema: SCHEMA, ok: true, out: args.out }, null, 2));
    return;
  }
  const report = JSON.parse(readFileSync(args.input, "utf8"));
  const result = validateHomeShellManualUxReport(report);
  console.log(JSON.stringify(result, null, 2));
  if (!result.ok) {
    process.exit(1);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] || "").href) {
  try {
    main();
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    usage();
    process.exit(2);
  }
}
