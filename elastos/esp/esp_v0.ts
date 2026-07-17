/**
 * ESP v0 - ElastOS Shell Protocol shared shapes.
 *
 * This package is private and outside the trusted runtime core. It mirrors the
 * current JSON facts and verb bodies that the Runtime already serves; it does
 * not perform transport, signing, provider dispatch, token storage, or policy.
 */

export type JsonPrimitive = string | number | boolean | null;
export type JsonValue =
  | JsonPrimitive
  | JsonValue[]
  | { [key: string]: JsonValue };

export const ESP_PROTOCOL = "elastos-shell-protocol" as const;
export const ESP_VERSION = "0" as const;
export const ESP_TRANSPORT = "http-json" as const;
export const ESP_TRANSPORT_SCOPE = "local_runtime_adapter" as const;
export const ESP_INITIALIZE_SCHEMA = "elastos.esp.initialize/v0" as const;

export const ESP_AUTHORITY_INVARIANTS = [
  "Verification proves evidence only; it does not authorize or make a method executable.",
  "Declared risk is advisory metadata; Runtime bindings and route policy decide executability and authority.",
  "Missing trust, permission, binding, or policy evidence is unknown, never safe.",
  "Routes, frames, iframe placement, and HTTP success are transport or presentation facts, not authority.",
  "Effect completion requires an exact request binding and matching Runtime result receipt.",
] as const;

export const ESP_SCHEMA_TAGS = {
  capsuleCatalog: "elastos.capsules.catalog/v1",
  capsuleInterfaces: "elastos.capsules.interfaces/v1",
  inspectCapsules: "elastos.inspect.capsules/v1",
  inspectObject: "elastos.inspect.object/v1",
  inspectGatePreview: "elastos.inspect.gate-preview/v1",
  inspectActionRequest: "elastos.inspect.action-request/v1",
  inspectActionResult: "elastos.inspect.action-result/v1",
  requestBinding: "elastos.esp.request-binding/v1",
  inspectDispatchResult: "elastos.inspect.dispatch-result/v1",
  capsuleInvokeResult: "elastos.capsules.invoke-result/v1",
} as const;

export type EspSupportedSchema =
  (typeof ESP_SCHEMA_TAGS)[keyof typeof ESP_SCHEMA_TAGS];
export type EspFactSchema =
  | typeof ESP_SCHEMA_TAGS.capsuleCatalog
  | typeof ESP_SCHEMA_TAGS.capsuleInterfaces
  | typeof ESP_SCHEMA_TAGS.inspectCapsules
  | typeof ESP_SCHEMA_TAGS.inspectObject
  | typeof ESP_SCHEMA_TAGS.inspectGatePreview
  | typeof ESP_SCHEMA_TAGS.inspectActionRequest;
export type EspFlowSchema =
  | typeof ESP_SCHEMA_TAGS.requestBinding
  | typeof ESP_SCHEMA_TAGS.inspectActionResult
  | typeof ESP_SCHEMA_TAGS.inspectDispatchResult
  | typeof ESP_SCHEMA_TAGS.capsuleInvokeResult;

export const ESP_SUPPORTED_SCHEMAS: readonly EspSupportedSchema[] = [
  ESP_SCHEMA_TAGS.capsuleCatalog,
  ESP_SCHEMA_TAGS.capsuleInterfaces,
  ESP_SCHEMA_TAGS.inspectCapsules,
  ESP_SCHEMA_TAGS.inspectObject,
  ESP_SCHEMA_TAGS.inspectGatePreview,
  ESP_SCHEMA_TAGS.inspectActionRequest,
  ESP_SCHEMA_TAGS.inspectActionResult,
  ESP_SCHEMA_TAGS.requestBinding,
  ESP_SCHEMA_TAGS.inspectDispatchResult,
  ESP_SCHEMA_TAGS.capsuleInvokeResult,
] as const;

export const ESP_FACT_OPERATIONS = {
  capsuleCatalog: "capsules.catalog",
  capsuleInterfaces: "capsules.interfaces",
  inspectCapsules: "inspect.capsules",
  inspectObject: "inspect.object",
  inspectGatePreview: "inspect.gate_preview",
  inspectActionRequest: "inspect.request_act",
} as const;

export type EspFactOperation =
  (typeof ESP_FACT_OPERATIONS)[keyof typeof ESP_FACT_OPERATIONS];

export type EspHttpMethod = "GET" | "POST";

