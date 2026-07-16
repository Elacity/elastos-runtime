import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  ESP_FACT_DESCRIPTORS,
  ESP_FACT_OPERATIONS,
  ESP_PROTOCOL,
  ESP_SUPPORTED_SCHEMAS,
  ESP_TRANSPORT,
  ESP_TRANSPORT_SCOPE,
  ESP_VERB_DESCRIPTORS,
} from "./esp_v0.ts";

const packageDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(packageDir, "../..");

const read = (path) => readFileSync(resolve(repoRoot, path), "utf8");
const espTypes = read("elastos/esp/esp_v0.ts");
const helperFiles = [
  "elastos/esp/audit_views.ts",
  "elastos/esp/capsule_detail.ts",
  "elastos/esp/consent.ts",
  "elastos/esp/custody.ts",
  "elastos/esp/home_fleet.ts",
  "elastos/esp/shell_picker.ts",
  "elastos/esp/trust.ts",
  "elastos/esp/index.ts",
];
const packageJson = JSON.parse(read("elastos/esp/package.json"));
const gatewayRoutes = read("elastos/crates/elastos-server/src/api/gateway.rs");
const gatewayEsp = read("elastos/crates/elastos-server/src/api/gateway_esp.rs");
const gatewayCatalog = read("elastos/crates/elastos-server/src/api/gateway_capsule_catalog.rs");
const docs = read("docs/ESP_V0.md");
const projectionTests = read("elastos/esp/projections.test.mjs");

const escapeRegExp = (value) => value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

const rustConstArray = (source, name) => {
  const match = source.match(new RegExp(`const ${name}:[\\s\\S]*?= &\\[([\\s\\S]*?)\\n\\];`));
  assert.ok(match, `gateway_esp.rs must expose ${name}`);
  return match[1];
};

const parseRustStringArray = (name) =>
  [...rustConstArray(gatewayEsp, name).matchAll(/"([^"]+\/v\d+)"/g)].map(
    (match) => match[1],
  );

const parseRustStructArray = (name, typeName, fields) => {
  const block = rustConstArray(gatewayEsp, name);
  const rows = [...block.matchAll(new RegExp(`${typeName}\\s*\\{([\\s\\S]*?)\\n\\s*\\},`, "g"))];
  assert.ok(rows.length > 0, `gateway_esp.rs ${name} must include ${typeName} rows`);
  return rows.map((row) =>
    Object.fromEntries(
      fields.map((field) => {
        const match = row[1].match(new RegExp(`${field}: "([^"]*)"`));
        assert.ok(match, `gateway_esp.rs ${name} row must include ${field}`);
        return [field, match[1]];
      }),
    ),
  );
};

