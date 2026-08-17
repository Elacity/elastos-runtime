# ADR 0001 — Extract app and service logic from the trusted core

- Status: **Proposed**
- Date: 2026-06-14
- Deciders: ElastOS runtime maintainers
- Related: [PRINCIPLES.md](../../PRINCIPLES.md) §5, §13; [PRINCIPLES_CONFORMANCE.md](../PRINCIPLES_CONFORMANCE.md) item A; [TASKS.md](../../TASKS.md) (oversized-file splits)

## Context

Principle 5 says the runtime "should stay small enough to reason about": the trusted
core does isolation, signatures, principal/session binding, capability validation,
object routing, and audit — and *app logic belongs in capsules, service logic in
providers.* Today the trusted core (`elastos/crates/elastos-server`) holds full app and
service implementations whose capsules already exist:

| Core file | Lines | What it holds | Where it belongs |
|---|---:|---|---|
| `src/content.rs` | 13,062 | fetch, availability receipts, **federated abuse-control exchange**, **quota ledger**, **storage-market admission**, **external repair fleet**, operator dashboard | `capsules/content-market`, `capsules/availability-provider`, `capsules/ipfs-provider` |
| `src/library.rs` | 6,913 | Library app CRUD / archive / trash | `capsules/library` |
| `src/room_service.rs` | 5,441 | chat-room service: members, invites, key-epochs, attachments, presence, sessions | `capsules/chat-room`, `capsules/chat-room-ui` |
| `src/documents.rs` | 1,605 | Documents app CRUD | `capsules/documents` |

Additionally, `elastos/crates/elastos-runtime/src/provider/registry.rs:448`
(`RESERVED_SUB_NAMES`) hardcodes a closed allowlist of specific app/service names
(`wallet`, `drm`, `library`, `media`, `browser-engine`, …) inside the trusted core, so
every new provider requires editing the core to register its taxonomy.

This is **Principle-5 erosion**: the trusted base is accreting the exact app/service
logic the capsule/provider model exists to hold elsewhere. The risk is not stylistic —
it is the "core grows past what anyone can hold in their head, then the core *is* the
un-auditable thing" failure mode the small-trusted-core rule is meant to prevent. The
4 files above total ~27,000 lines of app/service logic living below the capability
boundary instead of above it.

`TASKS.md` currently tracks `room_service.rs` and friends as *oversized-file splits*
(no-behavior module moves). That addresses the **symptom** (file size) but not the
**architectural fact**: the logic is on the wrong side of the trust boundary. A module
split inside `elastos-server` leaves it just as much in the trusted core.

## Decision

Move app and service logic out of `elastos-server` into the owning capsules, behind the
**existing capability contracts**, so the trusted core retains only: capability/route
registration, principal/session binding, capability validation, object routing, and
audit. Concretely:

1. Each block of app/service logic moves to its already-existing capsule/provider and is
   reached through a Carrier-style capability call, not an in-core function call.
2. The trusted core keeps the *contract* (the typed provider operation and its capability
   gate) and the *routing*, not the *implementation*.
3. `RESERVED_SUB_NAMES` is replaced by **manifest/capability-declared registration**: a
   provider declares its namespace and operations in `capsule.json`; the core validates
   the declaration instead of hardcoding the taxonomy.

A change is "done" for a slice when the moved logic runs in its capsule, the core holds
only the contract, behavior is unchanged, and the slice passes `just verify` plus the
relevant provider/UI smokes.

## Options considered

1. **Status quo + file-size splits** (current TASKS.md framing). Rejected: relabels the
   symptom; the logic stays in the trusted core, so Principle 5 is still violated and the
   core stays un-auditable.
2. **Move logic to capsules behind capability contracts, incrementally** (this ADR).
   Chosen: it shrinks the trusted core for real, preserves behavior per slice, and is
   independently verifiable.
3. **Big-bang rewrite** of all four files at once. Rejected: unbounded blast radius, and
   it would collide head-on with active in-flight work in `elastos-server` (DDRM
   creator/viewer). Risk vastly exceeds the incremental path's.

## Migration approach (incremental, one verifiable slice at a time)

- **Phase 0 — Freeze. ✅ Enforced 2026-06-14.** `scripts/check-wci-alignment.sh` now caps
  `content.rs` (13200), `room_service.rs` (5550), `documents.rs` (1700), and `library.rs` (7300,
  extra headroom for in-flight dDRM). Each file may only *shrink*; the ceilings ratchet DOWN as
  phases land, never up. This stops the boundary eroding while the move is planned. (The
  alignment gate itself is now reliable — see PRINCIPLES_CONFORMANCE.md.)
- **Phase 1 — `content.rs`.** Split its 8 concerns: availability receipts → availability
  provider; federated abuse-control, quota ledger, storage-market admission, repair fleet →
  `content-market` (or a new typed service provider where no home exists); operator
  dashboard → a read-only diagnostics route. Core keeps the content/availability provider
  contract + routing.
- **Phase 2 — `room_service.rs`.** Move chat-room service into `chat-room` /
  `chat-room-ui`. Core keeps room launch/authority routing only.
- **Phase 3 — `library.rs` + `documents.rs`.** Move app CRUD into `library` / `documents`
  capsules. Core keeps the documents/library provider contract.
- **Phase 4 — `RESERVED_SUB_NAMES`.** Replace the hardcoded allowlist with
  manifest-declared registration validated by the core.

Each phase is one or more coherent, no-behavior commits, behind the existing capability
contract, each gated by `just verify` and the named smokes for that surface.

## Consequences

**Positive**
- The trusted core shrinks toward something a person can actually audit (Principle 5).
- The blast radius of any future core change drops sharply; app bugs can no longer reach
  trusted-core privilege.
- The capsule/provider boundary becomes *real* rather than nominal (Principle 13).
- Docs/code/tests converge on one contract (Principle 12); the conformance register's
  biggest item closes.

**Costs / risks**
- Substantial, careful churn across ~27k lines; must be staged, not rushed.
- Cross-boundary calls now traverse Carrier/capability envelopes. That is the intended
  contract, but it adds latency on hot paths — measure availability/library read paths and
  keep batching where needed.
- Regression risk during each move — mitigated by per-slice `just verify` + provider/UI
  smokes and no-behavior commits.
- **Sequencing vs. active WIP:** the in-flight DDRM creator/viewer work edits
  `elastos-server` (`creator.rs`, `viewer_open.rs`, …). Do **not** begin Phase 1+ until
  that work lands, to avoid a merge collision on the very files being moved.

## Open questions

- Which exact `content.rs` concerns map to an *existing* provider vs. need a **new** typed
  service provider (federated abuse-control and the quota ledger have no obvious home).
- The shape of the capability contract for availability receipts and storage-market
  admission once they live outside the core.
- Whether the operator dashboard becomes a thin read-only route over provider state or a
  small dedicated operator surface.