export type EspFactLocalRoute =
  | "/api/capsules/catalog"
  | "/api/capsules/interfaces"
  | "/api/provider/inspect/capsules"
  | "/api/provider/inspect/capsule"
  | "/api/provider/inspect/plan"
  | "/api/provider/inspect/request_act";

export type EspVerbLocalRoute =
  | "/api/provider/inspect/request_act"
  | "/api/apps/inbox/actions"
  | "/api/capsules/interfaces/invoke";

export type EspLocalRoute = EspFactLocalRoute | EspVerbLocalRoute;

export type EspVerbName =
  | "inspect.request_act"
  | "inbox.approve_inspect_action"
  | "inbox.deny_inspect_action"
  | "capsule.invoke_runtime_policy_affordance";

export interface EspInitializeRequest {
  esp_version?: typeof ESP_VERSION;
  accepts?: string[];
}

export interface EspFactDescriptor {
  family: string;
  schema: EspFactSchema;
  operation: EspFactOperation;
  method: EspHttpMethod;
  route: EspFactLocalRoute;
  auth: string;
  authority: string;
  [key: string]: unknown;
}

export interface EspVerbDescriptor {
  name: EspVerbName;
  method: EspHttpMethod;
  route: EspVerbLocalRoute;
  auth: string;
  effect: string;
  gate: string;
  [key: string]: unknown;
}

export const ESP_FACT_DESCRIPTORS = [
  {
    family: "capsule_catalog",
    schema: ESP_SCHEMA_TAGS.capsuleCatalog,
    operation: ESP_FACT_OPERATIONS.capsuleCatalog,
    method: "GET",
    route: "/api/capsules/catalog",
    auth: "Home, System, Marketplace, or launchable shell token",
    authority:
      "Read-only installed capsule and affordance projection; descriptors are not grants.",
  },
  {
    family: "capsule_interfaces",
    schema: ESP_SCHEMA_TAGS.capsuleInterfaces,
    operation: ESP_FACT_OPERATIONS.capsuleInterfaces,
    method: "GET",
    route: "/api/capsules/interfaces",
    auth: "Home, System, Marketplace, or launchable shell token",
    authority:
      "Read-only interface registry derived from installed capsule manifests.",
  },
  {
    family: "inspect_capsules",
    schema: ESP_SCHEMA_TAGS.inspectCapsules,
    operation: ESP_FACT_OPERATIONS.inspectCapsules,
    method: "POST",
    route: "/api/provider/inspect/capsules",
    auth: "System launch token",
    authority:
      "System mirror list; ordinary capsules do not receive the system-wide view.",
  },
  {
    family: "inspect_object",
    schema: ESP_SCHEMA_TAGS.inspectObject,
    operation: ESP_FACT_OPERATIONS.inspectObject,
    method: "POST",
    route: "/api/provider/inspect/capsule",
    auth: "System launch token",
    authority:
      "Redacted capsule/provider object projection with provenance fingerprint, not raw secrets.",
  },
  {
    family: "gate_preview",
    schema: ESP_SCHEMA_TAGS.inspectGatePreview,
    operation: ESP_FACT_OPERATIONS.inspectGatePreview,
    method: "POST",
    route: "/api/provider/inspect/plan",
    auth: "System launch token",
    authority: "Preview-only authority reflection; cannot dispatch or mutate.",
  },
  {
    family: "inspect_action_request",
    schema: ESP_SCHEMA_TAGS.inspectActionRequest,
    operation: ESP_FACT_OPERATIONS.inspectActionRequest,
    method: "POST",
    route: "/api/provider/inspect/request_act",
    auth: "System launch token",
    authority:
      "Creates a principal-bound Inbox approval request; no provider dispatch happens here.",
  },
] as const satisfies readonly EspFactDescriptor[];

export const ESP_VERB_DESCRIPTORS = [
  {
    name: "inspect.request_act",
    method: "POST",
    route: "/api/provider/inspect/request_act",
    auth: "System launch token",
    effect:
      "Stores a pending Inspector action request bound to its request ID, principal, capsule, method, resources, and body.",
    gate: "Inbox approval required before dispatch.",
  },
  {
    name: "inbox.approve_inspect_action",
    method: "POST",
    route: "/api/apps/inbox/actions",
    auth: "Inbox launch token plus fresh same-principal passkey Home token",
    effect:
      "Revalidates the exact request binding and authority plan, dispatches through ProviderRegistry, and returns a matching Runtime receipt.",
    gate:
      "Fails closed when request body, authority plan, principal, or passkey proof does not match.",
  },
  {
    name: "inbox.deny_inspect_action",
    method: "POST",
    route: "/api/apps/inbox/actions",
    auth: "Inbox launch token",
    effect:
      "Marks only the exactly bound Inspector action request denied and returns its matching receipt without dispatch.",
    gate: "Fail-safe direction only; denial never mutates the target provider.",
  },
  {
    name: "capsule.invoke_runtime_policy_affordance",
    method: "POST",
    route: "/api/capsules/interfaces/invoke",
    auth: "Target capsule launch token",
    effect:
      "Invokes only executable generic Runtime bindings and returns the exact request ID, principal, capsule, interface, method, resource, and body binding.",
    gate:
      "Provider-path-only, unbound, unknown, and approval-required operations fail closed.",
  },
] as const satisfies readonly EspVerbDescriptor[];

