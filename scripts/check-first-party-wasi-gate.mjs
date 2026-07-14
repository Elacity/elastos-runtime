#!/usr/bin/env node

import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, extname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const defaultClassificationPath = resolve(
  repoRoot,
  "scripts/first-party-wasi-classifications.json",
);
const classificationPath = process.env.ELASTOS_WASI_CLASSIFICATIONS
  ? resolve(repoRoot, process.env.ELASTOS_WASI_CLASSIFICATIONS)
  : defaultClassificationPath;

const ALLOWED_CLASSES = new Set(["non-product-fixture"]);

const PRODUCT_ROLES = new Set([
  "app",
  "shell",
  "viewer",
  "provider",
  "connector",
  "content",
]);

const TEXT_EXTENSIONS = new Set([
  ".css",
  ".html",
  ".js",
  ".json",
  ".md",
  ".mjs",
  ".rs",
  ".sh",
  ".toml",
  ".ts",
  ".tsx",
  ".txt",
]);

const SOURCE_MARKERS = [
  { id: "wasi-preview1", pattern: /\bwasi-preview1\b/i },
  { id: "wasm32-wasip1", pattern: /\bwasm32-wasip1\b/i },
  { id: "wasi_snapshot_preview1", pattern: /\bwasi_snapshot_preview1\b/i },
  { id: "wasmtime-wasi", pattern: /\bwasmtime-wasi\b/i },
  { id: "WasiP1", pattern: /\bWasiP1\b/ },
  { id: "ELASTOS_CARRIER_FIFOS", pattern: /\bELASTOS_CARRIER_FIFOS\b/ },
  { id: "elastos.carrier_call", pattern: /\belastos\.carrier_call\b/ },
  { id: "/_carrier", pattern: /\/_carrier\b/ },
];

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function repoPath(path) {
  return relative(repoRoot, path).replaceAll("\\", "/");
}

function listDirs(root) {
  if (!existsSync(root)) {
    return [];
  }
  return readdirSync(root, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => resolve(root, entry.name));
}

function listFilesRecursive(root) {
  const entries = readdirSync(root, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    if (
      entry.name === ".git" ||
      entry.name === "target" ||
      entry.name === "node_modules"
    ) {
      continue;
    }
    const path = resolve(root, entry.name);
    if (entry.isDirectory()) {
      files.push(...listFilesRecursive(path));
    } else if (entry.isFile()) {
      files.push(path);
    }
  }
  return files;
}

function firstPartyCapsules() {
  const roots = [resolve(repoRoot, "capsules"), resolve(repoRoot, "elastos/capsules")];
  const capsules = [];
  for (const root of roots) {
    for (const dir of listDirs(root)) {
      const manifestPath = resolve(dir, "capsule.json");
      if (!existsSync(manifestPath) || !statSync(manifestPath).isFile()) {
        continue;
      }
      const manifest = readJson(manifestPath);
      capsules.push({
        dir,
        manifestPath,
        name: manifest.name || repoPath(dir),
        role: manifest.role || "",
        manifest,
      });
    }
  }
  return capsules.sort((left, right) => left.name.localeCompare(right.name));
}

function sourceEvidence(capsule) {
  const evidence = [];
  for (const file of listFilesRecursive(capsule.dir)) {
    if (file === capsule.manifestPath) {
      continue;
    }
    if (file.endsWith("Cargo.lock")) {
      continue;
    }
    if (!TEXT_EXTENSIONS.has(extname(file))) {
      continue;
    }
    const source = readFileSync(file, "utf8");
    const lines = source.split(/\r?\n/);
    for (let index = 0; index < lines.length; index += 1) {
      const line = lines[index];
      for (const marker of SOURCE_MARKERS) {
        if (marker.pattern.test(line)) {
          evidence.push(`${repoPath(file)}:${index + 1}:${marker.id}`);
        }
      }
    }
  }
  return evidence;
}

function manifestEvidence(manifestPath, manifest) {
  const evidence = [];
  if (manifest.runtime_abi === "wasi-preview1") {
    evidence.push(`${repoPath(manifestPath)}:runtime_abi=wasi-preview1`);
  }
  if (typeof manifest.execution === "string" && manifest.execution.startsWith("wasi-")) {
    evidence.push(`${repoPath(manifestPath)}:execution=${manifest.execution}`);
  }
  return evidence;
}

