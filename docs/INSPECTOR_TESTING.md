# Inspector Testing

Use this page for a quick local check that the Capsule Inspector remains a
permissioned mirror with Inbox-gated action dispatch.

## Focused Source Tests

Run the focused Inspector act proof:

```bash
scripts/capsule-inspector-act-check.sh
```

It runs the Inspector action tests from the Rust workspace:

```bash
(cd elastos && cargo test -p elastos-server inspect_action -- --nocapture)
```

Expected result:

- `inspect_action_requires_inbox_approval_before_dispatch` passes
- `inspect_action_can_be_denied_without_dispatch` passes
- stale plan and changed request-binding tests pass
- no provider dispatch happens before Inbox approval

Run the pure inspect-scope tests:

```bash
(cd elastos && cargo test -p elastos-runtime inspect -- --nocapture)
```

Expected result:

- no grant fails closed
- pure SelfOnly can view only self
- System scope can view all

Current product routing still keeps `/api/provider/inspect/self` System-only.
Do not treat the pure SelfOnly test as proof that ordinary capsules have a live
caller-bound Inspector route.

## Alignment Gate

Run the broad Home alignment sentinel:

```bash
scripts/capsule-inspector-act-check.sh
```

When `node` is not on `PATH` in the Codex Mac environment, use the bundled
runtime directly:

```bash
node scripts/home-entropy-check.mjs
```

The sentinel checks that:

- System can call `capsules`, `capsule`, `self`, `plan`, and `request_act`
- System cannot call `dispatch_approved`
- System cannot call `revoke`
- previews are non-mutating
- request approval is routed through Inbox
- redacted provenance stays redacted

## Manual UI Loop

With Home running:

1. Open Home.
2. Unlock with a passkey.
3. Open System.
4. Open Inspector.
5. Select a provider capsule such as `exit-provider`.
6. Select a declared operation such as `status`.
7. Confirm the gate preview shows resources/actions/audit and `dispatch=false`.
8. Click `Request approval`.
9. Open Inbox.
10. Approve or deny the Inspector action request.

Approval should dispatch exactly once. Denial should dispatch zero times.
Refreshing Inbox after approval or denial should not show the same pending
request.

## Failure Cases To Preserve

These are intentional fail-closed behaviors:

- `dispatch_approved` through `/api/provider/inspect/dispatch_approved` returns
  not found.
- `revoke` through `/api/provider/inspect/revoke` returns not found.
- request bodies with `_runtime_invocation`, `_runtime_transfer`,
  `connect_ticket`, `carrier_route`, or `carrier` are rejected before Inbox.
- if authority metadata changes after request creation, approval fails stale.
- if the stored request body changes after request creation, approval fails
  stale.

## Required Commit Gate

For Inspector changes, run at minimum:

```bash
scripts/capsule-inspector-act-check.sh
git diff --check
(cd elastos && cargo fmt --all -- --check)
```