export interface EspInitializeResponse {
  schema: typeof ESP_INITIALIZE_SCHEMA;
  protocol: typeof ESP_PROTOCOL;
  esp_version: typeof ESP_VERSION;
  transport: typeof ESP_TRANSPORT;
  transport_scope: typeof ESP_TRANSPORT_SCOPE;
  supported_schemas: readonly EspSupportedSchema[];
  facts: readonly EspFactDescriptor[];
  verbs: readonly EspVerbDescriptor[];
  invariants: readonly string[];
  accepted: string[];
  unsupported: string[];
  [key: string]: unknown;
}

export interface EspUnsupportedVersionResponse {
  schema: typeof ESP_INITIALIZE_SCHEMA;
  status: "error";
  code: "unsupported_esp_version";
  supported: readonly [typeof ESP_VERSION];
  [key: string]: unknown;
}

export type CapsuleRole = "shell" | "app" | "viewer" | "provider" | "content";
export type CapsuleType = "wasm" | "microvm" | "oci" | "media" | "data";
export type CapsuleRuntimeAbi =
  | "elastos.runtime-projection/v1"
  | "elastos.component/v1"
  | "microvm-linux"
  | "data";
export type CapsuleExecution =
  | "web-projection"
  | "component"
  | "microvm"
  | "data";
export type CapsuleProjection =
  | "web"
  | "cli"
  | "terminal"
  | "facts"
  | "affordances"
  | "gates"
  | "audit-mirror"
  | "carrier"
  | "content";
export type RequirementKind = "capsule" | "external";
export type AffordanceApprovalMode = "none" | "runtime_policy" | "user";
export type AffordanceRisk =
  | "read"
  | "write"
  | "launch"
  | "payment"
  | "rights"
  | "actuator"
  | "privileged";
export type AffordanceAuditMode = "none" | "summary" | "event" | "full";

export interface CapsuleRequirementSummary {
  name: string;
  kind: RequirementKind;
  [key: string]: unknown;
}

export interface CapsuleAffordanceDescriptor {
  id: string;
  description?: string | null;
  risk: AffordanceRisk;
  approval: AffordanceApprovalMode;
  audit: AffordanceAuditMode;
  resource?: string | null;
  operation?: string | null;
  input_schema?: JsonValue;
  output_schema?: JsonValue;
  [key: string]: unknown;
}

export interface CapsuleInterfaceDescriptor {
  id: string;
  version: string;
  description?: string | null;
  methods: CapsuleAffordanceDescriptor[];
  [key: string]: unknown;
}

export interface CapsuleCatalogCounts {
  total: number;
  installed: number;
  launchable: number;
  interfaces: number;
  methods: number;
  apps: number;
  viewers: number;
  providers: number;
  content: number;
  shell: number;
  [key: string]: unknown;
}

export interface CapsuleCatalogPolicy {
  install_state: string;
  install_note: string;
  payment_state: string;
  payment_note: string;
  drm_state: string;
  drm_note: string;
  [key: string]: unknown;
}

export interface CapsuleSummary {
  name: string;
  version: string;
  title: string;
  description: string;
  author?: string | null;
  role: CapsuleRole;
  type: CapsuleType;
  runtime_abi?: CapsuleRuntimeAbi | null;
  bus_contract?: string | null;
  wit_world_sha256?: string | null;
  execution?: CapsuleExecution | null;
  projections?: CapsuleProjection[];
  category: string;
  state: string;
  installed: boolean;
  launchable: boolean;
  launch_target?: string | null;
  route?: string | null;
  provides?: string | null;
  requires?: CapsuleRequirementSummary[];
  capabilities?: string[];
  interfaces?: CapsuleInterfaceDescriptor[];
  viewer?: string | null;
  cid?: string | null;
  cid_state: string;
  signature_state: string;
  trust_state: string;
  payment_state: string;
  drm_state: string;
  source: string;
  install_path?: string | null;
  release_path?: string | null;
  repository?: string | null;
  [key: string]: unknown;
}

