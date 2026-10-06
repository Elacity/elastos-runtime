import assert from "node:assert/strict";
import test from "node:test";
import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { checkProductData, parsePublishData, validatePublishData, validateCapsule, validateComponents, validateContracts } from "./check-product-data.mjs";

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
const clone = (value) => structuredClone(value);
const shellPublish = readFileSync(resolve(root, "scripts/publish-release.sh"), "utf8");
const rustPublish = readFileSync(resolve(root, "elastos/crates/elastos-server/src/publish.rs"), "utf8");
const publish = parsePublishData(shellPublish, rustPublish);

for (const [name, mutate, message] of [
  ["missing shell default capsule", (_components, lists) => lists.shellDefault = lists.shellDefault.filter((name) => name !== "home-gui"), /shellDefault: published capsules/],
  ["missing Rust Home capsule", (_components, lists) => lists.rustHome = lists.rustHome.filter((name) => name !== "services"), /rustHome: published capsules/],
  ["missing shell required capsule", (_components, lists) => lists.shellRequired = lists.shellRequired.filter((name) => name !== "wallet-provider"), /shellRequired: missing Home capsule wallet-provider/],
  ["missing Rust required capsule", (_components, lists) => lists.rustRequired = lists.rustRequired.filter((name) => name !== "documents"), /rustRequired: missing Home capsule documents/],
  ["demo capsule in default publication", (_components, lists) => lists.shellDefault.push("chat-room"), /shellDefault: published capsules/],
  ["missing demo publication", (_components, lists) => lists.rustDemo = lists.rustDemo.filter((name) => name !== "gba-emulator"), /missing demo capsule/],
  ["Home capsule in demo additions", (_components, lists) => lists.rustDemo.push("home"), /belongs in the demo profile/],
  ["uninstalled demo publication", (value) => value.profiles.demo.components = value.profiles.demo.components.filter((name) => name !== "tunnel-provider"), /belongs in the demo profile/],
  ["Home installation without publication parity", (value) => value.profiles.home.components = value.profiles.home.components.filter((name) => name !== "assistant"), /published capsules/],
  ["different supported capsule sets", (_components, lists) => lists.shellRequired.push("key-provider"), /supported publish capsules must match/],
  ["unknown supported capsule", (_components, lists) => { lists.shellRequired.push("unknown"); lists.rustRequired.push("unknown"); }, /unknown or retired publish capsule/],
]) {
  test(`publication data refuse ${name}`, () => {
    const value = clone(components), lists = clone(publish);
    mutate(value, lists);
    assert.throws(() => validatePublishData(value, manifests, lists), message);
  });
}

test("publish parser reads declared values and ignores comments", () => {
  assert.deepEqual(parsePublishData(shellPublish.replace("DEFAULT_CAPSULES=(", "DEFAULT_CAPSULES=( # comment-capsule"), rustPublish.replace('    "shell",', '    "shell", // "comment-capsule"')), publish);
  assert.deepEqual(parsePublishData(shellPublish.replace("\n    home-gui\n", '\n    "home-gui"\n'), rustPublish), publish);
  assert.throws(() => parsePublishData(shellPublish.replace("DEFAULT_CAPSULES=(", "DEFAULT_CAPSULES=(\n    home"), rustPublish), /duplicate entries/);
  assert.throws(() => parsePublishData(shellPublish.replace("DEFAULT_CAPSULES=(", "DEFAULT_CAPSULES=(\n    $(untrusted)"), rustPublish), /invalid publish capsule data/);
  assert.throws(() => parsePublishData(shellPublish, rustPublish.replace('"services"', "true")), /invalid publish capsule data/);
});

