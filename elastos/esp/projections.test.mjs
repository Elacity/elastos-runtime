import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  AUTHORITY_INVARIANT_FLAGS,
  auditCountsView,
  authorityInvariantView,
  capsuleDetailView,
  capsuleNeedsAttention,
  custodyView,
  dispatchResultAuditView,
  gatePreviewAuditView,
  gatePreviewIsPreviewOnly,
  homeCapsules,
  homeFleetView,
  homeFleetScope,
  inspectActionRequestValidation,
  inspectObjectsByName,
  isHomeCapsule,
  isInstalled,
  isShellSelectable,
  provenanceView,
  requestBindingView,
  selectableShells,
  shellIdentity,
  shellTrustCard,
  shellPicker,
  trustCard,
  trustMaterial,
  verificationState,
  withActiveShell,
} from "./index.ts";

function capsule(overrides = {}) {
  return {
    name: "home",
    version: "0.5.0",
    title: "Home",
    description: "Home shell",
    role: "shell",
    type: "wasm",
    category: "system",
    state: "installed",
    installed: true,
    launchable: true,
    requires: [],
    capabilities: [],
    interfaces: [
      {
        id: "shell.ui",
        version: "1",
        methods: [
          {
            id: "open",
            risk: "read",
            approval: "runtime_policy",
            audit: "event",
          },
        ],
      },
    ],
    cid_state: "present",
    signature_state: "signed",
    trust_state: "cid-with-manifest-signature",
    payment_state: "ok",
    drm_state: "none",
    source: "installed",
    ...overrides,
  };
}

function catalog(capsules) {
  return {
    schema: "elastos.capsules.catalog/v1",
    counts: {
      total: capsules.length,
      installed: capsules.filter((item) => item.installed).length,
      launchable: capsules.filter((item) => item.launchable).length,
      interfaces: 0,
      methods: 0,
      apps: 0,
      viewers: 0,
      providers: 0,
      content: 0,
      shell: 0,
    },
    capsules,
    policy: {
      install_state: "ok",
      install_note: "",
      payment_state: "ok",
      payment_note: "",
      drm_state: "ok",
      drm_note: "",
    },
  };
}

function inspectObject(overrides = {}) {
  return {
    schema: "elastos.inspect.object/v1",
    kind: "capsule",
    id: "capsule:home",
    name: "home",
    state: "running",
    type: "wasm",
    manifest: {
      schema: "elastos.capsule/v1",
      version: "0.5.0",
      role: "shell",
      entrypoint: "browser/index.html",
      provides: null,
    },
    affordances: [],
    required_capabilities: ["elastos://inspect/self"],
    granted_capabilities: [],
    storage_namespaces: { root: "localhost://UsersAI/home" },
    carrier: { enabled: true, endpoints: [] },
    authority: null,
    provenance: {
      author: "did:ela:author",
      cid: "bafyhome",
      signature_present: true,
      signature_fingerprint: "abc123",
      signed_by: null,
    },
    trust_evidence: {
      schema: "elastos.inspect.trust-evidence/v1",
      trust_state: "cid-with-manifest-signature",
      cid_state: "cid-published",
      signature_state: "manifest-signature-declared",
      manifest_signature: { state: "declared", fingerprint: "abc123" },
      verified: true,
      verified_by: "runtime-test",
    },
    audit: {
      counts: { total: 2, denied: 0, attested: 1 },
      recent: [{ event: "inspect.opened" }],
    },
    processes: [{ kind: "wasm", status: "running" }],
    ...overrides,
  };
}

function preview(overrides = {}) {
  return {
    schema: "elastos.inspect.gate-preview/v1",
    mode: "provider_authority",
    id: "capsule:home",
    operation: "status",
    capabilities: [{ resource: "elastos://home/status", actions: ["read"] }],
    audit_events: ["home.status.requested"],
    execution: {
      schema: "elastos.inspect.execution-policy/v1",
      mode: "preview_only",
      can_dispatch: false,
      can_mutate: false,
      approval_surface: null,
    },
    dispatch: false,
    ...overrides,
  };
}

function requestBinding(overrides = {}) {
  return {
    schema: "elastos.esp.request-binding/v1",
    request_id: "inspect-act-1",
    principal: "person:test",
    capsule: "capsule:home",
    interface: null,
    method: "status",
    resources: ["elastos://home/status"],
    sha256: "abcdef1234567890",
    bytes: 15,
    truncated: false,
    preview: { op: "status" },
    ...overrides,
  };
}