export interface CapsuleCatalogResponse {
  schema: typeof ESP_SCHEMA_TAGS.capsuleCatalog;
  counts: CapsuleCatalogCounts;
  capsules: CapsuleSummary[];
  policy: CapsuleCatalogPolicy;
  [key: string]: unknown;
}

export interface CapsuleInterfaceRegistryCounts {
  capsules: number;
  interfaces: number;
  methods: number;
  executable_methods: number;
  [key: string]: unknown;
}

export type CapsuleMethodBindingState =
  | "executable"
  | "approval-required"
  | "provider-path-only"
  | "unbound"
  | "handler-unavailable"
  | "descriptive-only"
  | string;

export interface CapsuleMethodBindingSummary {
  method: string;
  state: CapsuleMethodBindingState;
  handler_available: boolean;
  executable: boolean;
  handler_kind?: "runtime" | "provider" | string;
  handler?: string;
  required_action?: string;
  reason?: string;
  [key: string]: unknown;
}

export interface CapsuleInterfaceSummary {
  capsule: string;
  capsule_version: string;
  title: string;
  role: CapsuleRole;
  type: CapsuleType;
  runtime_abi?: CapsuleRuntimeAbi | null;
  bus_contract?: string | null;
  wit_world_sha256?: string | null;
  execution?: CapsuleExecution | null;
  projections?: CapsuleProjection[];
  cid?: string | null;
  trust_state: string;
  interface: CapsuleInterfaceDescriptor;
  bindings: CapsuleMethodBindingSummary[];
  [key: string]: unknown;
}

export interface CapsuleInterfaceRegistryPolicy {
  descriptor_state: string;
  descriptor_note: string;
  invocation_state: string;
  invocation_note: string;
  [key: string]: unknown;
}

export interface CapsuleInterfaceRegistryResponse {
  schema: typeof ESP_SCHEMA_TAGS.capsuleInterfaces;
  counts: CapsuleInterfaceRegistryCounts;
  interfaces: CapsuleInterfaceSummary[];
  policy: CapsuleInterfaceRegistryPolicy;
  [key: string]: unknown;
}

export interface InspectCapsulesEntry {
  id: string;
  name: string;
  kind: "provider" | "capsule" | string;
  state: string;
  type: string;
  [key: string]: unknown;
}

export interface InspectCapsulesResponse {
  schema: typeof ESP_SCHEMA_TAGS.inspectCapsules;
  capsules: InspectCapsulesEntry[];
  [key: string]: unknown;
}

export interface InspectObjectProjection {
  schema: typeof ESP_SCHEMA_TAGS.inspectObject;
  kind: "provider" | "capsule" | string;
  id: string;
  name: string;
  state: string;
  type: string;
  manifest: {
    schema: JsonValue;
    version: JsonValue;
    role: JsonValue;
    entrypoint: JsonValue;
    provides: JsonValue;
    [key: string]: unknown;
  };
  affordances: JsonValue[];
  required_capabilities: JsonValue[];
  granted_capabilities: JsonValue[] | null;
  storage_namespaces: JsonValue;
  carrier: {
    enabled: JsonValue;
    endpoints: JsonValue[];
    [key: string]: unknown;
  };
  authority:
    | {
        reason: JsonValue;
        capabilities: JsonValue;
        audit_events: JsonValue;
        [key: string]: unknown;
      }
    | JsonValue;
  provider_authority:
    | {
        reason: JsonValue;
        capabilities: JsonValue;
        audit_events: JsonValue;
        [key: string]: unknown;
      }
    | JsonValue;
  provenance: {
    author: JsonValue;
    cid: string | null;
    signature_present: boolean;
    signature_fingerprint?: string | null;
    signed_by: JsonValue;
    [key: string]: unknown;
  };
  trust_evidence: {
    schema: "elastos.inspect.trust-evidence/v1";
    trust_state: string;
    cid_state: string;
    signature_state: string;
    manifest_signature:
      | {
          state: "declared" | string;
          fingerprint: string;
          [key: string]: unknown;
        }
      | null;
    verified: boolean;
    verified_by: JsonValue;
    [key: string]: unknown;
  } | null;
  audit:
    | {
        counts: { total: number; denied: number; attested: number; [key: string]: unknown };
        recent: JsonValue[];
        [key: string]: unknown;
      }
    | null;
  spend_budget: JsonValue;
  intent_proof: JsonValue;
  audit_chain_attestation: JsonValue;
  processes: JsonValue[];
  [key: string]: unknown;
}

