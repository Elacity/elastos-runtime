# Capsule Inspector

> An object-centered, read-only introspection surface for ElastOS.
> Self's *mirrors* and *Morphic*, wrapped in our zero-ambient-authority model.

## Why

A computer should let you open up the living thing you're using and understand
exactly what it *is*, what it *can do*, what it's *allowed* to do, and where it
*came from* — without fear, and without having to trust a claim you can't see.

ElastOS already enforces a strong security model (no ambient authority,
capability-scoped provider calls, signed audit). But today that security is
**invisible**: a user installs a capsule and has no surface to see that it
*cannot* touch their keys, or what it has actually been doing. The Inspector
makes the existing guarantees **visible and demonstrable**.

This is also the human-facing answer to a gap `state.md` already records:

> System ... does not yet expose the fuller `elastos://` provider/object/
> capability discovery contract.

## How (the borrowed idea, re-secured)

Self is a live-object world: every running thing is an object you can inspect
and poke through *mirrors*, arranged in a direct-manipulation desktop
(*Morphic*). Self trusts everyone in one process. **We trust no one and prove
it.** So we keep Self's object-world UX and re-implement reflection as a
**permissioned mirror**:

| Self idea            | ElastOS realization                                  |
| -------------------- | ---------------------------------------------------- |
| Live object          | Capsule (signed, stateful, running)                  |
| Slots / messages     | Affordances (`interfaces[].methods` in capsule.json) |
| Mirror (reflection)  | Capability-gated, audited read-only inspect view     |
| Morphic world        | The Inspector UI (PC2 surface)                        |
| Transporter (export) | Signed capsule packaging (already exists)            |

Everything the Inspector shows is data the **trusted core already owns**
(manifests, capability grants, audit log, running instances, provenance). The
Inspector is therefore a *read-only projection* — not a new architecture.

## What

A read-only view, one screen per capsule, of nine fields:

1. **identity / DID** — capsule id, device/account DID where relevant
2. **manifest** — schema, role, type, entrypoint, version
3. **affordances** — declared interface methods + risk / approval / audit class
4. **required capabilities** — what the manifest asks for
5. **granted capabilities** — what was actually granted (and what was *denied*)
6. **storage namespaces** — `localhost://WebSpaces/...` scopes
7. **carrier endpoints** — gossip/peer endpoints, peer count
8. **provenance** — signature, signer DID, version, install time, CID
9. **logs / audit + running processes** — recent audited calls, live instances

## Security invariants (non-negotiable)

- **Read-only.** The inspect surface exposes *no* write/sign/launch effect.
- **No new trust.** The Inspector capsule holds zero ambient authority and
  itself appears in the Inspector, subject to the same rules it reveals.
- **Permissioned mirror.** Every inspect call is capability-gated
  (`elastos://inspect/read`) and audited like any other provider call.
- **Scope-bound.** A principal can only inspect capsules within its own
  session / grant scope. Out-of-scope inspection is denied *and* audited.

## Phasing

| Phase | What                                                         | Core impact            |
| ----- | ------------------------------------------------------------ | ---------------------- |
| **1** | Read-only Inspector UI + `elastos://inspect/*` contract      | None (additive)        |
| 2     | Invoke an affordance / revoke a grant from the UI            | Light (wire existing)  |
| 3     | Morphic-style direct manipulation: drag, re-wire providers   | Composition layer      |
| 4     | Transparent stored-vs-computed affordance backing (the Self  | Deeper ABI work        |
|       | "you can't tell if it's stored or computed" dream)           |                        |

Phases 1–2 deliver ~90% of the "see my system as living objects" experience for
~10% of the cost, with no rewrite. Phases 3–4 are where rewrite risk lives —
deliberately deferred.

## Phase 1 in this branch

This branch contains the Phase-1 starter:

- `capsules/capsule-inspector/` — a WASM **app** capsule (UI) that renders the
  nine-field view. It requires only `elastos://inspect/read`. Ships with sample
  data so the UI renders standalone; calls the live provider when present.
- This doc — the architecture, contract, and phasing.

The remaining Phase-1 piece is the runtime-side read-only handler that backs
`elastos://inspect/*` from `CapsuleManager` + `AuditLog`. It is additive and
read-only; see the contract below.

## Wire contract: `elastos://inspect/*` (read-only)

All operations are `read`. Responses are JSON.

### `elastos://inspect/capsules` — list

```json
{
  "capsules": [
    { "id": "...", "name": "chat-room", "role": "shell", "type": "wasm", "state": "running" }
  ]
}
```

### `elastos://inspect/capsule` (body: `{ "id": "..." }`) — detail

```json
{
  "id": "cap_chat_room_01",
  "name": "chat-room",
  "version": "0.1.0",
  "role": "shell",
  "type": "wasm",
  "description": "Peer-to-peer chat room",
  "author": "elastos",
  "identity": {
    "did": "did:key:z6Mk...",
    "cid": "bafy...",
    "trust_level": "verified",
    "signature_present": true,
    "signed_by": "gateway-did"
  },
  "manifest": { "schema": "elastos.capsule/v1", "entrypoint": "chat.wasm" },
  "affordances": [
    { "interface": "elastos.chat/v1", "id": "send", "risk": "write",
      "approval": "user", "audit": "event", "description": "Send a message" },
    { "interface": "elastos.chat/v1", "id": "history", "risk": "read",
      "approval": "none", "audit": "summary", "description": "Read history" }
  ],
  "required_capabilities": ["elastos://carrier/*", "elastos://storage/chat"],
  "granted_capabilities": [
    { "resource": "elastos://carrier/*", "action": "message", "granted": true,
      "token_id": "tok_...", "expiry": 1781990400 },
    { "resource": "elastos://did/*", "action": "read", "granted": false }
  ],
  "storage_namespaces": ["localhost://WebSpaces/chat-room/"],
  "carrier": { "enabled": true, "endpoints": ["gossip://..."], "peers": 1 },
  "provenance": { "signed_by": "gateway-did", "version": "0.1.0",
                  "installed_at": 1781817600, "cid": "bafy..." },
  "audit": {
    "counts": { "total_today": 14, "user_approved": 2, "denied": 1 },
    "recent": [
      { "ts": 1781990100, "event": "capability.use", "detail": "carrier/* message", "success": true },
      { "ts": 1781990050, "event": "capability.denied", "detail": "did/* read", "success": false }
    ]
  },
  "processes": [
    { "kind": "wasm", "instance": "#4", "memory_mb": 12, "uptime_s": 10800 }
  ]
}
```

The runtime-side handler maps these fields from existing sources:
`CapsuleManager::list()` / `get()` (id, manifest, state, cid, trust_level),
`CapabilityManager` (granted/denied tokens), and `AuditLog::memory_buffer`
(recent events + counts). No new state is introduced.
