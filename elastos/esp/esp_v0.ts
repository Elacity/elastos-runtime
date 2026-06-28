/**
 * ESP v0 — ElastOS Shell Protocol, shared TypeScript types.
 *
 * These types are EXTRACTED from the runtime's shipped serde shapes — they are
 * the contract, not an aspiration. Each type cites the Rust struct + file it
 * mirrors. The wire JSON is what those structs serialize, so enum values are the
 * serde `rename_all = "snake_case"` forms (lowercase / snake_case), and fields
 * marked optional are `#[serde(skip_serializing_if = ...)]` or `#[serde(default)]`
 * on the Rust side.
 *
 * Forward-compatibility rule (FACTS): a shell MUST ignore unknown fields on any
 * projection fact it reads, so the runtime can add fields without breaking it.
 * (Caveat: the runtime's *verb input* structs currently use
 * `#[serde(deny_unknown_fields)]` and reject unknown keys — see ESP_V0.md.)
 *
 * Spec: docs/ESP_V0.md. Sync: this file is hand-maintained against the Rust
 * structs; the alignment gate pins the route strings + schema tags so the doc
 * cannot silently drift. A future slice may codegen this from the serde types.
 */

// ─────────────────────────── Schema tags (versioning) ───────────────────────
// Versioning in ESP v0 is per-fact: every fact carries a `schema` tag of the form
// `elastos.<family>/vN`. A consumer keys off the tag, never a route shape.
export const ESP_SCHEMA_TAGS = {
  capsuleCatalog: "elastos.capsules.catalog/v1",
  capsuleInterfaces: "elastos.capsules.interfaces/v1",
  affordanceConsentPending: "elastos.capsules.affordance-consent-pending/v1",
  reachDescriptor: "elastos.reach.v1",
  affordanceReceipt: "elastos.affordance.receipt.v1",
} as const;

// ─────────────────────────── Reach (W0/W1) ──────────────────────────────────
// elastos-common/src/reach.rs — all enums are #[serde(rename_all = "snake_case")].

/** Network egress an act can perform. `allowlisted` (leashed) vs `open` (wide). */
export type EgressReach = "none" | "allowlisted" | "open";

/** The isolation boundary the capsule runs within (tightest → broadest). */
export type IsolationTier = "data" | "wasm" | "micro_vm" | "host_process";

/** How broad the touched resource is. */
export type ResourceScope = "object" | "collection" | "system" | "unknown";

/** Whether the act can be undone. */
export type Reversibility = "reversible" | "one_way" | "unknown";

/**
 * Core-COMPUTED reach of an affordance — the data behind the blast-radius halo.
 * Mirrors `ReachDescriptorV1` (elastos-common/src/reach.rs).
 *
 * DEGRADED in v0: `egress: "allowlisted"` is MODELED but not yet enforced at the
 * network boundary (W1b, needs KVM/CAP_NET_ADMIN); when `observed` is false, at
 * least one dimension could not be pinned — render the halo as "incomplete".
 */
export interface ReachDescriptorV1 {
  schema: typeof ESP_SCHEMA_TAGS.reachDescriptor;
  egress: EgressReach;
  isolation: IsolationTier;
  scope: ResourceScope;
  reversibility: Reversibility;
  observed: boolean;
}

/** A reach-scoped egress capability. Mirrors `EgressAllowlist` (reach.rs). */
export interface EgressAllowlist {
  allowed_hosts: string[];
  allowed_schemes: string[];
}

/** The capsule's self-declared risk (advisory). Manifest `AffordanceRisk`. */
export type AffordanceRisk =
  | "read"
  | "write"
  | "launch"
  | "payment"
  | "rights"
  | "actuator"
  | "privileged";

/** Whether the runtime can run the affordance without human approval. */
export type AffordanceApprovalMode = "runtime_policy" | "user";

// ─────────────────────────── Fact family 1+2: capsule catalog / interfaces ───
// gateway_capsule_catalog.rs. The catalog is the shell's read-only inventory of
// installed capsules and their declared affordances + core-derived reach.

/**
 * Core-derived reach for one declared affordance, projected ALONGSIDE the pure
 * manifest descriptor. Mirrors `AffordanceReachView`. `declared_understates_reach`
 * is the "a clone must lie" flag (claims low, reaches far).
 */
export interface AffordanceReachView {
  interface_id: string;
  method_id: string;
  risk: AffordanceRisk;
  reach: ReachDescriptorV1;
  declared_understates_reach: boolean;
}

/**
 * One installed capsule in the catalog. Mirrors the shell-relevant subset of
 * `CapsuleSummary` (gateway_capsule_catalog.rs); the runtime emits more fields,
 * and a shell MUST ignore the ones it does not use.
 */
export interface CapsuleSummary {
  name: string;
  version: string;
  title: string;
  description: string;
  role: string;
  /** Wire key is `type` (serde rename). */
  type: string;
  category: string;
  installed: boolean;
  launchable: boolean;
  interfaces?: CapsuleInterfaceDescriptor[];
  /** W0b: core-derived reach per declared affordance. */
  affordance_reach?: AffordanceReachView[];
  [key: string]: unknown; // must-ignore-unknown
}