function looksLikeFixture(capsule) {
  return (
    /\bfixture\b/i.test(capsule.name) ||
    /\bfixture\b/i.test(capsule.manifest.description || "") ||
    /\bfixture\b/i.test(repoPath(capsule.dir))
  );
}

function isProductCapsule(capsule, classification) {
  if (classification?.class === "non-product-fixture" && looksLikeFixture(capsule)) {
    return false;
  }
  if (!classification && looksLikeFixture(capsule)) {
    return false;
  }
  return PRODUCT_ROLES.has(capsule.role);
}

function loadClassifications() {
  const config = readJson(classificationPath);
  if (config.schema !== "elastos.first-party-wasi-classifications/v1") {
    throw new Error(
      "scripts/first-party-wasi-classifications.json has an unsupported schema",
    );
  }
  const classes = config.classes || {};
  for (const [name, classification] of Object.entries(classes)) {
    if (!ALLOWED_CLASSES.has(classification.class)) {
      throw new Error(`${name} has invalid WASI classification ${classification.class}`);
    }
    if (typeof classification.reason !== "string" || classification.reason.trim() === "") {
      throw new Error(`${name} WASI classification must include a reason`);
    }
  }
  return classes;
}

function main() {
  const classifications = loadClassifications();
  const findings = firstPartyCapsules()
    .map((capsule) => {
      const evidence = [
        ...manifestEvidence(capsule.manifestPath, capsule.manifest),
        ...sourceEvidence(capsule),
      ];
      const classification = classifications[capsule.name] || null;
      return {
        ...capsule,
        classification,
        evidence,
        product: isProductCapsule(capsule, classification),
      };
    })
    .filter((finding) => finding.evidence.length > 0);

  const productFindings = findings.filter((finding) => finding.product);
  const unclassifiedNonProduct = findings.filter(
    (finding) => !finding.product && !finding.classification,
  );
  const invalidFixtureClassifications = findings.filter(
    (finding) =>
      finding.classification?.class === "non-product-fixture" &&
      !looksLikeFixture(finding),
  );
  const classifiedNames = new Set(findings.map((finding) => finding.name));
  const staleClassifications = Object.keys(classifications)
    .filter((name) => !classifiedNames.has(name))
    .sort();

  const counts = Object.fromEntries([...ALLOWED_CLASSES].map((kind) => [kind, 0]));
  for (const finding of findings) {
    if (finding.classification) {
      counts[finding.classification.class] += 1;
    }
  }

  console.log("First-party WASI Preview 1 gate");
  console.log(`capsules with WASI evidence: ${findings.length}`);
  console.log(`product findings: ${productFindings.length}`);
  console.log(`non-product fixture: ${counts["non-product-fixture"]}`);
  console.log("");

  for (const finding of findings) {
    const classification = finding.classification?.class || "UNCLASSIFIED";
    console.log(`${finding.name} [${classification}] role=${finding.role}`);
    for (const item of finding.evidence) {
      console.log(`  evidence: ${item}`);
    }
    if (finding.classification) {
      console.log(`  reason: ${finding.classification.reason}`);
    }
  }

  if (unclassifiedNonProduct.length > 0) {
    console.log("");
    console.log("Non-product WASI usage without classification:");
    for (const finding of unclassifiedNonProduct) {
      console.log(`  ${finding.name}`);
    }
  }

  if (staleClassifications.length > 0) {
    console.log("");
    console.log("Stale WASI classifications with no current evidence:");
    for (const name of staleClassifications) {
      console.log(`  ${name}`);
    }
    process.exit(1);
  }

  if (productFindings.length > 0) {
    console.error("");
    console.error("FAIL first-party product WASI usage:");
    for (const finding of productFindings) {
      console.error(`  ${finding.name} (${repoPath(finding.manifestPath)})`);
    }
    process.exit(1);
  }

  if (invalidFixtureClassifications.length > 0) {
    console.error("");
    console.error("FAIL non-product-fixture classifications must be fixture capsules:");
    for (const finding of invalidFixtureClassifications) {
      console.error(`  ${finding.name} (${repoPath(finding.manifestPath)})`);
    }
    process.exit(1);
  }

  console.log("");
  console.log("PASS no first-party product Runtime WASI authority found");
}

main();
