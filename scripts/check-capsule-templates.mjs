#!/usr/bin/env node

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const read = (path) => readFileSync(resolve(root, path), "utf8");
const json = (path) => JSON.parse(read(path));
const exists = (path) => existsSync(resolve(root, path));
const internalCopy = /\b(runtime mirror|projection|schema|derived facts?|capability surface|provider boundary|hostcall|launch token)\b/i;

const paths = {
  component: "templates/capsules/component-app/capsule.json",
  web: "templates/capsules/web-app/capsule.json",
  viewer: "templates/capsules/viewer-content/viewer.capsule.json",
  content: "templates/capsules/viewer-content/content.capsule.json",
  provider: "templates/capsules/provider-contract/capsule.json",
};
const manifests = Object.fromEntries(
  Object.entries(paths).map(([name, path]) => [name, json(path)]),
);

for (const [name, manifest] of Object.entries(manifests)) {
  for (const field of ["schema", "name", "version", "description", "author", "role", "type", "entrypoint"]) {
    assert.equal(typeof manifest[field], "string", `${name} template requires ${field}`);
    assert.ok(manifest[field].trim(), `${name} template requires non-empty ${field}`);
  }
  assert.equal(manifest.schema, "elastos.capsule/v1", `${name} template schema drifted`);
  const publicCopy = [
    manifest.description,
    ...(manifest.interfaces || []).flatMap((entry) => [
      entry.description,
      ...(entry.methods || []).map((method) => method.description),
    ]),
  ].filter(Boolean);
  for (const copy of publicCopy) {
    assert.ok(!internalCopy.test(copy), `${name} template uses internal public copy: ${copy}`);
  }
}

const witHash = createHash("sha256")
  .update(read("elastos/wit/elastos-bus-v1.wit"))
  .digest("hex");
assert.equal(manifests.component.runtime_abi, "elastos.component/v1");
assert.equal(manifests.component.bus_contract, "elastos:bus@v1");
assert.equal(manifests.component.wit_world_sha256, witHash);
assert.equal(manifests.component.execution, "component");
assert.ok(!("permissions" in manifests.component));
assert.ok(!("authority" in manifests.component));
assert.ok(!("capabilities" in manifests.component));
assert.ok(!("interfaces" in manifests.component));
assert.ok(read("templates/capsules/component-app/src/lib.rs").includes('world: "product-capsule-v1"'));

assert.equal(manifests.web.execution, "web-projection");
assert.equal(manifests.web.runtime_abi, "elastos.runtime-projection/v1");
assert.ok(exists("templates/capsules/web-app/browser/index.html"));
assert.ok(exists("templates/capsules/web-app/browser/app.js"));

assert.equal(manifests.viewer.role, "viewer");
assert.equal(manifests.content.role, "content");
assert.equal(manifests.content.type, "data");
assert.equal(manifests.content.viewer, manifests.viewer.name);
const accepts = manifests.viewer.interfaces
  .flatMap((entry) => entry.methods || [])
  .flatMap((method) => method.input_schema?.accepts || []);
assert.ok(accepts.some((entry) => entry.viewer === manifests.content.viewer));
assert.ok(accepts.some((entry) => entry.extensions?.includes(".example")));
assert.ok(exists("templates/capsules/viewer-content/browser/index.html"));
assert.ok(exists("templates/capsules/viewer-content/sample.example"));

assert.equal(manifests.provider.role, "provider");
assert.match(manifests.provider.provides, /^elastos:\/\//);
assert.ok(manifests.provider.authority?.reason);
assert.ok(manifests.provider.authority?.capabilities?.length > 0);
assert.ok(manifests.provider.authority?.audit_events?.length > 0);
assert.ok(exists("templates/capsules/provider-contract/README.md"));

console.log(`PASS capsule templates wit_sha256=${witHash}`);
