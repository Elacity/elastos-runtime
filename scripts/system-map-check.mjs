#!/usr/bin/env node

import { createHash } from "node:crypto";
import { access, readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Script } from "node:vm";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const c4Path = resolve(repoRoot, "docs/system-map/c4.md");
const viewerPath = resolve(repoRoot, "docs/system-map/viewer.html");
const statePath = resolve(repoRoot, "state.md");
const glossaryPath = resolve(repoRoot, "docs/GLOSSARY.md");
const mapReadmePath = resolve(repoRoot, "docs/system-map/README.md");
const [c4, viewer, state, glossary, mapReadme] = await Promise.all([
  readFile(c4Path, "utf8"),
  readFile(viewerPath, "utf8"),
  readFile(statePath, "utf8"),
  readFile(glossaryPath, "utf8"),
  readFile(mapReadmePath, "utf8"),
]);

const failures = [];

const localTargetPath = (sourcePath, target) => {
  if (/^(?:[a-z]+:|#)/i.test(target)) return null;
  const pathPart = decodeURIComponent(target.split("#", 1)[0]);
  return pathPart ? resolve(dirname(sourcePath), pathPart) : null;
};

const linkedDocuments = [
  resolve(repoRoot, "PRINCIPLES.md"),
  statePath,
  c4Path,
  resolve(repoRoot, "docs/README.md"),
  resolve(repoRoot, "docs/ARCHITECTURE.md"),
  resolve(repoRoot, "docs/CARRIER.md"),
  glossaryPath,
  resolve(repoRoot, "docs/PROTECTED_CONTENT.md"),
  mapReadmePath,
  resolve(repoRoot, "docs/system-map/tree.md"),
  resolve(repoRoot, "docs/AGENT_ARCHITECTURE.md"),
  resolve(repoRoot, "docs/CONSEQUENCE_AWARE_EFFECTS.md"),
  resolve(repoRoot, "docs/MODEL_PROVIDER.md"),
  resolve(repoRoot, "docs/PRIVATE_NETWORK.md"),
];

for (const documentPath of linkedDocuments) {
  const document = await readFile(documentPath, "utf8");
  for (const match of document.matchAll(/\[[^\]]+\]\(([^)]+)\)/g)) {
    const targetPath = localTargetPath(documentPath, match[1]);
    if (!targetPath) continue;
    try {
      await access(targetPath);
    } catch {
      failures.push(`${documentPath} links to missing ${match[1]}`);
    }
  }
}

for (const match of viewer.matchAll(/href="([^"]+)"/g)) {
  const targetPath = localTargetPath(viewerPath, match[1]);
  if (!targetPath) continue;
  try {
    await access(targetPath);
  } catch {
    failures.push(`${viewerPath} links to missing ${match[1]}`);
  }
}

const sourceHash = createHash("sha256").update(c4).digest("hex");
const declaredHash = viewer.match(
  /<meta name="elastos-c4-sha256" content="([a-f0-9]{64})">/,
)?.[1];

if (declaredHash !== sourceHash) {
  failures.push(
    `viewer source hash ${declaredHash ?? "is missing"}; expected ${sourceHash}`,
  );
}

const mermaidDiagrams = [...c4.matchAll(/```mermaid\n([\s\S]*?)\n```/g)].map(
  (match) => match[1],
);
if (mermaidDiagrams.length !== 14) {
  failures.push(`c4.md has ${mermaidDiagrams.length} Mermaid diagrams; expected 14`);
}
for (const [index, diagram] of mermaidDiagrams.entries()) {
  if (!/^(?:flowchart\s+(?:LR|RL|TB|BT)|sequenceDiagram)\n/.test(diagram)) {
    failures.push(`Mermaid diagram ${index + 1} has an unexpected declaration`);
  }
}

const expectedViews = [
  "context",
  "containers",
  "runtime",
  "home",
  "collaboration",
  "agent",
  "code",
  "trust",
  "identity",
  "launch",
  "effect",
  "conversation",
  "wallet",
  "deployment",
];
const actualViews = [...viewer.matchAll(/^\s+id: "([^"]+)"/gm)].map(
  (match) => match[1],
);