describe("trust and provenance projections", () => {
  it("projects trust material without inventing a verification verdict", () => {
    assert.equal(
      trustMaterial({ trust_state: "cid-with-manifest-signature" }),
      "signature_declared",
    );
    assert.equal(
      trustMaterial({ trust_state: "local-manifest-signature" }),
      "signature_declared",
    );
    assert.equal(trustMaterial({ trust_state: "cid-without-manifest-signature" }), "content_addressed");
    assert.equal(trustMaterial({ trust_state: "local-dev" }), "unsigned");
    assert.equal(trustMaterial({ trust_state: "future" }), "unknown");
    assert.equal(trustMaterial(null), "unknown");
    assert.equal(verificationState(inspectObject()), "verified");
    assert.equal(
      verificationState(
        inspectObject({ trust_evidence: { ...inspectObject().trust_evidence, verified: false } }),
      ),
      "unverified",
    );
    assert.equal(verificationState(null), "unknown");
  });

  it("renders provenance as projected facts", () => {
    const view = provenanceView(inspectObject());
    assert.equal(view.state, "signed");
    assert.equal(view.cid, "bafyhome");
    assert.equal(view.signature_present, true);
    assert.equal(view.signature_fingerprint, "abc123");
    assert.equal(view.signer_known, false);
  });

  it("renders missing or partial provenance as absent or incomplete, never signed", () => {
    assert.deepEqual(provenanceView(null), {
      state: "absent",
      author: null,
      cid: null,
      signature_present: false,
      signature_fingerprint: null,
      signer_known: false,
    });
    const incomplete = provenanceView(
      inspectObject({
        provenance: {
          author: null,
          cid: null,
          signature_present: true,
          signed_by: null,
        },
      }),
    );
    assert.equal(incomplete.state, "incomplete");
    assert.notEqual(incomplete.state, "signed");
  });

  it("builds trust cards without inventing provenance", () => {
    assert.deepEqual(trustCard(capsule({ trust_state: "local-dev" })), {
      name: "home",
      title: "Home",
      trust: "unsigned",
      verification: "unknown",
    });
    const card = trustCard(capsule(), inspectObject());
    assert.equal(card.trust, "signature_declared");
    assert.equal(card.verification, "verified");
    assert.equal(card.provenance?.state, "signed");
  });
});

describe("trust and authority separation", () => {
  const method = {
    risk: "read",
    approval: "runtime_policy",
  };
  const runtimeBinding = {
    state: "executable",
    handler_available: true,
    executable: true,
    handler_kind: "runtime",
    handler: "runtime.catalog.list",
  };

  it("keeps positive trust, permission, binding, and policy facts independent", () => {
    const view = authorityInvariantView(
      capsule({ capabilities: ["elastos://capsules/*"] }),
      inspectObject(),
      method,
      runtimeBinding,
    );
    assert.deepEqual(view.trust_evidence, {
      material: "signature_declared",
      verification: "verified",
    });
    assert.deepEqual(view.declared_permissions, {
      state: "declared",
      resources: ["elastos://capsules/*"],
    });
    assert.deepEqual(view.executable_binding, {
      state: "executable",
      executable: true,
      handler: "runtime.catalog.list",
    });
    assert.deepEqual(view.policy_gate, {
      state: "runtime-policy",
      declared_risk: "read",
      declared_risk_is_advisory: true,
    });
    assert.deepEqual(view.authorization, {
      state: "unknown",
      authorized: null,
      decided_by: "runtime-route-policy",
    });
    assert.deepEqual(view.invariants, AUTHORITY_INVARIANT_FLAGS);
  });

  it("does not turn verification into authorization or executability", () => {
    const view = authorityInvariantView(capsule(), inspectObject(), method);
    assert.equal(view.trust_evidence.verification, "verified");
    assert.equal(view.executable_binding.state, "unknown");
    assert.equal(view.executable_binding.executable, false);
    assert.equal(view.authorization.state, "unknown");
    assert.equal(view.authorization.authorized, null);
  });

  it("does not turn an executable binding into verification or authorization", () => {
    const view = authorityInvariantView(
      capsule({ trust_state: "local-dev" }),
      null,
      method,
      runtimeBinding,
    );
    assert.equal(view.trust_evidence.material, "unsigned");
    assert.equal(view.trust_evidence.verification, "unknown");
    assert.equal(view.executable_binding.executable, true);
    assert.equal(view.authorization.authorized, null);
  });

  it("keeps missing evidence unknown and declared risk advisory", () => {
    const missing = authorityInvariantView(
      capsule({ trust_state: "future", capabilities: undefined }),
    );
    assert.equal(missing.trust_evidence.material, "unknown");
    assert.equal(missing.trust_evidence.verification, "unknown");
    assert.equal(missing.declared_permissions.state, "unknown");
    assert.equal(missing.executable_binding.state, "unknown");
    assert.equal(missing.policy_gate.state, "unknown");
    assert.equal(missing.authorization.authorized, null);

    const providerOnly = authorityInvariantView(capsule(), null, method, {
      state: "provider-path-only",
      handler_available: true,
      executable: false,
      handler_kind: "provider",
      handler: "chain-provider",
    });
    assert.equal(providerOnly.policy_gate.declared_risk, "read");
    assert.equal(providerOnly.policy_gate.declared_risk_is_advisory, true);
    assert.equal(providerOnly.executable_binding.state, "non-executable");
    assert.equal(providerOnly.authorization.authorized, null);
  });

  it("rejects contradictory bindings and presentation signals as authority", () => {
    const inconsistent = authorityInvariantView(capsule(), inspectObject(), method, {
      ...runtimeBinding,
      handler_kind: "provider",
    });
    assert.equal(inconsistent.executable_binding.state, "inconsistent");
    assert.equal(inconsistent.executable_binding.executable, false);
    assert.equal(AUTHORITY_INVARIANT_FLAGS.route_grants_authority, false);
    assert.equal(AUTHORITY_INVARIANT_FLAGS.frame_grants_authority, false);
    assert.equal(AUTHORITY_INVARIANT_FLAGS.iframe_placement_grants_authority, false);
    assert.equal(AUTHORITY_INVARIANT_FLAGS.http_success_grants_authority, false);
  });
});

