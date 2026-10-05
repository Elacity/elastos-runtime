#!/usr/bin/env node

// Validate shipped data. Runtime and UI behaviour belongs in executable tests.
import assert from "node:assert/strict";
import { existsSync, readFileSync, readdirSync, realpathSync } from "node:fs";
import { dirname, isAbsolute, resolve, sep, win32 } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const roles = new Set(["shell", "app", "viewer", "provider", "content"]);
const privilegedApps = new Set([
  "home", "system", "wallet", "wallet-metamask", "wallet-unisat",
  "wallet-walletconnect", "browser", "library",
]);
const rawBackends = new Set([
  "elacity", "elacity-sdk", "elacity_sdk", "gateway", "chain", "wallet",
  "library", "ipfs", "ipfs-cluster", "ipfs-provider", "kubo",
  "net", "exit", "browser-engine", "object", "availability", "drm", "rights", "key", "decrypt",
]);
const object = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const text = (value) => typeof value === "string" && value.trim().length > 0;
const resource = (value) => text(value) && /^(elastos|localhost):\/\/[^\s]+$/.test(value);
const backend = (value) => value?.match(/^elastos:\/\/([^/?#]+)/)?.[1];
const requireList = (value, label) => {
  assert(Array.isArray(value), `${label} must be an array`);
  return value;
};
const unique = (values, label) => assert.equal(new Set(values).size, values.length, `${label} has duplicate entries`);
const present = (value, label) => assert(text(value), `${label} must be nonempty`);
const same = (value, expected, label) => assert.deepEqual(value, expected, label);
const safePath = (value, label) => {
  present(value, label);
  assert(!isAbsolute(value) && !win32.isAbsolute(value) && !/^[a-z]:/i.test(value) && !value.includes("\0") && !value.split(/[\\/]/).includes(".."), `${label}: safe artifact path`);
};

export function validateCapsule(manifest, label = manifest?.name) {
  assert(object(manifest), `${label}: manifest must be an object`);
  same(manifest.schema, "elastos.capsule/v1", `${label}: capsule schema`);
  present(manifest.name, `${label}: name`);
  assert(roles.has(manifest.role), `${label}: unknown role ${manifest.role}`);
  safePath(manifest.entrypoint, `${label}: entrypoint`);
  const permissions = manifest.permissions ?? {};
  assert(object(permissions), `${label}: permissions must be an object`);
  for (const key of ["storage", "messaging"]) {
    if (permissions[key] !== undefined) requireList(permissions[key], `${label}: permissions.${key}`);
  }
  for (const key of ["capabilities", "requires"]) {
    if (manifest[key] !== undefined) requireList(manifest[key], `${label}: ${key}`);
  }
  for (const value of permissions.storage ?? []) assert(resource(value), `${label}: invalid storage resource ${value}`);
  for (const value of manifest.capabilities ?? []) assert(resource(value), `${label}: invalid capability resource ${value}`);
  if (manifest.provides) assert(resource(manifest.provides), `${label}: invalid provides resource`);
  if (manifest.role === "provider") {
    assert(resource(manifest.provides), `${label}: provider needs a resource namespace`);
    assert(object(manifest.authority), `${label}: provider needs authority metadata`);
    present(manifest.authority.reason, `${label}: authority reason`);
    const capabilities = requireList(manifest.authority.capabilities, `${label}: authority capabilities`);
    assert(capabilities.length > 0, `${label}: authority capabilities are empty`);
    for (const capability of capabilities) {
      assert(object(capability) && resource(capability.resource), `${label}: invalid authority resource`);
      const actions = requireList(capability.actions, `${label}: authority actions`);
      assert(actions.length > 0 && actions.every(text), `${label}: authority actions are empty or invalid`);
    }
    const events = requireList(manifest.authority.audit_events, `${label}: audit events`);
    assert(events.length > 0 && events.every(text), `${label}: audit events are empty or invalid`);
  } else {
    assert(manifest.provides === undefined, `${label}: provider owns resource namespaces`);
    assert(manifest.authority === undefined, `${label}: provider owns authority metadata`);
    assert(!permissions.host_process && !permissions.guest_network, `${label}: provider owns host process and network access`);
  }
  if (["app", "viewer", "content"].includes(manifest.role) && !privilegedApps.has(manifest.name)) {
    assert(!manifest.providers, `${label}: Runtime owns provider implementation selection`);
    assert(!manifest.microvm?.http_port, `${label}: Runtime owns microVM ingress`);
    for (const value of [...(manifest.capabilities ?? []), ...(permissions.storage ?? [])]) {
      assert(!rawBackends.has(backend(value)), `${label}: Runtime owns backend resource ${value}`);
    }
    for (const dependency of manifest.requires ?? []) assert(dependency.kind !== "external", `${label}: Runtime owns external dependencies`);
  }
  if (manifest.execution === "web-projection") {
    same(manifest.type, "wasm", `${label}: projection type`);
    same(manifest.runtime_abi, "elastos.runtime-projection/v1", `${label}: projection ABI`);
    same(manifest.bus_contract, "elastos.runtime-projection/v1", `${label}: projection bus`);
    assert(manifest.wit_world_sha256 === undefined, `${label}: web projection uses Runtime ABI`);
    assert(requireList(manifest.projections, `${label}: projections`).includes("web"), `${label}: web projection is missing`);
  }
  if (manifest.interfaces !== undefined) {
    const interfaces = requireList(manifest.interfaces, `${label}: interfaces`);
    assert(interfaces.length > 0, `${label}: interfaces are empty`);
    unique(interfaces.map((iface) => iface.id), `${label}: interface ids`);
    for (const iface of interfaces) {
      for (const key of ["id", "version", "description"]) present(iface[key], `${label}: interface ${key}`);
      const methods = requireList(iface.methods, `${label}: ${iface.id} methods`);
      assert(methods.length > 0, `${label}: ${iface.id} methods are empty`);
      unique(methods.map((method) => method.id), `${label}: ${iface.id} method ids`);
      for (const method of methods) {
        for (const key of ["id", "description", "risk", "approval", "audit", "operation"]) present(method[key], `${label}: ${iface.id} method ${key}`);
        assert(resource(method.resource), `${label}: ${iface.id}.${method.id} resource`);
      }
    }
  } else {
    assert(["shell", "localhost-provider"].includes(manifest.name), `${label}: interfaces are missing`);
  }
}

// Security-relevant published affordances: risk, approval, audit, resource, operation.
const methodContracts = [
  ["home-gui", "elastos.shell.gui", "desktop.render", "read", "runtime_policy", "summary", "elastos://home/desktop", "render"],
  ["home-gui", "elastos.shell.gui", "capsule.open", "launch", "runtime_policy", "event", "elastos://capsules/*", "launch"],
  ["home-cli", "elastos.shell.cli", "facts.search", "read", "runtime_policy", "summary", "elastos://home/facts", "search"],
  ["home-cli", "elastos.shell.cli", "capsule.open", "launch", "runtime_policy", "event", "elastos://capsules/*", "launch"],
  ["browser", "elastos.browser.page", "page.open", "launch", "runtime_policy", "event", "elastos://browser/page", "open"],
  ["browser", "elastos.browser.page", "exit.select", "write", "runtime_policy", "event", "elastos://browser/exit", "select"],
  ["wallet", "elastos.wallet.accounts", "approval.sign", "payment", "user", "full", "elastos://wallet/sign", "sign"],
  ["inbox", "elastos.inbox.approvals", "request.approve", "privileged", "user", "full", "elastos://inbox/request", "approve"],
  ["services", "elastos.services.offers", "browser.exit.select", "write", "runtime_policy", "event", "elastos://browser/exit", "select"],
  ["people", "elastos.people.contacts", "people.summary", "read", "runtime_policy", "summary", "elastos://people", "read"],
  ["system", "elastos.system.shell", "shell.switch", "privileged", "user", "event", "elastos://home/shell", "switch"],
  ["library", "elastos.library.objects", "object.choose", "read", "user", "event"],
  ["documents", "elastos.documents.editor", "document.publish", "privileged", "user", "full"],
  ["archive-manager", "elastos.archive.viewer", "entry.extract", "write", "user", "event"],
  ["chat-room", "elastos.chat.room", "guest.approve", "privileged", "user", "full"],
  ["marketplace", "elastos.marketplace.catalog", "capsule.install", "privileged", "user", "full"],
  ["gba-ucity", "elastos.content.asset", "asset.open", "launch", "runtime_policy", "event"],
  ...["wallet-metamask", "wallet-unisat", "wallet-walletconnect"].map((name) => [name, "elastos.wallet.approval-method", "method.connect", "privileged", "user", "event"]),
  ["browser-engine-adapter", "elastos.provider.browser.engine", "launch", "launch", "runtime_policy", "event", "elastos://browser-engine/*", "launch"],
  ["exit-provider", "elastos.provider.exit", "open_stream", "write", "runtime_policy", "event", "elastos://exit/*", "open_stream"],
  ["net-provider", "elastos.provider.net", "stream", "write", "runtime_policy", "event", "elastos://net/*", "stream"],
  ["chain-provider", "elastos.provider.chain", "broadcast_transaction", "payment", "user", "full", "elastos://chain/*", "broadcast_transaction"],
  ["wallet-provider", "elastos.provider.wallet", "status", "read", "runtime_policy", "summary", "elastos://wallet/meta/status", "status"],
  ["key-provider", "elastos.provider.key", "release", "rights", "user", "full", "elastos://key/*", "release"],
  ["rights-provider", "elastos.provider.rights", "can_stream", "rights", "runtime_policy", "event", "elastos://rights/*", "can_stream"],
  ["object-provider", "elastos.provider.object", "delete_permanently", "privileged", "user", "full", "elastos://object/*", "delete_permanently"],
];
const schemaContracts = [
  ["documents", "elastos.documents.editor", "document.open", "input_schema", "elastos.documents.document-open/v1", "library_object"],
  ["archive-manager", "elastos.archive.viewer", "archive.open", "input_schema", "elastos.archive.open/v1", "library_object"],
  ["gba-emulator", "elastos.gba.emulator", "game.open", "input_schema", "elastos.gba.rom-open/v1", "content_capsule"],
  ["gba-ucity", "elastos.content.asset", "asset.open", "output_schema", "elastos.content.asset-opened/v1"],
];
const method = (manifests, name, iface, id) => {
  const value = manifests[name]?.interfaces?.find((entry) => entry.id === iface)?.methods?.find((entry) => entry.id === id);
  assert(value, `${name}: missing ${iface}.${id}`);
  return value;
};

export function validateContracts(manifests) {
  for (const [name, iface, id, ...values] of methodContracts) {
    const value = method(manifests, name, iface, id);
    values.forEach((expected, index) => same(value[["risk", "approval", "audit", "resource", "operation"][index]], expected, `${name}: ${id} contract`));
  }
  for (const [name, iface, id, key, schema, kind] of schemaContracts) {
    const value = method(manifests, name, iface, id)[key];
    same(value?.schema, schema, `${name}: ${id} ${key}`);
    if (kind) assert(value.accepts?.some((entry) => entry.kind === kind), `${name}: ${id} accepts ${kind}`);
  }
  same(manifests.home.role, "app", "Home host role");
  for (const name of ["home-cli", "home-gui"]) {
    const shell = manifests[name];
    same(shell.role, "shell", `${name}: shell role`);
    same(shell.execution, "web-projection", `${name}: execution`);
    same(shell.permissions, { storage: [], messaging: [] }, `${name}: Runtime owns storage and messaging`);
  }
  for (const key of ["runtime_abi", "bus_contract", "execution", "resources", "permissions"]) {
    same(manifests["home-gui"][key], manifests["home-cli"][key], `Home shells: ${key}`);
  }
  const commonMethods = (shell) => shell.interfaces.flatMap((iface) => iface.methods)
    .filter((entry) => ["capsule.open", "shell.switch"].includes(entry.id))
    .map(({ description, ...entry }) => entry).sort((a, b) => a.id.localeCompare(b.id));
  same(commonMethods(manifests["home-gui"]), commonMethods(manifests["home-cli"]), "Home shells: common authority");
  const browser = manifests.browser;
  same(browser.execution, "web-projection", "Browser execution");
  same(browser.entrypoint, "browser/index.html", "Browser entrypoint");
  same(browser.role, "app", "Browser role");
  for (const capability of ["page", "display", "exit", "profile", "wallet-bridge"]) assert(browser.capabilities.includes(`elastos://browser/${capability}`), `Browser: ${capability} capability`);
  for (const capability of browser.capabilities) assert(backend(capability) === "browser", `Browser: Runtime owns raw backend ${capability}`);
  for (const name of ["browser-engine-adapter", "exit-provider", "net-provider", "wallet-provider"]) assert(browser.requires.some((entry) => entry.name === name && entry.kind === "capsule"), `Browser: ${name} dependency`);
  same(manifests["wallet-provider"].provides, "elastos://wallet/meta/status", "Wallet public namespace");
  same(manifests["wallet-provider"].interfaces.flatMap((iface) => iface.methods).map((entry) => entry.id), ["status"], "Wallet exposes read-only status");
  assert(manifests["did-provider"].authority.capabilities.every((entry) => !entry.operations?.includes("sign")), "DID provider uses typed signing intents");
  for (const name of ["people", "inbox", "wallet", "system"]) same(manifests[name].window_policy, "single", `${name}: single window policy`);
  same(manifests["gba-nonogram"].role, "content", "Nonogram role");
  same(manifests["gba-nonogram"].type, "data", "Nonogram type");
  same(manifests["gba-nonogram"].viewer, "gba-emulator", "Nonogram viewer");
}

const homeCore = [
  "shell", "localhost-provider", "did-provider", "chain-provider", "net-provider", "exit-provider",
  "browser-engine-adapter", "browser-engine-supervisor", "browser-native-proxy-engine", "browser-stream-bridge", "browser-local-exit",
  "webspace-provider", "object-provider", "wallet-provider", "model-provider",
  "home", "home-cli", "home-gui", "system", "services", "people", "browser", "documents", "library", "marketplace", "archive-manager", "inbox",
];
const protectedProviders = ["protected-content-protect-provider", "media-provider", "custody-provider", "protected-content-decrypt-provider"];
const obsolete = ["chat", "agent", "esp-shell", "capsule-inspector", "gba-engine-provider", "ai-provider", "llama-provider"];
const archiveAssets = ["home", "system", "services", "people", "documents", "library", "marketplace", "archive-manager", "inbox"];

export function parsePublishData(shellSource, rustSource) {
  const shell = (name) => {
    const body = shellSource.match(new RegExp(`^${name}=\\(([\\s\\S]*?)^\\)`, "m"))?.[1];
    assert(body !== undefined, `${name}: missing declared publish data`);
    return body.replace(/#[^\n]*/g, "").trim().split(/\s+/).filter(Boolean).map((entry) => entry.replace(/^(['"])(.*)\1$/, "$2"));
  };
  const rust = (name) => {
    const body = rustSource.match(new RegExp(`const\\s+${name}:\\s*&\\[&str\\]\\s*=\\s*&\\[([\\s\\S]*?)\\];`))?.[1];
    assert(body !== undefined, `${name}: missing declared publish data`);
    return body.replace(/\/\/[^\n]*/g, "").split(",").map((entry) => entry.trim()).filter(Boolean).map((entry) => JSON.parse(entry));
  };
  const lists = {
    shellDefault: shell("DEFAULT_CAPSULES"), shellRequired: shell("REQUIRED_SUPPORTED_CAPSULES"),
    rustHome: rust("HOME_PUBLISH_CAPSULES"), rustRequired: rust("REQUIRED_SUPPORTED_PUBLISH_CAPSULES"), rustDemo: rust("DEMO_PUBLISH_CAPSULES"),
  };
  for (const [name, values] of Object.entries(lists)) {
    assert(values.length > 0 && values.every((value) => typeof value === "string" && /^[a-z0-9][a-z0-9-]*$/.test(value)), `${name}: invalid publish capsule data`);
    unique(values, `${name}: publish capsules`);
  }
  return lists;
}

export function validatePublishData(components, manifests, lists) {
  const installedHome = components.profiles.home.components.filter((name) => Object.hasOwn(manifests, name)).sort();
  for (const name of ["shellDefault", "rustHome"]) same([...lists[name]].sort(), installedHome, `${name}: published capsules must match the Home profile`);
  for (const name of ["shellRequired", "rustRequired"]) {
    for (const capsule of installedHome) assert(lists[name].includes(capsule), `${name}: missing Home capsule ${capsule}`);
  }
  same([...lists.shellRequired].sort(), [...lists.rustRequired].sort(), "Shell and Rust supported publish capsules must match");
  for (const [name, values] of Object.entries(lists)) {
    unique(values, `${name}: publish capsules`);
    for (const capsule of values) assert(Object.hasOwn(manifests, capsule) && !obsolete.includes(capsule), `${name}: unknown or retired publish capsule ${capsule}`);
  }
  for (const capsule of ["gba-emulator", "gba-ucity", "chat-room", "tunnel-provider"]) assert(lists.rustDemo.includes(capsule), `rustDemo: missing demo capsule ${capsule}`);
  for (const capsule of lists.rustDemo) assert(components.profiles.demo.components.includes(capsule) && !installedHome.includes(capsule), `rustDemo: ${capsule} belongs in the demo profile`);
}

export function validateComponents(components, manifests) {
  same(components.schema, "elastos.components/v1", "Components schema");
  assert(object(components.external) && object(components.capsules) && object(components.profiles), "Components need external, capsules and profiles maps");
  const known = new Set([...Object.keys(components.external), ...Object.keys(components.capsules)]);
  for (const name of obsolete) assert(!known.has(name), `Retired component ${name} is packaged`);
  for (const [name, profile] of Object.entries(components.profiles)) {
    const list = requireList(profile.components, `${name}: components`);
    // full has a historical duplicate ipfs-provider; preserve this accepted profile for now.
    const installed = new Set(list);
    for (const entry of list) assert(known.has(entry) && !obsolete.includes(entry), `${name}: unknown component ${entry}`);
    if (["wallet", "wallet-metamask", "wallet-unisat", "wallet-walletconnect", "browser", "inbox"].some((entry) => installed.has(entry))) {
      for (const dependency of ["chain-provider", "wallet-provider"]) assert(installed.has(dependency), `${name}: missing Wallet/Browser dependency ${dependency}`);
    }
    if (["library", "marketplace", "elacity-player"].every((entry) => installed.has(entry))) {
      for (const dependency of ["chain-provider", "wallet-provider", "kubo", "ipfs-provider", ...protectedProviders.filter((entry) => entry !== "custody-provider")]) {
        assert.equal(list.filter((entry) => entry === dependency).length, 1, `${name}: protected-content dependency ${dependency} must occur once`);
      }
    }
  }
  for (const [profile, required] of [
    ["home", homeCore],
    ["demo", [...homeCore, "kubo", "ipfs-provider", "site-provider", "tunnel-provider", "chat-room", "cloudflared", "gba-emulator", "gba-ucity"]],
    ["blockchain", ["shell", "localhost-provider", "did-provider", "chain-provider", "wallet-provider", "drm-provider", "rights-provider", "key-provider", "decrypt-provider", ...protectedProviders]],
  ]) {
    const list = components.profiles[profile]?.components ?? [];
    for (const name of required) assert(list.includes(name), `${profile}: required component ${name}`);
  }
  const home = components.profiles.home.components;
  for (const name of ["availability-provider", "site-provider", "tunnel-provider", "cloudflared", "drm-provider", "rights-provider", "key-provider", "decrypt-provider"]) assert(!home.includes(name), `Home: ${name} belongs to an explicit profile`);
  // Local AI is optional: Assistant fetches the engine and models from the signed release on demand (#217).
  // model-provider is the Runtime's model broker, not a model.
  const localAi = home.filter((name) => name === "llama-server" || (name.startsWith("model-") && name !== "model-provider"));
  assert(localAi.length === 0, `Home: local AI installs on demand, found ${localAi.join(", ")}`);
  for (const name of ["gba-emulator", "gba-ucity"]) assert(!home.includes(name) && components.profiles.demo.components.includes(name), `${name}: demo profile placement`);
  same(Object.entries(components.profiles).filter(([, profile]) => profile.components.includes("custody-provider")).map(([name]) => name).sort(), ["blockchain", "full"], "Custody profile placement");
  for (const [name, component] of Object.entries(components.external)) {
    assert(object(component.platforms) && Object.keys(component.platforms).length > 0, `${name}: release platforms`);
    for (const [platform, metadata] of Object.entries(component.platforms)) {
      assert(object(metadata), `${name}: ${platform} metadata`);
      if (["crosvm", "vmlinux", "cloudflared"].includes(name)) {
        assert(text(metadata.release_path) || text(metadata.url) || text(metadata.cid) || (metadata.strategy === "local-copy" && text(metadata.source)), `${name}: ${platform} artifact source`);
      } else present(metadata.release_path, `${name}: ${platform} release path`);
      const installPath = metadata.install_path ?? component.install_path;
      present(installPath, `${name}: ${platform} install path`);
      for (const path of [metadata.release_path, installPath, metadata.extract_path].filter((value) => value !== undefined)) safePath(path, `${name}: ${platform} artifact`);
      if (metadata.release_path?.endsWith(".tar.gz") && manifests[name]?.execution === "web-projection") present(metadata.extract_path, `${name}: ${platform} archive extraction path`);
    }
  }
  for (const name of [...homeCore, "drm-provider", "rights-provider", "key-provider", "decrypt-provider", "availability-provider"]) {
    for (const platform of ["linux-amd64", "linux-arm64"]) assert(components.external[name]?.platforms[platform] ?? components.external[name]?.platforms["*"], `${name}: release metadata for ${platform}`);
  }
  for (const name of archiveAssets) {
    for (const platform of ["linux-amd64", "linux-arm64"]) {
      const metadata = components.external[name]?.platforms[platform] ?? components.external[name]?.platforms["*"];
      present(metadata?.extract_path, `${name}: ${platform} archive extraction path`);
    }
  }
  for (const name of protectedProviders) {
    const provider = components.external[name]?.provider_runtime;
    assert(provider?.role === "provider" && provider.substrate === "native" && provider.runtime_abi === "elastos.provider-stdio/v1" && provider.execution === "native-provider", `${name}: Runtime-only native provider metadata`);
    assert(!components.capsules[name], `${name}: Runtime owns provider inventory`);
  }
}

export function checkProductData(repoRoot = root) {
  const json = (path) => JSON.parse(readFileSync(resolve(repoRoot, path), "utf8"));
  const manifests = {};
  const manifestPaths = [];
  for (const base of ["capsules", "elastos/capsules"]) {
    for (const name of readdirSync(resolve(repoRoot, base)).sort()) {
      const path = `${base}/${name}/capsule.json`;
      if (!existsSync(resolve(repoRoot, path))) continue;
      const manifest = json(path);
      validateCapsule(manifest, path);
      assert(!manifests[manifest.name], `${path}: duplicate capsule name`);
      manifests[manifest.name] = manifest;
      manifestPaths.push(path);
      if (manifest.execution === "web-projection") assert(existsSync(resolve(repoRoot, dirname(path), manifest.entrypoint)), `${path}: missing entrypoint`);
      if (manifest.icon !== undefined) {
        present(manifest.icon, `${path}: icon path`);
        const capsuleRoot = realpathSync(resolve(repoRoot, dirname(path)));
        const iconRoot = resolve(capsuleRoot, manifest.icon);
        assert(iconRoot.startsWith(capsuleRoot + sep), `${path}: icon stays in its capsule`);
        for (const size of [32, 64, 128, 256]) {
          const icon = resolve(iconRoot, `icon-${size}.png`);
          assert(existsSync(icon), `${path}: missing ${size}px icon`);
          assert(realpathSync(icon).startsWith(capsuleRoot + sep), `${path}: icon stays in its capsule`);
          const bytes = readFileSync(icon);
          same(bytes.subarray(0, 8).toString("hex"), "89504e470d0a1a0a", `${path}: ${size}px PNG signature`);
          same([bytes.readUInt32BE(16), bytes.readUInt32BE(20)], [size, size], `${path}: ${size}px icon dimensions`);
        }
      }
    }
  }
  validateContracts(manifests);
  const components = json("components.json");
  validateComponents(components, manifests);
  validatePublishData(components, manifests, parsePublishData(
    readFileSync(resolve(repoRoot, "scripts/publish-release.sh"), "utf8"),
    readFileSync(resolve(repoRoot, "elastos/crates/elastos-server/src/publish.rs"), "utf8"),
  ));
  const pwa = json("capsules/home/browser/manifest.webmanifest");
  same(pwa.display, "standalone", "Home PWA display");
  for (const size of [192, 512]) {
    const icon = pwa.icons.find((entry) => entry.sizes === `${size}x${size}` && entry.type === "image/png");
    assert(icon, `Home PWA ${size}px icon`);
    const path = resolve(repoRoot, "capsules/home/browser", icon.src);
    assert(path.startsWith(resolve(repoRoot, "capsules/home/browser") + sep), "Home PWA icon stays in its capsule");
    assert(realpathSync(path).startsWith(realpathSync(resolve(repoRoot, "capsules/home/browser")) + sep), "Home PWA icon stays in its capsule");
    const bytes = readFileSync(path);
    same(bytes.subarray(0, 8).toString("hex"), "89504e470d0a1a0a", `Home PWA ${size}px PNG signature`);
    same([bytes.readUInt32BE(16), bytes.readUInt32BE(20)], [size, size], `Home PWA ${size}px dimensions`);
  }
  return { capsules: manifestPaths.length, profiles: Object.keys(components.profiles).length, method_contracts: methodContracts.length };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    console.log(JSON.stringify({ ok: true, ...checkProductData(process.argv[2]) }));
  } catch (error) {
    console.error(`[product-data] ${error.message}`);
    process.exitCode = 1;
  }
}