export interface InspectExecutionPolicy {
  schema: "elastos.inspect.execution-policy/v1";
  mode: "preview_only" | "approved_dispatch" | string;
  can_dispatch: boolean;
  can_mutate: boolean;
  approval_surface: string | null;
  [key: string]: unknown;
}

export interface InspectCapabilityProjection {
  resource: string;
  actions: string[];
  [key: string]: unknown;
}

export interface InspectGatePreview {
  schema: typeof ESP_SCHEMA_TAGS.inspectGatePreview;
  mode: "provider_resource" | "provider_authority" | string;
  provider?: string;
  id?: string;
  operation: string;
  capabilities: InspectCapabilityProjection[];
  audit_events?: string[];
  execution: InspectExecutionPolicy;
  dispatch: false;
  [key: string]: unknown;
}

export interface EspRequestBinding {
  schema: typeof ESP_SCHEMA_TAGS.requestBinding;
  request_id: string;
  principal: string;
  capsule: string;
  interface: string | null;
  method: string;
  resources: string[];
  sha256: string;
  bytes: number;
  truncated: boolean;
  preview: JsonValue;
  [key: string]: unknown;
}

export interface InspectActionRequestResponse {
  schema: typeof ESP_SCHEMA_TAGS.inspectActionRequest;
  status: "pending";
  request_id: string;
  id: string;
  operation: string;
  plan: InspectGatePreview | JsonValue;
  request_binding?: EspRequestBinding;
  [key: string]: unknown;
}

export interface InspectDispatchResult {
  schema: typeof ESP_SCHEMA_TAGS.inspectDispatchResult;
  mode: "provider_authority" | string;
  id: string;
  provider: string;
  target: string;
  operation: string;
  request_binding: EspRequestBinding;
  capabilities: InspectCapabilityProjection[];
  audit_events: string[];
  execution: InspectExecutionPolicy;
  provider_response: JsonValue;
  [key: string]: unknown;
}

export interface ProviderOk<T> {
  status: "ok";
  data: T;
  [key: string]: unknown;
}

export interface ProviderError {
  status: "error";
  code: string;
  message: string;
  [key: string]: unknown;
}

export type InspectCapsulesProviderResponse =
  | ProviderOk<InspectCapsulesResponse>
  | ProviderError;
export type InspectObjectProviderResponse =
  | ProviderOk<InspectObjectProjection>
  | ProviderError;
export type InspectGatePreviewProviderResponse =
  | ProviderOk<InspectGatePreview>
  | ProviderError;
export type InspectDispatchProviderResponse =
  | ProviderOk<InspectDispatchResult>
  | ProviderError;

export interface InspectPlanRequest {
  id?: string;
  capsule_id?: string;
  scheme?: string;
  operation: string;
  request?: JsonValue;
}

export interface InspectActionRequestInput {
  id: string;
  operation: string;
  request?: { [key: string]: JsonValue };
}

export interface InboxActionRequest {
  action_id: string;
  home_token?: string;
}

export interface InspectActionResult {
  schema: typeof ESP_SCHEMA_TAGS.inspectActionResult;
  status: "completed" | "denied";
  request_id: string;
  request_binding: EspRequestBinding;
  dispatch_result: InspectDispatchResult | null;
  [key: string]: unknown;
}

export interface InboxActionResponse {
  message: string;
  result?: InspectActionResult;
  [key: string]: unknown;
}

export interface CapsuleInterfaceInvokeRequest {
  request_id: string;
  capsule: string;
  interface: string;
  method: string;
  input?: JsonValue;
}

export interface CapsuleInterfaceInvokeSuccess {
  schema: "elastos.capsules.invoke-result/v1";
  status: "ok";
  capsule: string;
  interface: string;
  method: string;
  request_id: string;
  request_binding: EspRequestBinding;
  output: JsonValue;
  [key: string]: unknown;
}

export interface CapsuleInterfaceInvokeError {
  schema: "elastos.capsules.invoke-result/v1";
  status: "error";
  code: string;
  message: string;
  capsule: string;
  interface: string;
  method: string;
  request_id: string;
  [key: string]: unknown;
}

export type CapsuleInterfaceInvokeResponse =
  | CapsuleInterfaceInvokeSuccess
  | CapsuleInterfaceInvokeError;