describe("custody and audit projections", () => {
  it("projects custody from current Inspect object facts only", () => {
    const view = custodyView(inspectObject());
    assert.equal(view.state, "complete");
    assert.equal(view.present, true);
    assert.equal(view.required_capabilities, 1);
    assert.equal(view.granted_capabilities, 0);
    assert.equal(view.storage_declared, true);
    assert.equal(view.carrier_declared, true);
    assert.deepEqual(view.processes, { total: 1, running: 1 });
    assert.equal(view.audit.state, "attested");
  });

  it("projects audit counts fail-honestly", () => {
    assert.deepEqual(auditCountsView(null), {
      present: false,
      total: 0,
      denied: 0,
      attested: 0,
      recent: [],
      state: "absent",
    });
    assert.equal(
      auditCountsView({ counts: { total: 3, denied: 1, attested: 0 }, recent: [] }).state,
      "denied",
    );
    assert.equal(
      auditCountsView({ counts: { total: Number.NaN, denied: -1, attested: 0 }, recent: "bad" }).state,
      "absent",
    );
  });

  it("renders missing or malformed custody as absent or incomplete", () => {
    const absent = custodyView(null);
    assert.equal(absent.state, "absent");
    assert.equal(absent.present, false);
    assert.equal(absent.audit.state, "absent");

    const malformed = custodyView({
      ...inspectObject(),
      required_capabilities: undefined,
      processes: undefined,
    });
    assert.equal(malformed.state, "incomplete");
    assert.equal(malformed.processes.running, 0);
    assert.notEqual(malformed.state, "complete");
  });

  it("renders stopped processes or denied audit as degraded, never healthy", () => {
    assert.equal(
      custodyView(inspectObject({ processes: [{ kind: "wasm", status: "stopped" }] })).state,
      "degraded",
    );
    assert.equal(
      custodyView(
        inspectObject({ audit: { counts: { total: 2, denied: 1, attested: 0 }, recent: [] } }),
      ).state,
      "degraded",
    );
  });

  it("projects gate preview and dispatch result audit summaries without dispatching", () => {
    assert.deepEqual(gatePreviewAuditView(preview()), {
      state: "preview",
      operation: "status",
      capability_count: 1,
      audit_events: ["home.status.requested"],
      preview_only: true,
      can_dispatch: false,
    });
    assert.deepEqual(
      dispatchResultAuditView({
        schema: "elastos.inspect.dispatch-result/v1",
        mode: "provider_authority",
        id: "capsule:home",
        provider: "home",
        target: "home",
        operation: "status",
        request_binding: requestBinding(),
        capabilities: [{ resource: "elastos://home/status", actions: ["read"] }],
        audit_events: ["home.status.approved"],
        execution: {
          schema: "elastos.inspect.execution-policy/v1",
          mode: "approved_dispatch",
          can_dispatch: true,
          can_mutate: true,
          approval_surface: "inbox",
        },
        provider_response: {
          status: "ok",
          _runtime_transfer: {
            schema: "elastos.provider.transfer/v1",
            source: "inspect",
            target: "home",
            op: "status",
            status: "completed",
          },
        },
      }),
      {
        state: "approved",
        operation: "status",
        target: "home",
        capability_count: 1,
        audit_events: ["home.status.approved"],
        approved_execution: true,
        provider_status: "ok",
        request_id: "inspect-act-1",
        request_bound: true,
      },
    );
  });

  it("renders missing preview or dispatch proof as degraded", () => {
    assert.deepEqual(gatePreviewAuditView(preview({ execution: undefined })), {
      state: "degraded",
      operation: "status",
      capability_count: 1,
      audit_events: ["home.status.requested"],
      preview_only: false,
      can_dispatch: false,
    });
    const degraded = dispatchResultAuditView({
      schema: "elastos.inspect.dispatch-result/v1",
      mode: "provider_authority",
      id: "capsule:home",
      provider: "home",
      target: "home",
      operation: "status",
      request_binding: requestBinding(),
      capabilities: [],
      audit_events: [],
      execution: {
        schema: "elastos.inspect.execution-policy/v1",
        mode: "approved_dispatch",
        can_dispatch: true,
        can_mutate: true,
        approval_surface: "inbox",
      },
      provider_response: {},
    });
    assert.equal(degraded.state, "degraded");
    assert.equal(degraded.provider_status, null);
  });

  it("does not approve an unrelated provider result", () => {
    const result = {
      schema: "elastos.inspect.dispatch-result/v1",
      mode: "provider_authority",
      id: "capsule:home",
      provider: "home",
      target: "home",
      operation: "status",
      request_binding: requestBinding(),
      capabilities: [{ resource: "elastos://home/status", actions: ["read"] }],
      audit_events: ["home.status.approved"],
      execution: {
        schema: "elastos.inspect.execution-policy/v1",
        mode: "approved_dispatch",
        can_dispatch: true,
        can_mutate: true,
        approval_surface: "inbox",
      },
      provider_response: {
        status: "ok",
        _runtime_transfer: {
          schema: "elastos.provider.transfer/v1",
          source: "inspect",
          target: "home",
          op: "status",
          status: "completed",
        },
      },
    };
    assert.equal(dispatchResultAuditView(result, requestBinding()).state, "approved");

    for (const mutate of [
      (value) => { value.request_binding.schema = "elastos.esp.request-binding/v999"; },
      (value) => { value.request_binding.request_id = "inspect-act-other"; },
      (value) => { value.request_binding.principal = "person:other"; },
      (value) => { value.request_binding.capsule = "capsule:other"; },
      (value) => { value.request_binding.interface = "elastos.other"; },
      (value) => { value.request_binding.method = "other"; },
      (value) => { value.request_binding.resources = ["elastos://other/*"]; },
      (value) => { value.request_binding.sha256 = "00".repeat(32); },
      (value) => { value.request_binding.bytes = 999; },
      (value) => { value.request_binding.truncated = true; },
      (value) => { value.request_binding.preview = { op: "other" }; },
      (value) => { value.provider_response._runtime_transfer.target = "other"; },
      (value) => { value.provider_response._runtime_transfer.op = "other"; },
      (value) => { value.provider_response._runtime_transfer.status = "unrelated"; },
    ]) {
      const unrelated = structuredClone(result);
      mutate(unrelated);
      assert.equal(dispatchResultAuditView(unrelated, requestBinding()).state, "degraded");
    }
  });
});