const parseDocSupportedSchemas = () => {
  const marker = "The currently served `supported_schemas` list is:";
  const start = docs.indexOf(marker);
  assert.notEqual(start, -1, "ESP docs must include supported_schemas section");
  const section = docs.slice(start + marker.length).split(/\n## /)[0];
  return [...section.matchAll(/^- `([^`]+)`/gm)].map((match) => match[1]);
};

const parseMarkdownTable = (sectionTitle) => {
  const match = docs.match(new RegExp(`## ${escapeRegExp(sectionTitle)}\\n([\\s\\S]*?)(?:\\n## |$)`));
  assert.ok(match, `ESP docs must include ${sectionTitle} table`);
  return match[1]
    .split("\n")
    .filter((line) => line.startsWith("|") && !line.includes("---"))
    .slice(1)
    .map((line) => line.split("|").slice(1, -1).map((cell) => cell.trim().replaceAll("`", "")));
};

const splitMethodRoute = (value) => {
  const [method, ...routeParts] = value.split(" ");
  const route = routeParts.join(" ");
  assert.ok(method && route, `ESP docs route cell must include method and route: ${value}`);
  return { method, route };
};

const parseDocProjectionFacts = () =>
  parseMarkdownTable("Projection Facts").map((cells) => {
    assert.equal(cells.length, 5, "Projection Facts rows must have 5 columns");
    const [, schema, operation, localRoute, auth] = cells;
    return { schema, operation, ...splitMethodRoute(localRoute), auth };
  });

const parseDocVerbTable = () =>
  parseMarkdownTable("Consent And Act Verbs").map((cells) => {
    assert.equal(cells.length, 4, "Consent And Act Verbs rows must have 4 columns");
    const [name, localRoute, auth, effect] = cells;
    return { name, ...splitMethodRoute(localRoute), auth, effect };
  });

const gatewayRoutePattern = ({ method, route }) => {
  if (route.startsWith("/api/provider/")) {
    assert.equal(method, "POST", `gateway provider proxy only supports POST for ${route}`);
    return /\.route\(\s*"\/api\/provider\/:scheme\/:op"[\s\S]{0,160}\bpost\(/;
  }
  return new RegExp(
    `\\.route\\(\\s*"${escapeRegExp(route)}"[\\s\\S]{0,160}\\b${method.toLowerCase()}\\(`,
  );
};

const assertGatewayRoutes = (descriptors) => {
  for (const descriptor of descriptors) {
    assert.ok(
      gatewayRoutePattern(descriptor).test(gatewayRoutes),
      `gateway.rs must route ${descriptor.method} ${descriptor.route}`,
    );
  }
};

const servedSchemas = parseRustStringArray("SUPPORTED_SCHEMAS");
assert.deepEqual(
  ESP_SUPPORTED_SCHEMAS,
  servedSchemas,
  "ESP TypeScript schema tags must match the served Runtime descriptor",
);
assert.deepEqual(
  parseDocSupportedSchemas(),
  servedSchemas,
  "ESP docs supported_schemas must match the served Runtime descriptor",
);

const servedFacts = parseRustStructArray("FACTS", "EspFact", [
  "family",
  "schema",
  "operation",
  "method",
  "route",
  "auth",
  "authority",
]);
const servedFactDescriptors = servedFacts.map(({ family, schema, operation, method, route }) => ({
  family,
  schema,
  operation,
  method,
  route,
}));
const servedOperations = servedFacts.map((fact) => fact.operation);
assert.deepEqual(
  Object.values(ESP_FACT_OPERATIONS),
  servedOperations,
  "ESP TypeScript fact operations must match the served Runtime descriptor",
);
assert.deepEqual(
  ESP_FACT_DESCRIPTORS,
  servedFactDescriptors,
  "ESP TypeScript fact descriptors must match the served Runtime descriptor",
);
assert.deepEqual(
  parseDocProjectionFacts(),
  servedFacts.map(({ schema, operation, method, route, auth }) => ({
    schema,
    operation,
    method,
    route,
    auth,
  })),
  "ESP docs Projection Facts table must match the served Runtime descriptor",
);

const servedVerbs = parseRustStructArray("VERBS", "EspVerb", [
  "name",
  "method",
  "route",
  "auth",
  "effect",
  "gate",
]);
assert.deepEqual(
  ESP_VERB_DESCRIPTORS,
  servedVerbs.map(({ name, method, route }) => ({ name, method, route })),
  "ESP TypeScript verb descriptors must match the served Runtime descriptor",
);
assert.deepEqual(
  parseDocVerbTable(),
  servedVerbs.map(({ name, method, route, auth, effect }) => ({
    name,
    method,
    route,
    auth,
    effect,
  })),
  "ESP docs Consent And Act Verbs table must match the served Runtime descriptor",
);
assertGatewayRoutes([
  ...servedFactDescriptors,
  ...servedVerbs.map(({ name, method, route }) => ({ name, method, route })),
]);

assert.equal(packageJson.private, true, "package must remain private");
assert.equal(packageJson.dependencies, undefined, "package must not add runtime dependencies");
assert.equal(packageJson.devDependencies, undefined, "package must not add dev dependencies");

assert.equal(ESP_PROTOCOL, "elastos-shell-protocol");
assert.equal(ESP_TRANSPORT, "http-json");
assert.equal(ESP_TRANSPORT_SCOPE, "local_runtime_adapter");
assert.ok(
  gatewayCatalog.includes('"elastos.capsules.invoke-result/v1"') &&
    espTypes.includes('"elastos.capsules.invoke-result/v1"'),
  "capsule invoke response schema must match gateway_capsule_catalog.rs",
);

for (const needle of [
  "export interface EspInitializeRequest",
  "export interface EspInitializeResponse",
  "export interface EspFactDescriptor",
  "export interface EspVerbDescriptor",
  "export interface CapsuleCatalogResponse",
  "export interface CapsuleInterfaceRegistryResponse",
  "export interface InspectCapsulesResponse",
  "export interface InspectObjectProjection",
  "export interface InspectGatePreview",
  "export interface InspectActionRequestResponse",
  "export interface InspectRequestBinding",
  "export interface InspectDispatchResult",
  "export interface CapsuleInterfaceInvokeRequest",
  "export interface InboxActionRequest",
]) {
  assert.ok(espTypes.includes(needle), `missing ${needle}`);
}

for (const [path, content] of helperFiles.map((path) => [path, read(path)])) {
  for (const needle of [
    "fetch(",
    "XMLHttpRequest",
    "WebSocket",
    "localStorage",
    "sessionStorage",
    "indexedDB",
    "crypto.",
    "privateKey",
    "secret",
    "home_token",
    "dispatch_approved",
    "invoke_provider",
    "send_raw",
    "ProviderRegistry",
  ]) {
    assert.ok(!content.includes(needle), `${path} must not contain authority pattern ${needle}`);
  }
}

for (const [path, content] of helperFiles
  .filter((path) => path !== "elastos/esp/index.ts")
  .map((path) => [path, read(path)])) {
  for (const match of content.matchAll(/export function ([A-Za-z0-9_]+)/g)) {
    assert.ok(
      projectionTests.includes(match[1]),
      `projections.test.mjs must cover ${match[1]} from ${path}`,
    );
  }
}

for (const needle of ["absent", "incomplete", "degraded", "never signed", "not be counted as healthy"]) {
  assert.ok(
    projectionTests.includes(needle),
    `projection tests must preserve fail-honest missing-fact wording: ${needle}`,
  );
}

for (const path of [
  "./audit_views.ts",
  "./capsule_detail.ts",
  "./consent.ts",
  "./custody.ts",
  "./home_fleet.ts",
  "./shell_picker.ts",
  "./trust.ts",
]) {
  assert.ok(read("elastos/esp/index.ts").includes(path), `index must export ${path}`);
}

for (const forbidden of [
  "affordance-consent-pending",
  "elastos.reach",
  "ReachFact",
  "AffordanceGrantReceipt",
  "RequestCapabilityInput",
  "ValidateAndConsume",
  "validate-and-consume",
  "standing grant",
  "shell marketplace",
  "EventSource",
  "SSE",
  "projection stream",
  "full second-shell",
  "fetch(",
  "localStorage",
  "crypto.subtle",
]) {
  assert.ok(
    !espTypes.includes(forbidden),
    `ESP type package must not include unsupported Flint surface: ${forbidden}`,
  );
}

assert.ok(
  docs.includes("transport_scope: \"local_runtime_adapter\""),
  "ESP docs must name the current local adapter scope",
);
assert.ok(
  docs.includes("A future Carrier adapter may expose the"),
  "ESP docs must preserve the future Carrier same-schema/same-gate boundary",
);

console.log("PASS esp v0 type package check");
