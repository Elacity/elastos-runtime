import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  gatePreviewAuditView,
  gatePreviewIsPreviewOnly,
  inspectActionRequestValidation,
  provenanceView,
  requestBindingView,
} from "../elastos/esp/index.ts";
import * as systemEsp from "../capsules/system/browser/esp-projections.mjs";

const adapterSource = readFileSync(
  new URL("../capsules/system/browser/esp-projections.mjs", import.meta.url),
  "utf8",
);
const systemSource = readFileSync(
  new URL("../capsules/system/browser/system.js", import.meta.url),
  "utf8",
);

for (const forbidden of [
  "fetch(",
  "localStorage",
  "sessionStorage",
  "dispatch_approved",
  "/api/",
  "x-elastos-home-token",
]) {
  assert(
    !adapterSource.includes(forbidden),
    `System ESP projection adapter must not contain authority marker ${forbidden}`,
  );
}

assert(
  systemSource.includes("binding.executable !== true"),
  "System must expose actions only from Runtime-derived executable bindings",
);
for (const forbidden of [
  "binding.state === \"provider-path-only\"",
  "binding.state === \"approval-required\"",
]) {
  assert(
    !systemSource.includes(forbidden),
    `System must not present non-executable binding state as an action: ${forbidden}`,
  );
}

const object = {
  schema: "elastos.inspect.object/v1",
  provenance: {
    author: "did:ela:author",
    cid: "bafyprofile",
    signature_present: true,
    signature_fingerprint: "abcd1234",
    signed_by: "did:ela:signer",
  },
};

const incompleteObject = {
  schema: "elastos.inspect.object/v1",
  provenance: {
    author: null,
    cid: null,
    signature_present: true,
    signed_by: null,
  },
};

const preview = {
  schema: "elastos.inspect.gate-preview/v1",
  mode: "provider_authority",
  id: "capsule:exit-provider",
  operation: "status",
  capabilities: [{ resource: "elastos://exit/*", actions: ["read"] }],
  audit_events: ["exit.status.requested"],
  execution: {
    schema: "elastos.inspect.execution-policy/v1",
    mode: "preview_only",
    can_dispatch: false,
    can_mutate: false,
    approval_surface: null,
  },
  dispatch: false,
};

const degradedPreview = {
  ...preview,
  execution: {
    ...preview.execution,
    can_dispatch: true,
  },
};

const binding = {
  schema: "elastos.esp.request-binding/v1",
  request_id: "inspect-approve-request:test",
  principal: "person:test",
  capsule: "capsule:exit-provider",
  interface: null,
  method: "status",
  resources: ["elastos://exit/*"],
  sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  bytes: 2,
  truncated: false,
  preview: {},
};

const request = {
  schema: "elastos.inspect.action-request/v1",
  status: "pending",
  request_id: "inspect-approve-request:test",
  id: "capsule:exit-provider",
  operation: "status",
  request: {},
  plan: preview,
  request_binding: binding,
};

const incompleteRequest = {
  ...request,
  request_id: "",
  plan: degradedPreview,
  request_binding: { ...binding, sha256: "", bytes: undefined },
};

assert.deepEqual(systemEsp.provenanceView(object), provenanceView(object));
assert.deepEqual(systemEsp.provenanceView(incompleteObject), provenanceView(incompleteObject));
assert.deepEqual(systemEsp.provenanceView(null), provenanceView(null));
assert.deepEqual(systemEsp.gatePreviewAuditView(preview), gatePreviewAuditView(preview));
assert.deepEqual(systemEsp.gatePreviewAuditView(degradedPreview), gatePreviewAuditView(degradedPreview));
assert.equal(systemEsp.gatePreviewIsPreviewOnly(preview), gatePreviewIsPreviewOnly(preview));
assert.equal(
  systemEsp.gatePreviewIsPreviewOnly(degradedPreview),
  gatePreviewIsPreviewOnly(degradedPreview),
);
assert.deepEqual(systemEsp.requestBindingView(binding), requestBindingView(binding));
assert.deepEqual(systemEsp.requestBindingView(null), requestBindingView(null));
assert.deepEqual(
  systemEsp.inspectActionRequestValidation(request, {}),
  inspectActionRequestValidation(request, {}),
);
assert.deepEqual(
  systemEsp.inspectActionRequestValidation(incompleteRequest),
  inspectActionRequestValidation(incompleteRequest),
);

console.log("System ESP projection adapter matches @elastos/esp projection helpers.");