if (actualViews.join("\n") !== expectedViews.join("\n")) {
  failures.push(
    `viewer view set differs: expected ${expectedViews.join(", ")}; found ${actualViews.join(", ")}`,
  );
}

for (const required of [
  "Peer Runtime admission",
  "public agent identity",
  "PrivateNetwork",
  "terminal outcomes",
  "Person device · Deployment node",
  "not a C4 Level 4 diagram",
  "not the capsule contract",
  "Browser projection still chooses display_mode",
  "seed has no /dev/kvm",
  "Markdown is correct and this viewer is wrong",
]) {
  if (!viewer.includes(required)) failures.push(`viewer is missing: ${required}`);
}

for (const required of [
  "Dated implementation snapshot",
  "Browser projection code still chooses `display_mode`",
  "`POST /api/provider/:scheme/:op` is still a live host adapter",
  "Commit\n  `8dd54706`",
  "public seed has no `/dev/kvm`",
  "Exit is a typed egress service",
  "never enters Runtime, Carrier, or an ordinary App",
  "cannot select a host path",
  "Consequence-aware effects",
]) {
  if (!c4.includes(required)) failures.push(`c4.md is missing: ${required}`);
}

for (const required of [
  "Last updated: 2026-08-28 UTC",
  "`origin/review/collaboration-candidate` at",
  "`46e51a77` is published for review",
  "`POST /api/provider/:scheme/:op` route remains a live host adapter",
  "Browser projection code still selects",
  "`test -e /dev/kvm` returned 1",
  "implemented by `8dd54706`",
  "No shipped capsule manifest declares `actuator`",
]) {
  if (!state.includes(required)) failures.push(`state.md is missing: ${required}`);
}

for (const required of [
  "## ESP",
  "lineage shorthand, not a second",
  "## CEK",
  "## Exit",
  "Runtime selects the local adapter or Carrier route",
]) {
  if (!glossary.includes(required)) failures.push(`GLOSSARY.md is missing: ${required}`);
}

for (const required of [
  "## Change gate",
  "Layer:",
  "Capability:",
  "Hidden detail:",
  "Done-check:",
  "Consequence-aware effects",
]) {
  if (!mapReadme.includes(required)) failures.push(`system-map README is missing: ${required}`);
}

let currentView = null;
const nodeIdsByView = new Map();
for (const line of viewer.split("\n")) {
  const viewMatch = line.match(/^\s+id: "([^"]+)", group:/);
  if (viewMatch) {
    currentView = viewMatch[1];
    nodeIdsByView.set(currentView, new Set());
    continue;
  }
  if (!currentView) continue;
  const nodeMatch = line.match(/\bN\("([^"]+)"/);
  if (!nodeMatch) continue;
  const nodeIds = nodeIdsByView.get(currentView);
  if (nodeIds.has(nodeMatch[1])) {
    failures.push(`viewer view ${currentView} repeats node id ${nodeMatch[1]}`);
  }
  nodeIds.add(nodeMatch[1]);
}

for (const stale of [
  "Authenticated peer message and content endpoints",
  "Admits an authenticated remote request",
  "agent Profile/persona",
  "C4 · Level 4",
  "owns context",
  "boundaries.md",
]) {
  if (viewer.includes(stale)) failures.push(`viewer retains stale wording: ${stale}`);
}

const inlineScript = viewer.match(/<script>([\s\S]*?)<\/script>/)?.[1];
if (!inlineScript) {
  failures.push("viewer inline script is missing");
} else {
  try {
    new Script(inlineScript, { filename: "docs/system-map/viewer.html" });
  } catch (error) {
    failures.push(`viewer JavaScript does not parse: ${error.message}`);
  }
}

if (failures.length > 0) {
  for (const failure of failures) console.error(`FAIL: ${failure}`);
  process.exitCode = 1;
} else {
  console.log(`PASS: C4 viewer matches ${sourceHash}`);
}
