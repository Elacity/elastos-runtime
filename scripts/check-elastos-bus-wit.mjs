#!/usr/bin/env node
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const witPath = resolve(repoRoot, "elastos/wit/elastos-bus-v1.wit");
const fixtureRoot = resolve(repoRoot, "elastos/tests/fixtures/components/bus-v1");
const wit = readFileSync(witPath, "utf8");
const witSha256 = createHash("sha256").update(wit).digest("hex");
const fixtureManifest = JSON.parse(readFileSync(resolve(fixtureRoot, "capsule.json"), "utf8"));
const fixtureArtifact = readFileSync(resolve(fixtureRoot, fixtureManifest.entrypoint));
const componentBuilder = readFileSync(resolve(repoRoot, "scripts/build-component-capsule.sh"), "utf8");
const productCatalog = readFileSync(resolve(repoRoot, "components.json"), "utf8");

const section = (kind, name) => {
  const match = wit.match(new RegExp(`${kind}\\s+${name}\\s*\\{([\\s\\S]*?)\\n\\}`));
  assert.ok(match, `${kind} ${name} must exist`);
  return match[1];
};

const includes = (text, value, message = `${value} missing`) => {
  assert.ok(text.includes(value), message);
};

includes(wit, "package elastos:bus@1.0.0;");
includes(wit, "Manifest contract id: elastos:bus@v1.");

const world = section("world", "product-capsule-v1");
for (const imported of ["runtime", "identity", "capabilities", "providers"]) {
  includes(world, `import ${imported};`, `world must import ${imported}`);
}
includes(world, "export lifecycle;", "world must export lifecycle");

const requiredInterfaces = {
  runtime: ["info: func() -> runtime-info;"],
  identity: ["context: func() -> identity-context;"],
  capabilities: ["request: func(request: capability-request) -> result<capability-grant, bus-error>;"],
  providers: ["invoke: func(request: invoke-request) -> result<invoke-response, bus-error>;"],
  lifecycle: ["run: func() -> result<_, bus-error>;"],
};
for (const [interfaceName, required] of Object.entries(requiredInterfaces)) {
  const body = section("interface", interfaceName);
  for (const value of required) includes(body, value, `${interfaceName} must define ${value}`);
}

assert.equal(fixtureManifest.runtime_abi, "elastos.component/v1");
assert.equal(fixtureManifest.bus_contract, "elastos:bus@v1");
assert.equal(fixtureManifest.execution, "component");
assert.equal(fixtureManifest.wit_world_sha256, witSha256, "fixture must bind the checked-in WIT");
assert.ok(!productCatalog.includes("bus-v1-conformance"), "test fixture must not enter components.json");

const artifactText = fixtureArtifact.toString("latin1");
for (const hostPath of [/\/Users\/[^/\0]+\//, /\/home\/[^/\0]+\//, /[A-Za-z]:\\Users\\/]) {
  assert.ok(!hostPath.test(artifactText), "component artifact must not embed a developer home path");
}

for (const required of ["mktemp -d", "--locked", "--remap-path-prefix", "CARGO_TARGET_DIR"]) {
  includes(componentBuilder, required, `component builder must use ${required}`);
}
for (const stale of ["elastos.component-bus/v1", "component-bus-fixture"]) {
  assert.ok(!wit.includes(stale), `WIT contains stale identifier ${stale}`);
  assert.ok(!componentBuilder.includes(stale), `builder contains stale identifier ${stale}`);
}

const providerRequest = section("record", "invoke-request");
assert.ok(!/\bprovider\b/.test(providerRequest), "invoke request must select a resource, not a provider");

for (const forbidden of [
  /\bwasi:/i,
  /\bfilesystem\b/i,
  /\bpreopen\b/i,
  /\bpath\b/i,
  /\bnetwork\b/i,
  /\bsocket\b/i,
  /\btcp\b/i,
  /\budp\b/i,
  /\bhttp\b/i,
  /\burl\b/i,
  /\bgateway\b/i,
  /\benv\b/i,
  /\bprovider-id\b/i,
]) {
  assert.ok(!forbidden.test(wit), `WIT must not expose ${forbidden}`);
}

console.log(`PASS ElastOS Bus WIT sha256=${witSha256}`);