for (const [name, shell, rust, message] of [
  ["shell default", shellPublish.replace(/(DEFAULT_CAPSULES=\([\s\S]*?)\n    home-gui\n/, "$1\n"), rustPublish, /shellDefault: published capsules/],
  ["shell required", shellPublish.replace(/(REQUIRED_SUPPORTED_CAPSULES=\([\s\S]*?)\n    wallet-provider\n/, "$1\n"), rustPublish, /shellRequired: missing Home capsule/],
  ["Rust Home", shellPublish, rustPublish.replace(/(const HOME_PUBLISH_CAPSULES:[\s\S]*?)\n    "services",/, "$1"), /rustHome: published capsules/],
  ["Rust required", shellPublish, rustPublish.replace(/(const REQUIRED_SUPPORTED_PUBLISH_CAPSULES:[\s\S]*?)\n    "documents",/, "$1"), /rustRequired: missing Home capsule/],
  ["Rust demo", shellPublish, rustPublish.replace('"gba-emulator", "gba-ucity", "chat-room", "tunnel-provider"', '"gba-ucity", "chat-room", "tunnel-provider"'), /missing demo capsule/],
]) {
  test(`changed ${name} declared publication data is refused`, () => {
    assert.throws(() => validatePublishData(components, manifests, parsePublishData(shell, rust)), message);
  });
}

test("shipped capsule, profile, catalog and icon data pass together", () => {
  const result = checkProductData();
  assert.equal(result.capsules, paths.length);
  assert(result.capsules > 0 && result.profiles > 0 && result.method_contracts > 0);
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

for (const [name, field, invalid, message] of [
  ["documents", "type", "microvm", /projection type/],
  ["home-cli", "type", "native-provider", /projection type/],
  ["home-gui", "type", "microvm", /projection type/],
  ["browser", "execution", "component", /projection execution/],
  ["documents", "execution", null, /projection execution/],
  ["documents", "runtime_abi", "elastos.provider-stdio/v1", /projection ABI/],
  ["documents", "microvm", {}, /projection.*metadata/],
  ["chain-provider", "type", "microvm", /native provider type/],
  ["chain-provider", "type", "wasm", /native provider type/],
  ["chain-provider", "execution", "microvm", /native provider execution/],
  ["chain-provider", "execution", null, /native provider execution/],
  ["chain-provider", "runtime_abi", "elastos.component/v1", /native provider ABI/],
  ["chain-provider", "microvm", {}, /native provider.*metadata/],
  ["chain-provider", "bus_contract", "elastos.runtime-projection/v1", /projection type/],
  ["shell", "type", "wasm", /native host type/],
  ["shell", "execution", null, /native host execution/],
  ["shell", "entrypoint", "arbitrary-program", /native host entrypoint/],
]) {
  test(`${name} refuses contradictory ${field}=${JSON.stringify(invalid)}`, () => {
    const value = clone(manifests[name]);
    if (value.role === "provider") Object.assign(value, {type: "native-provider", execution: "native-provider", runtime_abi: "elastos.provider-stdio/v1", entrypoint: value.name});
    if (name === "shell") Object.assign(value, {type: "native-host", execution: "native-host", runtime_abi: "native-host", entrypoint: "shell"});
    value[field] = invalid;
    assert.throws(() => validateCapsule(value), message);
  });
}

test("old and new provider and web projection formats validate during expansion", () => {
  const old = clone(manifests["chain-provider"]);
  assert.equal(old.type, "microvm");
  assert.equal(old.execution, undefined);
  assert.equal(old.runtime_abi, undefined);
  validateCapsule(old);
  validateCapsule({...old, type: "wasm"});
  validateCapsule({...old, type: "native-provider", execution: "native-provider", runtime_abi: "elastos.provider-stdio/v1", entrypoint: old.name});
  assert.equal(manifests.documents.type, "wasm");
  validateCapsule(manifests.documents);
  validateCapsule({...manifests.documents, type: "web-projection"});
});

test("shipped execution enums stay readable by released Runtimes", () => {
  for (const manifest of Object.values(manifests)) {
    assert.ok(["wasm", "microvm", "oci", "media", "data"].includes(manifest.type), manifest.name);
    if (manifest.execution) assert.ok(["wasi-receipt", "wasi-app", "web-projection", "component", "microvm", "data"].includes(manifest.execution), manifest.name);
    if (manifest.runtime_abi) assert.ok(["wasi-preview1", "elastos.runtime-projection/v1", "elastos.component/v1", "microvm-linux", "data"].includes(manifest.runtime_abi), manifest.name);
  }
});

test("descriptive types require their execution metadata in both directions", () => {
  for (const [name, message] of [["documents", /projection execution/], ["chain-provider", /native provider execution/], ["shell", /native host execution/]]) {
    const value = clone(manifests[name]);
    value.type = name === "chain-provider" ? "native-provider" : name === "shell" ? "native-host" : "web-projection";
    for (const field of ["execution", "runtime_abi", "bus_contract"]) delete value[field];
    assert.throws(() => validateCapsule(value), message);
  }
});

for (const name of ["home", "system", "services", "people", "documents", "library", "marketplace", "archive-manager", "inbox"]) {
  for (const platform of ["linux-amd64", "linux-arm64"]) {
    test(`${name} refuses missing ${platform} extraction metadata for a changed archive format`, () => {
      const value = clone(components);
      const metadata = clone(value.external[name].platforms[platform] ?? value.external[name].platforms["*"]);
      metadata.release_path = `${name}.zip`;
      delete metadata.extract_path;
      value.external[name].platforms[platform] = metadata;
      assert.throws(() => validateComponents(value, manifests), /archive extraction path/);
    });
  }
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

function dataFixture() {
  const fixture = mkdtempSync(join(tmpdir(), "product-data-fixture-"));
  try {
    // Copy only the declared small assets; fixture writes and icon paths stay isolated.
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
        mkdirSync(resolve(directory, value.icon), { recursive: true });
        for (const size of [32, 64, 128, 256]) copyFileSync(resolve(root, dirname(path), value.icon, `icon-${size}.png`), resolve(directory, value.icon, `icon-${size}.png`));
      }
    }
    for (const path of ["components.json", "capsules/home/browser/manifest.webmanifest", "capsules/home/browser/elastos-home-icon-192.png", "capsules/home/browser/elastos-home-icon-512.png", "scripts/publish-release.sh", "elastos/crates/elastos-server/src/publish.rs"]) {
      mkdirSync(dirname(resolve(fixture, path)), { recursive: true });
      copyFileSync(resolve(root, path), resolve(fixture, path));
    }
    return fixture;
  } catch (error) {
    rmSync(fixture, { recursive: true, force: true });
    throw error;
  }
}

test("CLI exits unsuccessfully on a broken manifest and missing declared icon fixture", () => {
  const fixture = dataFixture();
  try {
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

test("product data accepts a symlink alias for its repository root", () => {
  const fixture = dataFixture(), alias = `${fixture}-alias`;
  try {
    symlinkSync(fixture, alias, "dir");
    assert.equal(checkProductData(alias).capsules, paths.length);
  } finally {
    rmSync(alias, { force: true });
    rmSync(fixture, { recursive: true, force: true });
  }
});

for (const [name, path, type, message] of [
  ["capsule icon directory", "capsules/assistant/browser/icons", "dir", /icon stays in its capsule/],
  ["capsule icon file", "capsules/assistant/browser/icons/icon-32.png", "file", /icon stays in its capsule/],
  ["Home PWA icon file", "capsules/home/browser/elastos-home-icon-192.png", "file", /Home PWA icon stays in its capsule/],
]) {
  test(`product data refuses escaping ${name} symlink`, () => {
    const fixture = dataFixture();
    try {
      rmSync(resolve(fixture, path), { recursive: true });
      symlinkSync(resolve(root, path), resolve(fixture, path), type);
      assert.throws(() => checkProductData(fixture), message);
    } finally {
      rmSync(fixture, { recursive: true, force: true });
    }
  });
}