/** A declared interface (subset). Mirrors `CapsuleInterfaceDescriptor`. */
export interface CapsuleInterfaceDescriptor {
  id: string;
  version?: string;
  methods: CapsuleAffordanceDescriptor[];
  [key: string]: unknown;
}

/** A declared affordance method (the PURE manifest declaration — no reach). */
export interface CapsuleAffordanceDescriptor {
  id: string;
  description?: string;
  risk: AffordanceRisk;
  approval: AffordanceApprovalMode;
  audit: string;
  resource?: string;
  operation?: string;
  [key: string]: unknown;
}

/** GET /api/capsules/catalog → `elastos.capsules.catalog/v1`. */
export interface CapsuleCatalogResponse {
  schema: typeof ESP_SCHEMA_TAGS.capsuleCatalog;
  capsules: CapsuleSummary[];
  [key: string]: unknown;
}

/** GET /api/capsules/interfaces → `elastos.capsules.interfaces/v1`. */
export interface CapsuleInterfaceSummary {
  capsule: string;
  capsule_version: string;
  interface: CapsuleInterfaceDescriptor;
  [key: string]: unknown;
}
export interface CapsuleInterfaceRegistryResponse {
  schema: typeof ESP_SCHEMA_TAGS.capsuleInterfaces;
  interfaces: CapsuleInterfaceSummary[];
  [key: string]: unknown;
}

// ─────────────────────────── Fact family 3: consent-pending (W2) ─────────────
// gateway_capsule_catalog.rs `AffordanceConsentPending`. Returned with HTTP 202
// from POST /api/capsules/interfaces/invoke for a consent-gated affordance.

export interface AffordanceConsentPending {
  schema: typeof ESP_SCHEMA_TAGS.affordanceConsentPending;
  /** Always "approval_pending". */
  status: string;
  request_id: string;
  resource: string;
  action: string;
  risk: AffordanceRisk;
  approval: AffordanceApprovalMode;
  capsule: string;
  interface: string;
  method: string;
  principal_id: string;
}

// ─────────────────────────── Fact family 4: signed receipt (W2 step 9) ───────
// elastos-runtime/src/capability/receipt.rs `AffordanceGrantReceiptV1`. The
// portable, verifiable proof of a redemption. `redeemed_at` is the runtime's
// SecureTimestamp serialization (opaque to the shell; treat as a timestamp).

export type SecureTimestamp = unknown;

export interface AffordanceGrantReceiptV1 {
  schema: typeof ESP_SCHEMA_TAGS.affordanceReceipt;
  capsule: string;
  method_id: string;
  input_hash: string;
  resource: string;
  action: string;
  token_id: string;
  redeemed_at: SecureTimestamp;
  /** Issuer ed25519 public key (hex) that signed this receipt. */
  signer: string;
  /** Ed25519 signature (base64) over the canonical receipt bytes. */
  signature: string;
}

// ─────────────────────────── Verbs (the 3 + invoke) ─────────────────────────
// NOTE: these INPUT bodies use `#[serde(deny_unknown_fields)]` on the runtime
// side — they REJECT unknown keys. The must-ignore-unknown rule is for FACTS the
// shell READS, not for these request bodies the shell SENDS. See ESP_V0.md.

/** POST /api/capability/request — handlers/capability.rs `RequestCapabilityInput`. */
export interface RequestCapabilityInput {
  resource: string;
  action: string;
  /** W2 affordance-consent binding — all four present together, or all absent. */
  capsule?: string;
  principal_id?: string;
  method_id?: string;
  input_hash?: string;
}
export interface RequestCapabilityOutput {
  /** "pending" | "granted" | "denied". */
  status: string;
  request_id?: string;
  token?: string;
  reason?: string;
}

/** POST /api/capability/validate-and-consume — `ValidateAndConsumeInput`. */
export interface ValidateAndConsumeInput {
  /** The granted affordance-consent token (base64). */
  token: string;
  method_id: string;
  resource: string;
  action: string;
  /** Re-hashed and compared to the token's binding. */
  input?: unknown;
}
export interface ValidateAndConsumeOutput {
  /** Always "consumed" on success. */
  status: string;
  receipt: AffordanceGrantReceiptV1;
}

/**
 * POST /api/capsules/interfaces/invoke — `CapsuleInterfaceInvokeRequest`.
 * First call (no `consent_token`) on a consent-gated affordance → 202
 * AffordanceConsentPending. Retry with the granted `consent_token` → redeem +
 * dispatch (DEGRADED in v0: the live gateway→runtime redeem round-trip is
 * integration-verified, not unit-tested — W5/journey).
 */
export interface CapsuleInterfaceInvokeRequest {
  capsule: string;
  interface: string;
  method: string;
  input?: unknown;
  consent_token?: string;
}

/**
 * The ESP v0 initialize handshake. DEFINED here as the forward contract but NOT
 * YET IMPLEMENTED by the runtime — a shell today reads the projection routes
 * directly. A future slice serves this so a shell can negotiate stream versions.
 */
export interface EspInitialize {
  esp_version: "0";
  /** Per-fact schema tags the client understands. */
  accepts: string[];
}
