#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

function fail(message) {
  throw new Error(`Carrier dependency generation check failed: ${message}`);
}

function requireText(path, pattern, message) {
  const source = readFileSync(resolve(repoRoot, path), "utf8");
  if (!pattern.test(source)) {
    fail(message);
  }
  return source;
}

function loadMetadata(manifestPath) {
  const absoluteManifest = resolve(repoRoot, manifestPath);
  const result = spawnSync(
    "cargo",
    [
      "metadata",
      "--locked",
      "--format-version",
      "1",
      "--manifest-path",
      absoluteManifest,
    ],
    {
      cwd: repoRoot,
      encoding: "utf8",
      env: process.env,
      maxBuffer: 64 * 1024 * 1024,
    },
  );
  if (result.status !== 0) {
    process.stderr.write(result.stderr || "");
    fail(`cargo metadata failed for ${manifestPath}`);
  }
  return JSON.parse(result.stdout);
}

function versions(metadata, name) {
  return [...new Set(
    metadata.packages
      .filter((pkg) => pkg.name === name)
      .map((pkg) => pkg.version),
  )].sort();
}

function requireVersions(metadata, graphName, name, expected) {
  const actual = versions(metadata, name);
  const wanted = [...expected].sort();
  if (JSON.stringify(actual) !== JSON.stringify(wanted)) {
    fail(
      `${graphName} resolves ${name} as [${actual.join(", ")}], expected [${wanted.join(", ")}]`,
    );
  }
}

function packageId(metadata, name, version) {
  const matches = metadata.packages.filter(
    (pkg) => pkg.name === name && pkg.version === version,
  );
  if (matches.length !== 1) {
    fail(`expected one ${name} ${version} package, found ${matches.length}`);
  }
  return matches[0].id;
}

function reaches(metadata, sourceId, targetId) {
  const nodes = new Map(metadata.resolve.nodes.map((node) => [node.id, node]));
  const pending = [sourceId];
  const visited = new Set();
  while (pending.length > 0) {
    const id = pending.pop();
    if (id === targetId) {
      return true;
    }
    if (visited.has(id)) {
      continue;
    }
    visited.add(id);
    const node = nodes.get(id);
    for (const dep of node?.deps || []) {
      pending.push(dep.pkg);
    }
  }
  return false;
}

function requireReachable(metadata, graphName, source, target) {
  const sourceId = packageId(metadata, source.name, source.version);
  const targetId = packageId(metadata, target.name, target.version);
  if (!reaches(metadata, sourceId, targetId)) {
    fail(
      `${graphName} does not connect ${source.name} ${source.version} to ${target.name} ${target.version}`,
    );
  }
}

function inspectGraph(manifestPath, graphName) {
  const metadata = loadMetadata(manifestPath);
  const expected = new Map([
    ["iroh", ["1.0.2"]],
    ["iroh-gossip", ["0.101.0"]],
    ["iroh-mdns-address-lookup", ["0.4.0"]],
    ["distributed-topic-tracker", ["0.3.5"]],
    ["hickory-proto", ["0.26.1"]],
    ["hickory-resolver", ["0.26.1"]],
    ["ed25519-dalek", ["2.2.0", "3.0.0-rc.0"]],
    ["crossbeam-epoch", ["0.9.20"]],
    ["quinn", ["0.11.9"]],
    ["quinn-proto", ["0.11.15"]],
    ["quick-xml", ["0.41.0"]],
    ["bincode", ["1.3.3"]],
  ]);
  for (const [name, expectedVersions] of expected) {
    requireVersions(metadata, graphName, name, expectedVersions);
  }
  for (const retiredName of ["iroh-quinn", "iroh-quinn-proto", "iroh-quinn-udp"]) {
    requireVersions(metadata, graphName, retiredName, []);
  }

  requireReachable(
    metadata,
    graphName,
    { name: "distributed-topic-tracker", version: "0.3.5" },
    { name: "iroh", version: "1.0.2" },
  );
  requireReachable(
    metadata,
    graphName,
    { name: "distributed-topic-tracker", version: "0.3.5" },
    { name: "iroh-gossip", version: "0.101.0" },
  );
  requireReachable(
    metadata,
    graphName,
    { name: "elastos-identity", version: "0.6.0" },
    { name: "ed25519-dalek", version: "2.2.0" },
  );
  requireReachable(
    metadata,
    graphName,
    { name: "iroh", version: "1.0.2" },
    { name: "ed25519-dalek", version: "3.0.0-rc.0" },
  );
  requireReachable(
    metadata,
    graphName,
    { name: "distributed-topic-tracker", version: "0.3.5" },
    { name: "ed25519-dalek", version: "3.0.0-rc.0" },
  );

  return {
    manifest: relative(repoRoot, resolve(repoRoot, manifestPath)),
    packages: Object.fromEntries(expected),
  };
}

