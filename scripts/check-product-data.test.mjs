import assert from "node:assert/strict";
import test from "node:test";
import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { checkProductData, validateCapsule, validateCatalogBinding, validateComponents, validateContracts, validateModelCatalog } from "./check-product-data.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const json = (path) => JSON.parse(readFileSync(resolve(root, path), "utf8"));
const manifests = {};
const paths = [];
for (const base of ["capsules", "elastos/capsules"]) {
  for (const entry of readdirSync(resolve(root, base), { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const path = `${base}/${entry.name}/capsule.json`;
    try {
      const value = json(path);
      manifests[value.name] = value;
      paths.push(path);
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
  }
}
const components = json("components.json");
const catalog = json("model-catalog.json");
const clone = (value) => structuredClone(value);

test("shipped capsule, profile, catalog and icon data pass together", () => {
  const result = checkProductData();
  assert.equal(result.capsules, paths.length);
  assert(result.capsules > 0 && result.profiles > 0 && result.method_contracts > 0);
});

test("catalog head binding refuses changed signed bytes and a substituted CID", () => {
  const bytes = readFileSync(resolve(root, "model-catalog.json"));
  assert.throws(() => validateCatalogBinding(components, Buffer.concat([bytes, Buffer.from(" ")])), /pinned head CID/);
  const value = clone(components);
  value.model_catalog.head_cid = "bafkreiother";
  assert.throws(() => validateCatalogBinding(value, bytes), /pinned head CID/);
});

for (const [name, mutate, message] of [
  ["unknown role", (value) => value.role = "root", /unknown role/],
  ["provider metadata on an app", (value) => value.authority = {}, /provider owns authority/],
  ["host process permission on an app", (value) => value.permissions = { host_process: true }, /provider owns host process/],
  ["guest network permission on an app", (value) => value.permissions = { guest_network: true }, /provider owns host process/],
  ["external app dependency", (value) => value.requires = [{ name: "curl", kind: "external" }], /Runtime owns external/],
  ["raw backend capability", (value) => value.capabilities = ["elastos://chain/*"], /Runtime owns backend/],
  ["raw backend storage", (value) => value.permissions = { storage: ["elastos://wallet/*"] }, /Runtime owns backend/],
  ["provider override", (value) => value.providers = { chain: "other" }, /Runtime owns provider/],
  ["unmanaged microVM ingress", (value) => value.microvm = { http_port: 8080 }, /Runtime owns microVM/],
  ["unsupported resource scheme", (value) => value.capabilities = ["https://example.com"], /invalid capability/],
  ["malformed capability list", (value) => value.capabilities = "", /capabilities must be an array/],
  ["malformed dependency list", (value) => value.requires = "", /requires must be an array/],
  ["wrong projection ABI", (value) => value.runtime_abi = "elastos:bus@v1", /projection ABI/],
  ["entrypoint traversal", (value) => value.entrypoint = "../wallet/browser/index.html", /safe artifact path/],
  ["absolute entrypoint", (value) => value.entrypoint = "/outside/index.html", /safe artifact path/],
  ["missing interfaces", (value) => delete value.interfaces, /interfaces are missing/],
  ["duplicate method ids", (value) => value.interfaces[0].methods.push(value.interfaces[0].methods[0]), /duplicate entries/],
  ["missing method approval", (value) => delete value.interfaces[0].methods[0].approval, /method approval/],
]) {
  test(`capsule refuses ${name}`, () => {
    const value = clone(manifests.documents);
    mutate(value);
    assert.throws(() => validateCapsule(value), message);
  });
}

for (const [name, mutate, message] of [
  ["missing provider namespace", (value) => delete value.provides, /provider needs a resource/],
  ["missing provider authority", (value) => delete value.authority, /provider needs authority/],
  ["empty provider audit events", (value) => value.authority.audit_events = [], /audit events/],
  ["unsupported provider capability", (value) => value.authority.capabilities[0].resource = "https://example.com", /invalid authority/],
  ["malformed provider action", (value) => value.authority.capabilities[0].actions = [null], /actions are empty or invalid/],
]) {
  test(`provider refuses ${name}`, () => {
    const value = clone(manifests["chain-provider"]);
    mutate(value);
    assert.throws(() => validateCapsule(value), message);
  });
}

for (const [name, mutate, message] of [
  ["unsigned payment affordance", (value) => value.wallet.interfaces[0].methods.find((entry) => entry.id === "approval.sign").approval = "runtime_policy", /approval.sign contract/],
  ["raw Browser authority", (value) => value.browser.capabilities.push("elastos://net/stream"), /Runtime owns raw backend/],
  ["missing Browser dependency", (value) => value.browser.requires.pop(), /wallet-provider dependency/],
  ["private Wallet operation exposed", (value) => value["wallet-provider"].interfaces[0].methods.push({ id: "sign" }), /read-only status/],
  ["generic DID signing", (value) => value["did-provider"].authority.capabilities[0].operations.push("sign"), /typed signing intents/],
  ["shell authority mismatch", (value) => value["home-cli"].interfaces[0].methods.find((entry) => entry.id === "shell.switch").approval = "runtime_policy", /common authority/],
  ["missing viewer launch schema", (value) => delete value.documents.interfaces[0].methods.find((entry) => entry.id === "document.open").input_schema, /input_schema/],
]) {
  test(`affordance contracts refuse ${name}`, () => {
    const value = clone(manifests);
    mutate(value);
    assert.throws(() => validateContracts(value), message);
  });
}

for (const [name, mutate, message] of [
  ["unknown profile component", (value) => value.profiles.home.components.push("missing-provider"), /unknown component/],
  ["missing Wallet dependency", (value) => value.profiles.home.components = value.profiles.home.components.filter((entry) => entry !== "wallet-provider"), /Wallet\/Browser dependency/],
  ["duplicate protected dependency", (value) => value.profiles.home.components.push("media-provider"), /must occur once/],
  ["default public edge service", (value) => value.profiles.home.components.push("tunnel-provider"), /explicit profile/],
  ["custody in default Home", (value) => value.profiles.home.components.push("custody-provider"), /Custody profile/],
  ["missing release path", (value) => delete value.external["chain-provider"].platforms["linux-amd64"].release_path, /release path/],
  ["missing archive extraction path", (value) => delete value.external.browser.platforms["*"].extract_path, /archive extraction/],
  ["artifact path traversal", (value) => value.external.browser.platforms["*"].extract_path = "../outside", /safe artifact path/],
  ["Windows absolute artifact path", (value) => value.external.browser.platforms["*"].extract_path = "C:\\Windows\\outside", /safe artifact path/],
  ["Windows drive relative artifact path", (value) => value.external.browser.platforms["*"].extract_path = "C:outside", /safe artifact path/],
  ["Windows artifact traversal", (value) => value.external.browser.platforms["*"].extract_path = "..\\outside", /safe artifact path/],
  ["missing release platform", (value) => delete value.external["chain-provider"].platforms["linux-arm64"], /release metadata/],
  ["Runtime-only provider exposed as a capsule", (value) => value.capsules["custody-provider"] = {}, /Runtime owns provider inventory/],
]) {
  test(`component data refuse ${name}`, () => {
    const value = clone(components);
    mutate(value);
    assert.throws(() => validateComponents(value, manifests), message);
  });
}

for (const [name, mutate, message] of [
  ["substituted signer", (value) => value.signer_did = "did:key:other", /trusted signer/],
  ["missing model weights", (value) => value.payload.entries[0].object_manifest.files.pop(), /lacks weights/],
  ["missing model provenance", (value) => value.payload.entries[0].capsule_manifest.model_content.provenance.path = "missing.md", /lacks missing/],
  ["malformed file digest", (value) => value.payload.entries[0].object_manifest.files[0].sha256 = "00", /integrity metadata/],
  ["model file traversal", (value) => value.payload.entries[0].object_manifest.files.push({ path: "../outside", sha256: "0".repeat(64), size: 1 }), /safe artifact path/],
  ["absolute model file", (value) => value.payload.entries[0].object_manifest.files.push({ path: "/outside", sha256: "0".repeat(64), size: 1 }), /safe artifact path/],
  ["negative minimum memory", (value) => value.payload.entries[0].capsule_manifest.model_content.minimum_memory_mb = -1, /positive integer/],
]) {
  test(`model data refuse ${name}`, () => {
    const value = clone(catalog);
    mutate(value);
    assert.throws(() => validateModelCatalog(components, value), message);
  });
}

test("CLI exits unsuccessfully on a broken manifest and missing declared icon fixture", () => {
  const fixture = mkdtempSync(join(tmpdir(), "product-data-fixture-"));
  try {
    // Copy manifests and point at small checked-in assets; fixture writes remain isolated.
    for (const path of paths) {
      const value = json(path);
      const directory = resolve(fixture, dirname(path));
      mkdirSync(directory, { recursive: true });
      copyFileSync(resolve(root, path), resolve(fixture, path));
      if (value.execution === "web-projection") {
        mkdirSync(dirname(resolve(directory, value.entrypoint)), { recursive: true });
        writeFileSync(resolve(directory, value.entrypoint), "");
      }
      if (value.icon) {
        mkdirSync(dirname(resolve(directory, value.icon)), { recursive: true });
        symlinkSync(resolve(root, dirname(path), value.icon), resolve(directory, value.icon), "dir");
      }
    }
    for (const path of ["components.json", "model-catalog.json", "capsules/home/browser/manifest.webmanifest", "capsules/home/browser/elastos-home-icon-192.png", "capsules/home/browser/elastos-home-icon-512.png"]) copyFileSync(resolve(root, path), resolve(fixture, path));
    const lint = resolve(root, "scripts/check-product-data.mjs");
    assert.equal(JSON.parse(execFileSync(process.execPath, [lint, fixture], { encoding: "utf8" })).ok, true);
    const path = resolve(fixture, "capsules/assistant/capsule.json");
    const value = json("capsules/assistant/capsule.json");
    value.role = "root";
    writeFileSync(path, JSON.stringify(value));
    let result = spawnSync(process.execPath, [lint, fixture], { encoding: "utf8" });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /capsules\/assistant\/capsule.json: unknown role/);
    value.role = "app";
    value.icon = "missing-icons";
    writeFileSync(path, JSON.stringify(value));
    result = spawnSync(process.execPath, [lint, fixture], { encoding: "utf8" });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /missing 32px icon/);
  } finally {
    rmSync(fixture, { recursive: true, force: true });
  }
});