describe("consent validation", () => {
  it("validates the current Inspector request binding path", () => {
    const request = {
      schema: "elastos.inspect.action-request/v1",
      status: "pending",
      request_id: "inspect-act-1",
      id: "capsule:home",
      operation: "status",
      plan: preview(),
      request_binding: requestBinding(),
    };
    const result = inspectActionRequestValidation(request, { op: "status" });
    assert.equal(result.ok, true);
    assert.equal(result.binding.state, "bound");
    assert.equal(result.binding.hash_short, "abcdef123456");
    assert.equal(requestBindingView(request.request_binding).preview_available, true);
    assert.equal(gatePreviewIsPreviewOnly(request.plan), true);
  });

  it("rejects every unrelated Inspector request binding field", () => {
    const request = {
      schema: "elastos.inspect.action-request/v1",
      status: "pending",
      request_id: "inspect-act-1",
      id: "capsule:home",
      operation: "status",
      plan: preview(),
      request_binding: requestBinding(),
    };
    for (const [field, replacement] of [
      ["schema", "elastos.esp.request-binding/v999"],
      ["request_id", "inspect-act-other"],
      ["principal", ""],
      ["capsule", "capsule:other"],
      ["interface", "elastos.other"],
      ["method", "other"],
      ["resources", ["elastos://other/*"]],
      ["sha256", ""],
      ["bytes", 999],
      ["truncated", true],
      ["preview", { op: "other" }],
    ]) {
      const mutated = structuredClone(request);
      mutated.request_binding[field] = replacement;
      assert.equal(
        inspectActionRequestValidation(mutated, { op: "status" }).ok,
        false,
        `accepted mutated ${field}`,
      );
    }
  });

  it("rejects an unrelated bound request body", () => {
    const request = {
      schema: "elastos.inspect.action-request/v1",
      status: "pending",
      request_id: "inspect-act-1",
      id: "capsule:home",
      operation: "status",
      plan: preview(),
      request_binding: requestBinding({ bytes: 14, preview: { probe: true } }),
    };
    assert.equal(inspectActionRequestValidation(request, { probe: true }).ok, true);
    const unrelated = inspectActionRequestValidation(request, { probe: false });
    assert.equal(unrelated.ok, false);
    assert.ok(unrelated.reasons.includes("body_binding_mismatch"));
  });

  it("rejects non-preview plans and missing bindings", () => {
    const result = inspectActionRequestValidation({
      schema: "elastos.inspect.action-request/v1",
      status: "pending",
      request_id: "inspect-act-1",
      id: "capsule:home",
      operation: "status",
      plan: preview({ dispatch: true }),
    });
    assert.equal(result.ok, false);
    assert.deepEqual(result.reasons.sort(), ["missing_request_binding", "plan_not_preview_only"]);
  });

  it("renders missing and incomplete request bindings fail-honestly", () => {
    assert.deepEqual(requestBindingView(null), {
      state: "absent",
      present: false,
      bytes: 0,
      truncated: false,
      hash_short: "",
      preview_available: false,
    });
    const incomplete = requestBindingView({
      schema: "elastos.esp.request-binding/v1",
      request_id: "",
      principal: "",
      capsule: "",
      interface: null,
      method: "",
      resources: [],
      sha256: "",
      bytes: undefined,
      truncated: false,
      preview: null,
    });
    assert.equal(incomplete.state, "incomplete");
    assert.equal(incomplete.present, true);
    assert.equal(incomplete.preview_available, false);
  });

  it("marks truncated request bindings as degraded context, not full proof", () => {
    const truncated = requestBindingView({
      ...requestBinding({ bytes: 4096, truncated: true, preview: null }),
    });
    assert.equal(truncated.state, "truncated");
    assert.equal(truncated.preview_available, false);

    const validation = inspectActionRequestValidation({
      schema: "elastos.inspect.action-request/v1",
      status: "pending",
      request_id: "inspect-act-1",
      id: "capsule:home",
      operation: "status",
      plan: preview(),
      request_binding: requestBinding({ sha256: "", bytes: undefined, preview: null }),
    });
    assert.equal(validation.ok, false);
    assert.ok(validation.reasons.includes("incomplete_request_binding"));
    assert.ok(validation.reasons.includes("missing_binding_hash"));
  });
});

