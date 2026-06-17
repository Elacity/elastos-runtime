// Capsule Inspector (Phase 1) — read-only object-centered view.
//
// Data flow: the UI reads the `elastos://inspect/*` endpoints (System-scope
// capability `elastos://inspect/*`) through the runtime bridge when present. When the
// bridge is absent (e.g. opened standalone in a plain browser for design
// review), it falls back to SAMPLE_DATA so the surface always renders. No
// write/sign/launch path exists anywhere in this capsule.

"use strict";

// ---------------------------------------------------------------------------
// Host adapter for the inspect surface.
//
// The capsule-facing contract is Carrier-shaped (Principle #4): a provider
// scheme (`inspect`), an operation, and a payload. The transport underneath is
// an adapter *below* the capsule contract — here the runtime's node-local
// control API (CARRIER.md "Where HTTP Fits" #1), the same door the `library`
// and `browser` capsules use. Swapping that transport (postMessage, native
// host, in-process) must not change capsule code, so all calls go through one
// `inspectInvoke(operation, payload)`.
// ---------------------------------------------------------------------------
const HOME_TOKEN = new URLSearchParams(globalThis.location?.search || "").get("home_token") || "";

async function inspectInvoke(operation, payload) {
  // Prefer an injected host bridge (native/postMessage) when present.
  const bridge = globalThis.elastos && globalThis.elastos.inspect;
  if (bridge && typeof bridge.invoke === "function") {
    return await bridge.invoke("elastos://inspect", operation, payload || {});
  }
  // Otherwise use the node-local control API adapter, exactly as library does:
  // POST /api/provider/inspect/<op> with the signed home launch token. The
  // gateway derives identity from that token (never from page input) and maps
  // the authenticated app to an inspect scope before dispatching.
  if (!HOME_TOKEN) return null;
  const res = await fetch("/api/provider/inspect/" + encodeURIComponent(operation), {
    method: "POST",
    headers: { "content-type": "application/json", "x-elastos-home-token": HOME_TOKEN },
    body: JSON.stringify(payload || {}),
  });
  const envelope = await res.json().catch(() => ({}));
  if (!res.ok || envelope.status === "error") {
    throw new Error(
      envelope.message || envelope.error || `inspect ${operation} failed: ${res.status}`
    );
  }
  return envelope.data || envelope;
}

async function loadCapsuleList() {
  try {
    const live = await inspectInvoke("capsules", {});
    if (live && Array.isArray(live.capsules)) {
      setSourceBadge(true);
      // Scope is reported by the runtime ("system" | "self").
      setScopeBadge(live.scope || "system");
      return live.capsules;
    }
  } catch (err) {
    console.warn("inspect capsules failed, showing sample:", err);
  }
  setSourceBadge(false);
  // Sample data illustrates the privileged System view.
  setScopeBadge("system");
  return SAMPLE_DATA.map((c) => ({
    id: c.id, name: c.name, role: c.role, type: c.type, state: c.state,
  }));
}

async function loadCapsuleDetail(id) {
  try {
    const live = await inspectInvoke("capsule", { id });
    if (live && live.id) return live;
  } catch (err) {
    console.warn("inspect capsule failed, showing sample:", err);
  }
  return SAMPLE_DATA.find((c) => c.id === id) || null;
}

// Phase 2 (write): revoke a capability by token id. A System-admin mutation
// requiring a *write* inspect capability. Intentionally NOT driven from the
// read view — read summaries never carry bearer token ids (Principle #16); a
// dedicated System admin surface supplies the id and a write-scoped token.
async function inspectRevoke(tokenId) {
  return await inspectInvoke("revoke", { token_id: tokenId });
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------
function el(tag, attrs, children) {
  const node = document.createElement(tag);
  if (attrs) {
    for (const [k, v] of Object.entries(attrs)) {
      if (k === "class") node.className = v;
      else if (k === "text") node.textContent = v;
      else node.setAttribute(k, v);
    }
  }
  for (const child of children || []) {
    node.appendChild(typeof child === "string" ? document.createTextNode(child) : child);
  }
  return node;
}

function setSourceBadge(isLive) {
  const badge = document.getElementById("source-badge");
  badge.textContent = isLive ? "live" : "sample data";
  badge.className = "badge " + (isLive ? "badge-live" : "badge-sample");
}

function setScopeBadge(scope) {
  const badge = document.getElementById("scope-badge");
  const isSystem = scope === "system";
  badge.textContent = "scope: " + (isSystem ? "system (all capsules)" : "self only");
  badge.className = "badge " + (isSystem ? "badge-live" : "badge-sample");
}

function fmtTime(ts) {
  if (typeof ts !== "number" || !isFinite(ts) || ts <= 0) return "—";
  return new Date(ts * 1000).toISOString().replace("T", " ").slice(0, 19);
}

function fmtUptime(s) {
  if (!s && s !== 0) return "—";
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60);
  return h > 0 ? `${h}h ${m}m` : `${m}m`;
}