requireText(
  "rust-toolchain.toml",
  /^channel = "1\.91\.0"$/m,
  "rust-toolchain.toml must pin Rust 1.91.0",
);
requireText(
  "elastos/Cargo.toml",
  /^rust-version = "1\.91"$/m,
  "the workspace MSRV must be Rust 1.91",
);
const ci = requireText(
  ".github/workflows/ci.yml",
  /dtolnay\/rust-toolchain@1\.91\.0/,
  "CI must install Rust 1.91.0",
);
const ciToolchains = [...ci.matchAll(/dtolnay\/rust-toolchain@([^\s]+)/g)].map(
  (match) => match[1],
);
if (
  ciToolchains.length === 0 ||
  ciToolchains.some((toolchain) => toolchain !== "1.91.0")
) {
  fail(`CI Rust toolchains must all be 1.91.0, got [${ciToolchains.join(", ")}]`);
}
requireText(
  "scripts/setup-source-home.sh",
  /minor < 91/,
  "source-home setup must reject Rust older than 1.91",
);
for (const path of ["README.md", "docs/GETTING_STARTED.md", "elastos/README.md"]) {
  requireText(path, /Rust 1\.91(\.0)?\+?|Rust 1\.91 or newer/, `${path} must document the Rust 1.91 floor`);
}
const serverManifest = requireText(
  "elastos/crates/elastos-server/Cargo.toml",
  /ed25519-dalek3 = \{ package = "ed25519-dalek", version = "=3\.0\.0-rc\.0" \}/,
  "the Ed25519 3 transport boundary must remain explicit",
);
for (const declaration of [
  /^iroh = "=1\.0\.2"$/m,
  /^iroh-gossip = "=0\.101\.0"$/m,
  /^iroh-mdns-address-lookup = "=0\.4\.0"$/m,
  /^distributed-topic-tracker = "=0\.3\.5"$/m,
]) {
  if (!declaration.test(serverManifest)) {
    fail(`server manifest is missing ${declaration}`);
  }
}
const carrierSource = requireText(
  "elastos/crates/elastos-server/src/carrier.rs",
  /ed25519_dalek3::SigningKey::from_bytes\(&sk_bytes\)/,
  "tracker signing must cross the Ed25519 boundary as raw key bytes",
);
if (!/SecretKey::from_bytes\(&signing_key\.to_bytes\(\)\)/.test(carrierSource)) {
  fail("Iroh signing must cross the Ed25519 boundary as raw key bytes");
}

const auditConfig = readFileSync(resolve(repoRoot, "elastos/.cargo/audit.toml"), "utf8");
for (const closed of ["RUSTSEC-2026-0118", "RUSTSEC-2026-0119"]) {
  if (auditConfig.includes(closed)) {
    fail(`closed Hickory advisory ${closed} is still ignored`);
  }
}
for (const accepted of ["RUSTSEC-2026-0194", "RUSTSEC-2026-0195"]) {
  if (!auditConfig.includes(accepted)) {
    fail(`accepted quick-xml advisory ${accepted} is not explicit`);
  }
}

const report = {
  schema: "elastos.carrier-dependency-generation-check/v1",
  ok: true,
  rust: "1.91.0",
  graphs: [
    inspectGraph("elastos/Cargo.toml", "Runtime workspace"),
    inspectGraph("capsules/object-provider/Cargo.toml", "object-provider workspace"),
  ],
  advisory_disposition: {
    closed: ["RUSTSEC-2026-0118", "RUSTSEC-2026-0119"],
    accepted: ["RUSTSEC-2026-0194", "RUSTSEC-2026-0195"],
    point_updates: {
      "RUSTSEC-2026-0204": "crossbeam-epoch 0.9.20",
      "RUSTSEC-2026-0185": "quinn-proto 0.11.15",
    },
  },
};

console.log(JSON.stringify(report, null, 2));