describe("shell picker", () => {
  it("selects only launchable shell-role capsules", () => {
    const catalogFact = catalog([
      capsule({ name: "home", role: "shell", launchable: true }),
      capsule({ name: "home-gui", title: "Home GUI", role: "shell", launchable: true }),
      capsule({ name: "home-cli", title: "Home CLI", role: "shell", launchable: true }),
      capsule({ name: "app", role: "app", launchable: true }),
      capsule({ name: "dev-shell", role: "shell", launchable: false }),
    ]);
    const picker = shellPicker(catalogFact, "home");
    assert.equal(isShellSelectable(catalogFact.capsules[0]), false);
    assert.equal(isShellSelectable(catalogFact.capsules[1]), true);
    assert.equal(isShellSelectable(catalogFact.capsules[3]), false);
    assert.deepEqual(selectableShells(catalogFact).map((item) => item.name), ["home-gui", "home-cli"]);
    assert.deepEqual(shellTrustCard(catalogFact.capsules[1]), {
      name: "home-gui",
      title: "Home GUI",
      trust: "signature_declared",
      verification: "unknown",
    });
    assert.deepEqual(picker.shells.map((shell) => shell.name), ["home-gui", "home-cli"]);
    assert.equal(picker.active, "home-gui");
    assert.equal(withActiveShell(picker, "app"), null);
    assert.equal(shellIdentity("home"), "home");
    assert.equal(withActiveShell(picker, "home"), null);
    assert.equal(withActiveShell(picker, "home-gui")?.active, "home-gui");
  });

  it("renders an empty or stale shell picker without choosing an invalid active shell", () => {
    const picker = shellPicker(catalog([capsule({ role: "app", launchable: true })]), "missing");
    assert.deepEqual(picker.shells, []);
    assert.equal(picker.active, "");
    assert.equal(withActiveShell(picker, "missing"), null);

    const two = shellPicker(
      catalog([
        capsule({ name: "home", role: "shell", launchable: true }),
        capsule({ name: "alt", role: "shell", launchable: true, trust_state: "local-dev" }),
      ]),
    );
    assert.deepEqual(two.shells.map((shell) => shell.name), ["alt"]);
    assert.equal(withActiveShell(two, "alt")?.active, "alt");
  });
});