function card(title, body) {
  return el("div", { class: "card" }, [el("h3", { text: title }), body]);
}

function kv(pairs) {
  const dl = el("dl", { class: "kv" });
  for (const [k, v] of pairs) {
    dl.appendChild(el("dt", { text: k }));
    dl.appendChild(el("dd", { class: "mono" }, [String(v == null ? "—" : v)]));
  }
  return dl;
}

function renderList(capsules, activeId, onSelect) {
  const list = document.getElementById("capsule-list");
  list.innerHTML = "";
  for (const c of capsules) {
    const item = el("li", {
      class: "capsule-item" + (c.id === activeId ? " active" : ""),
    }, [
      el("div", {}, [
        el("div", { class: "ci-name", text: c.name }),
        el("div", { class: "ci-role", text: `${c.role} · ${c.type}` }),
      ]),
      el("span", { class: "state-pill state-" + (c.state || "stopped") }),
    ]);
    item.addEventListener("click", () => onSelect(c.id));
    list.appendChild(item);
  }
}

function renderAffordances(affordances) {
  const wrap = el("div");
  for (const a of affordances || []) {
    wrap.appendChild(el("div", { class: "row" }, [
      el("span", { class: "mono grow", text: `${a.interface} · ${a.id}` }),
      el("span", { class: "tag tag-" + a.risk, text: a.risk }),
      el("span", { class: "tag tag-" + a.approval, text: "approval: " + a.approval }),
      el("span", { class: "tag", text: "audit: " + a.audit }),
    ]));
  }
  if (!wrap.childNodes.length) wrap.appendChild(el("div", { class: "note", text: "No declared affordances." }));
  return wrap;
}

// Provider authority — the declarative powers a provider capsule is authorized
// for. This is what makes a provider's real capabilities (key release, decrypt
// render, rights decisions, chain broadcast) visible in the glass box.
function renderAuthority(authority) {
  const wrap = el("div");
  if (authority.reason) {
    wrap.appendChild(el("div", { class: "note", text: authority.reason }));
  }
  for (const cap of authority.capabilities || []) {
    const ops = (cap.operations || []).join(", ");
    const acts = (cap.actions || []).join(", ");
    wrap.appendChild(el("div", { class: "row" }, [
      el("span", { class: "mono grow", text: cap.resource }),
      el("span", { class: "tag tag-sign", text: acts }),
      el("span", { class: "tag mono", text: ops }),
    ]));
  }
  for (const ev of authority.audit_events || []) {
    wrap.appendChild(el("div", { class: "row" }, [
      el("span", { class: "mono", text: "audit: " + ev }),
    ]));
  }
  return wrap;
}

function renderRequired(caps) {
  const wrap = el("div");
  for (const r of caps || []) {
    wrap.appendChild(el("div", { class: "row" }, [el("span", { class: "mono", text: r })]));
  }
  return wrap;
}

function renderGranted(grants) {
  const wrap = el("div");
  for (const g of grants || []) {
    wrap.appendChild(el("div", { class: "row" }, [
      el("span", { class: "mono grow", text: `${g.resource} · ${g.action}` }),
      g.granted
        ? el("span", { class: "pill-ok", text: "✓ granted" })
        : el("span", { class: "pill-deny", text: "✗ denied" }),
      el("span", { class: "tag mono", text: g.token_id || "" }),
    ]));
  }
  return wrap;
}

function renderAudit(audit) {
  const wrap = el("div");
  const counts = (audit && audit.counts) || {};
  wrap.appendChild(el("div", { class: "note", text:
    `today: ${counts.total_today ?? 0} calls · ${counts.user_approved ?? 0} user-approved · ${counts.denied ?? 0} denied` }));
  for (const e of (audit && audit.recent) || []) {
    wrap.appendChild(el("div", { class: "audit-line" }, [
      el("span", { class: "ts", text: fmtTime(e.ts) }),
      el("span", { class: "mono", text: e.event }),
      el("span", { class: "mono", text: e.detail }),
      el("span", { class: e.success ? "pill-ok" : "pill-deny", text: e.success ? "ok" : "blocked" }),
    ]));
  }
  return wrap;
}

