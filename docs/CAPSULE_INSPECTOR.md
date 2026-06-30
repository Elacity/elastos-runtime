# Capsule Inspector

Capsule Inspector is the Runtime-owned live object mirror for ElastOS capsules
and providers. It borrows the useful part of Self's object world: running
objects should be inspectable as objects, with identity, authority, provenance,
state, and affordances visible in one place.

It does not copy Self's trusted-process security model. Inspector is
fail-closed and permissioned:

- `elastos://inspect/*` is the privileged System-wide mirror.
- `elastos://inspect/self` is the pure SelfOnly scope rule and cannot cross
  capsule boundaries.
- Current product routing keeps `/api/provider/inspect/self` System-only until a
  caller-bound ordinary-capsule SelfOnly route is explicitly wired and tested.
- System can read object metadata and request an action approval.
- Ordinary capsules cannot call approved dispatch directly.
- `revoke` is intentionally not implemented.

The pure scope rules live in
[elastos-runtime/src/inspect/mod.rs](../elastos/crates/elastos-runtime/src/inspect/mod.rs).
The Runtime provider projection and gate preview live in
[inspect_provider.rs](../elastos/crates/elastos-server/src/inspect_provider.rs).
The Inbox approval and dispatch path lives in
[gateway_inspect_actions.rs](../elastos/crates/elastos-server/src/api/gateway_inspect_actions.rs).

## Object Model

Self maps neatly onto the ElastOS runtime model:

| Self idea | ElastOS shape |
| --- | --- |
| live object | capsule or provider |
| slot/message | typed affordance or provider operation |
| mirror | permissioned Inspector view |
| transporter | signed capsule package/provenance |
| Morphic world | Home/System live desktop |
| VM | Runtime execution substrate |

The Inspector projection returns metadata only:

- identity: capsule/provider id, name, kind, state
- manifest slice: schema, version, role, entrypoint, provided namespace
- authority: capabilities, actions, operations, audit events
- provenance: CID plus signature presence/fingerprint
- storage/carrier/process summary

Raw signatures, bearer tokens, host paths, Carrier tickets, runtime stream
descriptors, wallet/node authority, and mutation handles are not projected.

## Gate Preview

`plan` is the preview half. It reflects what a provider operation would require
before anything executes.

For manifest-backed providers, Inspector calls the same authority planner used
by Runtime provider invocation. A successful preview returns
`elastos.inspect.gate-preview/v1` with:

- the target provider/capsule
- the operation
- required resources and actions
- audit events
- execution policy `mode=preview_only`
- `can_dispatch=false`
- `can_mutate=false`
- `dispatch=false`

Provider-resource previews can also be built from a scheme and operation using
the canonical provider resource helpers. This keeps previews tied to the same
capability vocabulary as real provider calls.

## Act Path

`request_act` is the only System-callable act entrypoint. It creates an Inbox
approval request and stores a pending record with:

- requesting principal and session
- target id and operation
- provider request body
- original gate preview
- canonical request binding hash
- pending status

The Inbox notification is generated from that stored pending record and includes
a concise gate-preview summary: capability resource, actions, audit events, and
request hash. Approval therefore stays tied to the same reflected authority that
System previewed before creating the request.

Approval happens through Inbox, not through System. A System launch token can
create the action request but cannot call the Inbox action endpoint to approve
it. Inbox approval must also include a fresh same-principal passkey Home token,
matching the Wallet signing approval boundary. On approval, Runtime:

1. Loads the pending Inspector action record.
2. Verifies the fresh passkey Home token belongs to the same principal.
3. Confirms the approver is the same principal.
4. Recomputes the request binding and rejects tampering.
5. Recomputes the authority plan and rejects stale authority.
6. Calls `dispatch_approved` on the internal Inspect provider.
7. Dispatches to the target provider through `ProviderRegistry`.
8. Stores completed or failed status and appends signed audit.

Successful approvals persist the target provider's typed result in the
Inspector action record and append an `inspect.action.completed` audit event.
Failed approved dispatches persist the provider error and append
`inspect.action.failed`.

Denied requests are marked denied and never dispatch. Duplicate requests remain
distinct because request ids include a nonce.

`dispatch_approved` is intentionally not exposed through
`/api/provider/inspect/*`. The gateway allowlist exposes only:

- `capsules`
- `capsule`
- `self`
- `plan`
- `request_act`

Any attempt to predeclare Runtime metadata such as `_runtime_invocation`,
`_runtime_transfer`, `connect_ticket`, `carrier_route`, or `carrier` is rejected
before an Inbox request is created.

## Security Invariants

- Preview never mutates.
- Approved dispatch requires Inbox approval.
- Approval revalidates both request body and authority plan.
- Action records are principal-bound.
- Dispatch uses Runtime provider invocation, not app-supplied routes.
- Hidden Runtime metadata is stripped or rejected before dispatch.
- Failed dispatch is audited as a failed approved action.
- Direct revoke remains unsupported.
- System UI can request approval but cannot directly dispatch.

## Review Hooks

The focused source tests are in
[gateway_tests/inspect.rs](../elastos/crates/elastos-server/src/api/gateway_tests/inspect.rs)
and cover:

- approval before dispatch
- denial without dispatch
- dispatch failure audit
- runtime metadata rejection before Inbox
- raw Carrier route metadata rejection before Inbox
- stale authority plan rejection
- changed request binding rejection
- duplicate pending records

The broad alignment sentinel is in
[home-entropy-check.mjs](../scripts/home-entropy-check.mjs).