describe("capsule detail and Home fleet", () => {
  it("composes capsule detail from catalog plus Inspect facts", () => {
    const detail = capsuleDetailView(capsule(), inspectObject());
    assert.equal(detail.name, "home");
    assert.equal(detail.trust.trust, "signature_declared");
    assert.equal(detail.trust.verification, "verified");
    assert.equal(detail.affordance_count, 1);
    assert.equal(detail.custody.state, "complete");
    assert.equal(detail.custody.audit.state, "attested");
  });

  it("renders capsule detail without Inspect facts as incomplete, not all-clear", () => {
    const detail = capsuleDetailView(capsule(), null);
    assert.equal(detail.trust.trust, "signature_declared");
    assert.equal(detail.trust.verification, "unknown");
    assert.equal(detail.custody.state, "absent");
    assert.equal(detail.audit.state, "absent");
    assert.equal(capsuleNeedsAttention(detail), true);
  });

  it("builds the installed user-facing Home fleet and attention count", () => {
    const catalogFact = catalog([
      capsule({ name: "home", role: "app", installed: true }),
      capsule({ name: "reader", role: "viewer", installed: true, trust_state: "local-dev" }),
      capsule({ name: "content", role: "content", installed: true }),
      capsule({ name: "library", role: "app", installed: false }),
    ]);
    const inspected = inspectObjectsByName([
      inspectObject({ name: "home" }),
      inspectObject({
        name: "reader",
        audit: { counts: { total: 1, denied: 1, attested: 0 }, recent: [] },
      }),
    ]);
    const view = homeFleetView(catalogFact, inspected);
    assert.deepEqual(view.capsules.map((entry) => entry.name), ["home", "reader"]);
    assert.equal(view.total, 2);
    assert.equal(view.needs_attention, 1);
  });

  it("covers Home fleet scope helpers and treats missing Inspect facts as attention", () => {
    assert.equal(isHomeCapsule("shell"), true);
    assert.equal(isHomeCapsule("app"), true);
    assert.equal(isHomeCapsule("viewer"), true);
    assert.equal(isHomeCapsule("provider"), false);
    assert.equal(isInstalled({ installed: true }), true);
    assert.equal(isInstalled({ installed: false }), false);
    assert.equal(isInstalled({}), false);

    const entries = [
      capsule({ name: "home", role: "app", installed: true }),
      capsule({ name: "library", role: "app", installed: false }),
      capsule({ name: "viewer", role: "viewer", installed: true }),
      capsule({ name: "provider", role: "provider", installed: true }),
    ];
    assert.deepEqual(homeCapsules(entries).map((entry) => entry.name), [
      "home",
      "library",
      "viewer",
    ]);
    assert.deepEqual(homeFleetScope(entries).map((entry) => entry.name), ["home", "viewer"]);

    const view = homeFleetView(catalog(entries), inspectObjectsByName([inspectObject({ name: "home" })]));
    assert.deepEqual(view.capsules.map((entry) => entry.name), ["home", "viewer"]);
    assert.equal(
      view.needs_attention,
      1,
      "viewer has no Inspect custody fact and must not be counted as healthy",
    );
    assert.equal(view.capsules[1].custody.state, "absent");
  });
});