function renderProcesses(procs) {
  const wrap = el("div");
  for (const p of procs || []) {
    wrap.appendChild(el("div", { class: "row" }, [
      el("span", { class: "mono grow", text: `${p.kind} ${p.instance}` }),
      el("span", { class: "tag", text: `${p.memory_mb} MB` }),
      el("span", { class: "tag", text: "up " + fmtUptime(p.uptime_s) }),
    ]));
  }
  if (!wrap.childNodes.length) wrap.appendChild(el("div", { class: "note", text: "No running instances." }));
  return wrap;
}

function renderDetail(c) {
  const detail = document.getElementById("detail");
  detail.innerHTML = "";
  if (!c) {
    detail.appendChild(el("div", { class: "detail-empty", text: "Select a capsule to inspect." }));
    return;
  }

  detail.appendChild(el("div", { class: "detail-head" }, [
    el("h2", { text: c.name }),
    el("span", { class: "ver", text: "v" + (c.version || "?") }),
    el("span", { class: "tag", text: c.role }),
    el("span", { class: "tag", text: c.type }),
  ]));
  detail.appendChild(el("p", { class: "detail-sub", text: c.description || "" }));

  const id = c.identity || {};
  const prov = c.provenance || {};
  const carrier = c.carrier || {};

  // 1 identity + 2 manifest + 8 provenance
  const topGrid = el("div", { class: "grid2" }, [
    card("Identity / DID", kv([
      ["DID", id.did], ["CID", id.cid], ["trust level", id.trust_level],
      ["signed", id.signature_present ? "yes" : "no"], ["signed by", id.signed_by],
    ])),
    card("Manifest", kv([
      ["schema", c.manifest && c.manifest.schema], ["role", c.role],
      ["type", c.type], ["entrypoint", c.manifest && c.manifest.entrypoint],
      ["author", c.author],
    ])),
  ]);
  detail.appendChild(topGrid);

  // 3 affordances
  detail.appendChild(card("Affordances (slots / messages)", renderAffordances(c.affordances)));

  // Provider powers (for provider capsules that declare authority).
  if (c.authority && (c.authority.capabilities || c.authority.reason)) {
    detail.appendChild(card("Provider authority (powers)", renderAuthority(c.authority)));
  }

  // 4 + 5 capabilities
  detail.appendChild(el("div", { class: "grid2" }, [
    card("Required capabilities", renderRequired(c.required_capabilities)),
    card("Granted capabilities", renderGranted(c.granted_capabilities)),
  ]));

  // 6 storage + 7 carrier + 8 provenance
  detail.appendChild(el("div", { class: "grid2" }, [
    card("Storage namespaces", kv((c.storage_namespaces || []).map((s, i) => [`ns ${i + 1}`, s]))),
    card("Carrier endpoints", kv([
      ["enabled", carrier.enabled ? "yes" : "no"],
      ["peers", carrier.peers], ["endpoint", (carrier.endpoints || [])[0]],
    ])),
  ]));
  detail.appendChild(card("Provenance", kv([
    ["signed by", prov.signed_by], ["version", prov.version],
    ["installed", fmtTime(prov.installed_at)], ["CID", prov.cid],
  ])));

  // 9 audit + processes
  detail.appendChild(card("Audit log", renderAudit(c.audit)));
  detail.appendChild(card("Running processes", renderProcesses(c.processes)));
}

// ---------------------------------------------------------------------------
// Boot
// ---------------------------------------------------------------------------
let state = { capsules: [], activeId: null };

async function selectCapsule(id) {
  state.activeId = id;
  renderList(state.capsules, id, selectCapsule);
  renderDetail(await loadCapsuleDetail(id));
}

async function boot() {
  state.capsules = await loadCapsuleList();
  renderList(state.capsules, null, selectCapsule);
  if (state.capsules.length) selectCapsule(state.capsules[0].id);
  else renderDetail(null);
}

document.addEventListener("DOMContentLoaded", boot);

