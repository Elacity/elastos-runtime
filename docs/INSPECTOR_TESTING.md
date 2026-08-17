# Inspector testing

[Capsule Inspector](CAPSULE_INSPECTOR.md) owns the contract. This page owns the
shortest local proof.

## Automatic proof

From the repository root:

```bash
scripts/capsule-inspector-act-check.sh
```

The wrapper runs the Inspector server tests, Runtime scope tests, and the Home
alignment check. Set `ELASTOS_NODE_BIN` to choose a Node.js binary if `node` is
not on `PATH`.

The scope tests prove that `NoGrant` fails closed, `System` can view all, and
pure SelfOnly can view only self.

`/api/provider/inspect/self` is a live, caller-bound app/browser-tier route
(`"self" => &[BROWSER_CAPSULE_ID]`): fail-closed SelfOnly — the gateway injects
the authenticated `principal_id`, a client-supplied `id` is ignored, and
`authorize_view` enforces caller == target. The other inspect ops
(`capsules`, `capsule`, `plan`, `intent`, `discover`, `request_act`) remain
System-only.

## Manual approval loop

With Home running:

1. Unlock Home with a passkey.
2. Open System and expand Technical Details, then Inspection.
3. Select `exit-provider` and the `status` operation.
4. Select Preview. Confirm that the operation and required permission appear,
   then request approval.
5. Open Inbox and approve the request.
6. Repeat with a new request and deny it.

Approval requires a new passkey check and removes the completed request from
Inbox. Denial removes the request without a passkey prompt. Refreshing Inbox
must not restore either request to the pending list.

The automatic proof fails if System can call `dispatch_approved` or `revoke`,
if preview mutates state, if hidden Runtime or Carrier fields reach Inbox, or
if approval accepts a changed request or stale authority plan. It also verifies
that approval dispatches once and denial does not dispatch.

## Handoff

```bash
git diff --check
(cd elastos && cargo fmt --all -- --check)
```
