/* Preview mock agent provider — stable seams for fx4 → w1/w3.
   UI ≠ authority (Principle 16): never mints Carrier/Capsule grants,
   never calls live ai-provider. Label everything Preview · mock. */

const TIP = "home-20260724ci";

export const MOCK_REPLY =
  "I'm a local preview on this machine — not live inference yet.\n\n" +
  "I start with **no tools**. If you need Downloads or other capsule access, " +
  "you'll grant it explicitly (Inbox-style). Nothing ambient.\n\n" +
  "```text\nTools: none\nLocality: this device\n```";

/** @typedef {"none"|"pending"|"granted"|"denied"} CapState */

/** @type {Map<string, { id: string, label: string, state: CapState, tier: number }>} */
const capabilities = new Map([
  [
    "library.read",
    {
      id: "library.read",
      label: "Library · Read",
      state: "none",
      tier: 1,
    },
  ],
  [
    "wallet.sign",
    {
      id: "wallet.sign",
      label: "Wallet · Sign",
      state: "none",
      tier: 4,
    },
  ],
]);

/** @type {Map<string, { approvalId: string, toolId: string, args: object, status: string }>} */
const pendingApprovals = new Map();
let approvalSeq = 0;

export function listModels() {
  return [
    {
      id: "local-preview",
      label: "Local preview",
      tier: "preview",
      fitsDevice: true,
      tokensPerSecEstimate: null,
    },
    {
      id: "qwen-stub-small",
      label: "Small local (stub)",
      tier: "fits",
      fitsDevice: true,
      tokensPerSecEstimate: null,
    },
    {
      id: "spark-stub",
      label: "Needs Spark (stub)",
      tier: "unsupported",
      fitsDevice: false,
      tokensPerSecEstimate: null,
    },
  ];
}

export function listCapabilities() {
  return [...capabilities.values()].map((cap) => ({ ...cap }));
}

export function toolsSummaryLabel() {
  const caps = listCapabilities();
  const granted = caps.filter((c) => c.state === "granted");
  const pending = caps.filter((c) => c.state === "pending");
  if (granted.length) {
    return `Tools: ${granted.map((c) => c.label).join(", ")}`;
  }
  if (pending.length) {
    return `Tools: pending review`;
  }
  return "Tools: none";
}

/**
 * @param {{ toolId: string, args?: object }} opts
 * @returns {{ status: "needs_approval"|"denied"|"ok", approvalId?: string, toolId: string, label: string, summary: string, scope: string, preview: true, result?: string, reason?: string }}
 */
export function requestTool({ toolId, args = {} }) {
  const cap = capabilities.get(toolId);
  if (!cap) {
    return {
      status: "denied",
      toolId,
      label: toolId,
      summary: "Unknown tool",
      scope: "",
      preview: true,
      reason: "Unknown tool",
    };
  }
  if (cap.tier >= 4) {
    cap.state = "denied";
    return {
      status: "denied",
      toolId,
      label: cap.label,
      summary: "Wallet / signing needs a human ceremony — never Approve for me.",
      scope: "human ceremony only · never ambient",
      preview: true,
      reason: "tier-4-forbidden",
    };
  }
  if (cap.state === "granted") {
    return {
      status: "ok",
      toolId,
      label: cap.label,
      summary: summaryFor(toolId, args),
      scope: scopeFor(cap),
      preview: true,
      result: mockToolResult(toolId, args),
    };
  }
  if (cap.state === "pending") {
    const existing = [...pendingApprovals.values()].find((r) => r.toolId === toolId);
    if (existing) {
      return {
        status: "needs_approval",
        approvalId: existing.approvalId,
        toolId,
        label: cap.label,
        summary: summaryFor(toolId, existing.args || args),
        scope: scopeFor(cap),
        preview: true,
      };
    }
  }
  const approvalId = `mock-approval-${(approvalSeq += 1)}`;
  cap.state = "pending";
  pendingApprovals.set(approvalId, {
    approvalId,
    toolId,
    args,
    status: "pending",
  });
  return {
    status: "needs_approval",
    approvalId,
    toolId,
    label: cap.label,
    summary: summaryFor(toolId, args),
    scope: scopeFor(cap),
    preview: true,
  };
}

/**
 * Mock-only resolve. Does not call Capsule/Carrier.
 * @param {{ approvalId?: string, toolId?: string, decision: "allow_once"|"deny" }} opts
 */
export function resolveMockApproval({ approvalId, toolId, decision }) {
  let record = approvalId ? pendingApprovals.get(approvalId) : null;
  if (!record && toolId) {
    record = [...pendingApprovals.values()].find((r) => r.toolId === toolId) || null;
  }
  const id = record?.toolId || toolId;
  const cap = id ? capabilities.get(id) : null;
  if (!cap) {
    return { status: "denied", preview: true, reason: "Unknown approval" };
  }
  if (cap.tier >= 4) {
    cap.state = "denied";
    if (record) {
      pendingApprovals.delete(record.approvalId);
    }
    return {
      status: "denied",
      preview: true,
      toolId: cap.id,
      label: cap.label,
      reason: "tier-4-forbidden",
    };
  }
  if (decision === "allow_once") {
    cap.state = "granted";
    if (record) {
      pendingApprovals.delete(record.approvalId);
    }
    return {
      status: "ok",
      preview: true,
      toolId: cap.id,
      label: cap.label,
      result: mockToolResult(cap.id, record?.args || {}),
    };
  }
  cap.state = "denied";
  if (record) {
    pendingApprovals.delete(record.approvalId);
  }
  return {
    status: "denied",
    preview: true,
    toolId: cap.id,
    label: cap.label,
  };
}

/** Reset mock grants (session hydrate / leave harness). */
export function resetMockCapabilities() {
  for (const cap of capabilities.values()) {
    cap.state = "none";
  }
  pendingApprovals.clear();
}

/** Paint mock capability state from a persisted session grant (preview only). */
export function applyCapabilityState(toolId, state) {
  const cap = capabilities.get(toolId);
  if (!cap) {
    return;
  }
  if (state === "granted" || state === "denied" || state === "pending" || state === "none") {
    cap.state = state;
  }
}

/** Heuristic: user text that should demo a Library grant card. */
export function wantsLibraryTool(text) {
  const t = String(text || "").toLowerCase();
  return (
    t.includes("download") ||
    t.includes("library") ||
    t.includes("folder") ||
    t.includes("files in")
  );
}

export function wantsWalletTool(text) {
  const t = String(text || "").toLowerCase();
  return t.includes("wallet") || t.includes("sign") || t.includes("recovery");
}

function scopeFor(cap) {
  if (cap.tier >= 4) {
    return "human ceremony only · never ambient";
  }
  return "read-only · this session · revocable";
}

function summaryFor(toolId, args) {
  if (toolId === "library.read") {
    const path = args.path || "Downloads";
    return `Agent wants to list files in ${path}`;
  }
  if (toolId === "wallet.sign") {
    return "Agent wants Wallet signing power";
  }
  return `Agent wants ${toolId}`;
}

function mockToolResult(toolId, args) {
  if (toolId === "library.read") {
    const path = args.path || "Downloads";
    return (
      `Preview listing for ${path} (mock — not a real Library call):\n` +
      `· weekend-plan.md\n· photo-dump.zip\n· notes.txt`
    );
  }
  return "Preview result (mock).";
}

export function providerTip() {
  return TIP;
}