// ---------------------------------------------------------------------------
// Sample data — mirrors the elastos://inspect/* contract (docs/CAPSULE_INSPECTOR.md).
// Used only when no runtime bridge is present.
// ---------------------------------------------------------------------------
const SAMPLE_DATA = [
  {
    id: "cap_chat_room_01", name: "chat-room", version: "0.1.0", role: "shell", type: "wasm", state: "running",
    description: "Peer-to-peer chat room over Carrier gossip.", author: "elastos",
    identity: { did: "did:key:z6MkChatRoomDeviceKeyExample", cid: "bafychatroom...", trust_level: "verified", signature_present: true, signed_by: "gateway-did" },
    manifest: { schema: "elastos.capsule/v1", entrypoint: "chat.wasm" },
    affordances: [
      { interface: "elastos.chat/v1", id: "send", risk: "write", approval: "user", audit: "event", description: "Send a message" },
      { interface: "elastos.chat/v1", id: "history", risk: "read", approval: "none", audit: "summary", description: "Read history" },
    ],
    required_capabilities: ["elastos://carrier/*", "elastos://storage/chat"],
    granted_capabilities: [
      { resource: "elastos://carrier/*", action: "message", granted: true, token_id: "tok_c1a2", expiry: 1781990400 },
      { resource: "elastos://storage/chat", action: "write", granted: true, token_id: "tok_d3f4" },
      { resource: "elastos://did/*", action: "read", granted: false },
    ],
    storage_namespaces: ["localhost://WebSpaces/chat-room/"],
    carrier: { enabled: true, endpoints: ["gossip://iroh/abcd…"], peers: 1 },
    provenance: { signed_by: "gateway-did", version: "0.1.0", installed_at: 1781817600, cid: "bafychatroom..." },
    audit: { counts: { total_today: 14, user_approved: 2, denied: 1 }, recent: [
      { ts: 1781990100, event: "capability.use", detail: "carrier/* message", success: true },
      { ts: 1781990060, event: "capability.use", detail: "storage/chat write", success: true },
      { ts: 1781990050, event: "capability.denied", detail: "did/* read", success: false },
    ] },
    processes: [{ kind: "wasm", instance: "#4", memory_mb: 12, uptime_s: 10800 }],
  },
  {
    id: "cap_wallet_provider_01", name: "wallet-provider", version: "0.1.0", role: "provider", type: "microvm", state: "running",
    description: "Wallet proof bindings, approvals, and audit. Holds keys; exposes only typed affordances.", author: "elastos",
    identity: { did: "did:key:z6MkWalletProviderExample", cid: "bafywallet...", trust_level: "system", signature_present: true, signed_by: "gateway-did" },
    manifest: { schema: "elastos.capsule/v1", entrypoint: "rootfs.ext4" },
    affordances: [
      { interface: "elastos.wallet/v1", id: "accounts", risk: "read", approval: "none", audit: "summary", description: "List accounts" },
      { interface: "elastos.wallet/v1", id: "sign", risk: "sign", approval: "user", audit: "full", description: "Sign a transaction" },
      { interface: "elastos.wallet/v1", id: "delete", risk: "privileged", approval: "user", audit: "full", description: "Delete account" },
    ],
    required_capabilities: ["elastos://wallet/*"],
    authority: {
      reason: "Holds wallet keys; validates proof bindings and signs approved transactions without exposing key material.",
      capabilities: [
        { resource: "elastos://wallet/*", actions: ["read", "sign"], operations: ["accounts", "sign", "delete"] },
      ],
      audit_events: ["wallet.sign.denied", "wallet.delete"],
    },
    granted_capabilities: [
      { resource: "elastos://wallet/*", action: "read", granted: true, token_id: "tok_w9z0" },
    ],
    storage_namespaces: ["localhost://WebSpaces/wallet/"],
    carrier: { enabled: false, endpoints: [], peers: 0 },
    provenance: { signed_by: "gateway-did", version: "0.1.0", installed_at: 1781731200, cid: "bafywallet..." },
    audit: { counts: { total_today: 7, user_approved: 3, denied: 0 }, recent: [
      { ts: 1781989000, event: "capability.use", detail: "wallet/* read accounts", success: true },
      { ts: 1781988500, event: "affordance.sign", detail: "sign tx (user approved)", success: true },
    ] },
    processes: [{ kind: "microvm", instance: "vm#2", memory_mb: 64, uptime_s: 25200 }],
  },
  {
    id: "cap_capsule_inspector_01", name: "capsule-inspector", version: "0.1.0", role: "app", type: "wasm", state: "running",
    description: "This inspector — subject to the same rules it reveals.", author: "elastos",
    identity: { did: "did:key:z6MkInspectorExample", cid: "bafyinspector...", trust_level: "verified", signature_present: true, signed_by: "gateway-did" },
    manifest: { schema: "elastos.capsule/v1", entrypoint: "capsule-inspector.wasm" },
    affordances: [],
    required_capabilities: ["elastos://inspect/*"],
    granted_capabilities: [
      { resource: "elastos://inspect/*", action: "read", granted: true, token_id: "tok_i0n1" },
    ],
    storage_namespaces: [],
    carrier: { enabled: false, endpoints: [], peers: 0 },
    provenance: { signed_by: "gateway-did", version: "0.1.0", installed_at: 1781990000, cid: "bafyinspector..." },
    audit: { counts: { total_today: 3, user_approved: 0, denied: 0 }, recent: [
      { ts: 1781990200, event: "capability.use", detail: "inspect/* capsules", success: true },
    ] },
    processes: [{ kind: "wasm", instance: "#1", memory_mb: 9, uptime_s: 200 }],
  },
];
