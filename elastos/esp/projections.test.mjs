import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  auditCountsView,
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

describe("trust and provenance projections", () => {
  it("projects runtime trust verdicts without re-deriving crypto", () => {
    assert.equal(trustMaterial({ trust_state: "cid-with-manifest-signature" }), "verified");
    assert.equal(trustMaterial({ trust_state: "local-manifest-signature" }), "verified");
    assert.equal(trustMaterial({ trust_state: "cid-without-manifest-signature" }), "content_addressed");
    assert.equal(trustMaterial({ trust_state: "local-dev" }), "unsigned");
    assert.equal(trustMaterial({ trust_state: "future" }), "unsigned");
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
    });
    const card = trustCard(capsule(), inspectObject());
    assert.equal(card.trust, "verified");
    assert.equal(card.provenance?.state, "signed");
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
        capabilities: [{ resource: "elastos://home/status", actions: ["read"] }],
        audit_events: ["home.status.approved"],
        execution: {
          schema: "elastos.inspect.execution-policy/v1",
          mode: "approved_dispatch",
          can_dispatch: true,
          can_mutate: true,
          approval_surface: "inbox",
        },
        provider_response: { status: "ok" },
      }),
      {
        state: "approved",
        operation: "status",
        target: "home",
        capability_count: 1,
        audit_events: ["home.status.approved"],
        approved_execution: true,
        provider_status: "ok",
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
      request_binding: {
        schema: "elastos.inspect.request-binding/v1",
        sha256: "abcdef1234567890",
        bytes: 64,
        truncated: false,
        preview: { op: "status" },
      },
    };
    const result = inspectActionRequestValidation(request);
    assert.equal(result.ok, true);
    assert.equal(result.binding.state, "bound");
    assert.equal(result.binding.hash_short, "abcdef123456");
    assert.equal(requestBindingView(request.request_binding).preview_available, true);
    assert.equal(gatePreviewIsPreviewOnly(request.plan), true);
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
      schema: "elastos.inspect.request-binding/v1",
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
      schema: "elastos.inspect.request-binding/v1",
      sha256: "abcdef1234567890",
      bytes: 4096,
      truncated: true,
      preview: null,
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
      request_binding: {
        schema: "elastos.inspect.request-binding/v1",
        sha256: "",
        bytes: undefined,
        truncated: false,
        preview: null,
      },
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
      trust: "verified",
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
    assert.equal(detail.trust.trust, "verified");
    assert.equal(detail.affordance_count, 1);
    assert.equal(detail.custody.state, "complete");
    assert.equal(detail.custody.audit.state, "attested");
  });

  it("renders capsule detail without Inspect facts as incomplete, not all-clear", () => {
    const detail = capsuleDetailView(capsule(), null);
    assert.equal(detail.trust.trust, "verified");
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
