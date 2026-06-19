# KNOWN_GAPS — Capsule Inspector

Build-visible registry of what the Inspector does **not** yet assert, so gaps are
impossible to forget and "we should fix this" becomes a tracked, enforceable
contract instead of prose. (Pattern from `LESSONS.md`: *turn an audit into a
build-visible gap registry, not a doc that rots*.)

Each **open** gap below has a `#[ignore]`d **ratchet test** that encodes the
desired end-state and **fails today** (hence ignored — non-blocking in a shared
tree). Closing a gap = wire the feature, delete the `#[ignore]`, the test goes
green, and the row moves to "Closed."

## Open gaps

| # | Gap | Why open | Ratchet test (`#[ignore]`d) | Close criteria |
|---|-----|----------|------------------------------|----------------|
| G1 | `granted_capabilities` is always empty | ElastOS caps are bearer tokens with no central per-capsule registry, and `RuntimeAuditEventV1` carries no resource/action — the observed-grant list can't be derived without fabrication. | `inspect_provider::tests::ratchet_granted_capabilities_populated` | A capability-event source records resource+action; projection lists observed grants; test asserts non-empty and goes green. |
| G2 | Provenance `signed_by` (verified signer) is always null | The manifest schema carries no signer DID/pubkey, so we surface presence + fingerprint + trust level but never a *verified* signer (we refuse to present the declared author as verified). | `inspect_provider::tests::ratchet_provenance_verified_signer_present` | A signature-verification source resolves the signer DID; projection fills `signed_by`; test asserts non-null. |
| G3 | Invoke **dispatch** (the "act" half) | Preview-only by design. Dispatch must consult DDRM's `required_action_for` so preview and enforcement agree by construction — best built on the merged base. **Merge-gated.** | *Pending feature scaffold* (no compiling test until the dispatch API exists — a fabricated one would be vacuous). | Dispatch lands on the unified base; a carrier e2e test proves a gated call enforces exactly the previewed gate. |
| G4 | Human-**approval** loop — *recording* | **Decision core DONE** (`elastos-runtime::approval` + `inspect/intent` preview, fail-closed, tested). Remaining: *recording* a signed approve/deny — a mutation that pairs with dispatch (G3) on the write path. | n/a — decision core is enforced by real tests (`approval::tests::*`, `inspect_provider::tests::intent_*`). | Recorded approve/deny decision exists on the runtime/dispatch path, audited. |

## Enforced invariants (the inverse — already guaranteed, not gaps)

These are *closed by construction* and worth recording so they aren't mistaken
for open work:

- **Merge gate-contract tripwire** — `carrier_bridge::tests::carrier_inspect_ops_match_canonical_action_contract` drives a real carrier call per inspect op at its canonical action; goes red the moment DDRM's `required_action_for` would fail-close an inspect op. Enforced now.
- **No secret/handle leakage (#16)** — `inspect_provider::tests::capsule_detail_renders_contract_without_leaking_authority` proves the raw signature / token never appear in output.
- **Two-tier scope, fail-closed (#11)** — `elastos-runtime::inspect` unit tests + `tests/inspect_conformance.rs` prove a SelfOnly caller cannot read another capsule.

## How to use this file
- A new gap → add a row + an `#[ignore]`d ratchet test (or note "pending scaffold" if no API exists yet).
- Closing a gap → wire it, delete the `#[ignore]`, confirm the test is green, move the row to a "Closed" section (or delete it — this is memory, not an archive).
- A gap proven safe-by-construction → move it to "Enforced invariants" (confirming-safe is as important as finding-bad).
