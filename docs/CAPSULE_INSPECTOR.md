# Capsule Inspector

Capsule Inspector is Runtime's permissioned, metadata-only view of capsules and
providers. It exposes facts and action previews without giving the viewer
provider authority.

## Scope

- `elastos://inspect/*` is the System-wide view.
- `elastos://inspect/self` defines the pure SelfOnly scope rule.
- Self scope is a live, fail-closed route. `/api/provider/inspect/self` is
  served to the app/browser tier (`"self" => &[BROWSER_CAPSULE_ID]` in
  `gateway_provider_proxy.rs`), not the System tier. It is caller-bound: the
  gateway injects the authenticated `principal_id`,
  any client-supplied `id` is ignored, and the provider routes through
  `inspect::authorize_view` under
  `InspectScope::SelfOnly`, so a capsule reads only its own record.
- Every other inspect op (`capsules`, `capsule`, `plan`, `intent`, `discover`,
  `request_act`) stays pinned to the System tier.
- System can read facts, preview an operation, and request approval.
- System cannot approve, dispatch, or revoke an Inspector action.

## Projection

Inspector may return:

- capsule or provider identity, kind, and state
- a redacted manifest slice
- declared capabilities, actions, operations, and audit events
- CID, signature presence, and a signature fingerprint
- process, storage, and Carrier summaries

Inspector reports manifest signature evidence as declared but unverified:
`verified=false`, while `verified_by` and `signed_by` are `null`. Granted
capabilities, audit summary, spend budget, intent proof, and audit-chain
attestation are not projected today and return `null`.

Projection recursively removes known secret-bearing fields and strings,
including Runtime-private metadata, Carrier tickets and routes, IPC and control
socket paths, absolute host paths, raw signatures and tokens, private keys, and
mutation handles. Declared provider capabilities remain visible after this
redaction; visibility does not grant authority.

## Preview and approval

`plan` runs the same authority planner used by Runtime provider invocation. It
returns the required resources, actions, audit events, and these fixed
properties:

```text
mode=preview_only
can_dispatch=false
can_mutate=false
dispatch=false
```

`request_act` stores the principal, session, target, operation, request body,
preview, and canonical request-binding hash in a pending action record. Runtime
builds the Inbox notification from that record.
The notification includes a concise gate-preview summary.

Approval happens in Inbox. A System launch token can create a request.
It cannot call the Inbox action endpoint.
Inbox requires a fresh same-principal passkey Home token. On approval, Runtime:

1. Loads the pending record.
2. Verifies the passkey token and principal.
3. Recomputes the request binding.
4. Recomputes the authority plan.
5. Dispatches through the internal Inspect provider and `ProviderRegistry`.
6. Stores the result or error and appends signed audit.

A changed request or authority plan fails stale. Denial dispatches nothing.
Each request has a nonce, so two otherwise identical requests remain distinct.

The public Inspector gateway allows only `capsules`, `capsule`, `self`, `plan`,
and `request_act`. `dispatch_approved` is internal. `revoke` is not implemented.
Requests containing `_runtime_invocation`, `_runtime_transfer`,
`connect_ticket`, `carrier_route`, or `carrier` are rejected before Inbox.

## Source and proof

- Pure scope rules:
  [inspect/mod.rs](../elastos/crates/elastos-runtime/src/inspect/mod.rs)
- Projection, planning, and dispatch:
  [inspect_provider](../elastos/crates/elastos-server/src/inspect_provider/)
- Inbox action flow:
  [gateway_inspect_actions.rs](../elastos/crates/elastos-server/src/api/gateway_inspect_actions.rs)
- Local proof:
  [Inspector testing](INSPECTOR_TESTING.md)
