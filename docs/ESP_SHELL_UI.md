# ESP Shell UI (W5b) — the visual projection shell

**Status:** spec (in-cloud). The component *contracts* are defined here; the live
Svelte paint lands in the browser/local lane. Every contract below maps directly
onto a proven, tested ESP projection function (W5a/W6/W7) — the UI adds **no new
logic**, it only renders what the headless projection already computed.

## The one invariant (non-negotiable — Bret Victor's law)

> **Every pixel is a read-only projection of signed runtime state. No component
> holds a key, a token, or any authority.**

Concretely, for every component in this spec:

1. **Props in = ESP fact types only** (the serde-mirrored types in `esp/esp_v0.ts`),
   each carrying a `schema` tag from `ESP_SCHEMA_TAGS`. A component must IGNORE
   unknown fact fields (forward-compat) and must NOT reconstruct authority from
   them.
2. **Pixels out** — the component renders; it computes nothing security-relevant
   that the headless layer (`two_channel.ts`, `consent_act.ts`, `shell_picker.ts`,
   `ai_act_audit.ts`) hasn't already decided. If a badge says "verified," it is
   because `trustMaterial()` returned `"verified"`, not because the view guessed.
3. **Actions out = INTENTS, never authority.** A click emits an event (`approve`,
   `deny`, `select-shell`, `invoke`) that travels back over the ESP protocol to
   the runtime. The runtime is the ONLY minter of tokens/receipts. The view can
   never grant itself anything; worst case a compromised view can *ask*, and the
   consent broker still gates.
4. **Fail-honest rendering.** Unknown/missing trust → render as `unsigned`
   (never blank, never optimistic). Incomplete reach → render the hazard as
   `incomplete`, not `cool`. This mirrors the headless fail-closed defaults.

This is the moat made visible: the buyer can diff any pixel against the signed
fact behind it.

## Component contracts

Each component lists: **props** (the ESP fact type + its schema tag), the
**headless function** it projects (the single source of truth), what it
**renders**, and the **intent** it may emit.

### `<CapsuleCard>` — the trust badge
- **Props:** `CapsuleSummary` (`elastos.capsules.catalog/v1`).
- **Projects:** `trustMaterial(capsule)` → `"verified" | "content_addressed" | "unsigned"`.
- **Renders:** name + a trust chip whose colour/label is a pure function of the
  `TrustMaterial` value (verified = signed-and-checked; content_addressed = CID
  pinned but unsigned; unsigned = neither). Unknown `trust_state` → `unsigned`.
- **Intent:** `open(capsule.name)` (navigates; mints nothing).

### `<TwoChannelObject>` — the hero ("never-seen moment")
- **Props:** `TrustMaterial` + `AffordanceReachView` (the catalog's per-affordance
  reach), or directly a `TwoChannelObject` from `twoChannel(trust, view)`.
- **Projects:** `twoChannel(trust, view)`. The load-bearing field is
  `refuseTrained = trust === "verified" && blast.level === "hot"` — the object the
  user has been *trained by every other OS to trust* (a signed, verified app) is
  shown as the one to scrutinise *because* its blast radius is hot. Trust and
  reach are **two independent channels**, never collapsed into one "safe/unsafe."
- **Renders:** two side-by-side channels — TRUST (the badge) and REACH (the
  hazard meter, below) — plus the `refuseTrained` call-out when set.
- **Intent:** none (pure display); it frames the `<ConsentSheet>`.

### `<HazardMeter>` — blast radius
- **Props:** `ReachDescriptorV1` (`elastos.reach.v1`).
- **Projects:** `blastRadius(reach)` → `{ level: "cool" | "warm" | "hot", reasons, incomplete }`.
- **Renders:** a 3-stop meter. Open egress drives `hot`; allowlisted egress is
  strictly cooler than open; `!observed` reach renders `incomplete` (honest
  "we haven't watched this run yet"), never a confident `cool`.
- **Intent:** none.

### `<ConsentSheet>` — the hero consent act
- **Props:** `AffordanceConsentPending` (`elastos.capsules.affordance-consent-pending/v1`).
- **Projects:** `consentPendingIsWellFormed(pending)` — the sheet renders the act
  ONLY if the pending fact is well-formed; otherwise it shows an error, never a
  blank approve button.
- **Renders:** "⟨capsule⟩ wants to ⟨method⟩ ⟨resource⟩" with the two-channel
  object above it, the risk class, and the approval mode. Approve/Deny buttons.
- **Intent:** `approve(request_id)` / `deny(request_id)` — sent to the consent
  broker. The view never mints the token; it only relays the human decision. The
  runtime's `validate-and-consume` is what actually issues authority.

### `<ReceiptBadge>` — proof the act happened in the user's name
- **Props:** `AffordanceGrantReceiptV1` (`elastos.affordance.receipt.v1`) + the
  originating request.
- **Projects:** `receiptMatchesRequest(receipt, request)` — the badge is "valid"
  only when the signed receipt attests the EXACT requested `(capsule, method,
  input_hash, resource, action)`. A mismatch renders as a warning, not a tick.
- **Renders:** a signed-receipt chip (signer fingerprint, redeemed-at) — never the
  raw signature bytes or token (Principle 16: no authority/secret in the view).
- **Intent:** none.

### `<ShellPicker>` — choose the active shell (W6)
- **Props:** `CapsuleCatalogResponse` (`elastos.capsules.catalog/v1`) + optional
  active shell name.
- **Projects:** `shellPicker(catalog, active)` → `{ shells, active }`, built from
  `selectableShells()` (role === "shell" && launchable) and `shellTrustCard()`.
  Selecting routes through `withActiveShell(picker, name)` which returns `null`
  for a non-selectable name (the view shows the choice as rejected, fail-closed).
- **Renders:** a list of selectable shells, each a `<CapsuleCard>`-style trust
  card; the active one marked. A non-shell/non-launchable capsule is simply
  absent (not greyed — it was never a candidate).
- **Intent:** `select-shell(name)` — relayed to the supervisor's
  `set_active_shell`; the runtime re-issues the privileged shell token, not the view.

### `<RefractionToggle>` — focus flip without losing the projection (W6)
- **Props:** `RefractionState<T>` (generic over any projected ESP fact `T`).
- **Projects:** `toggleFocus(state)` — flips focus between the two faces (e.g.
  consumer view ⇄ auditor view of the SAME signed object) while preserving the
  projected payload via `{ ...state }`. The two faces are two refractions of one
  fact, never two different facts.
- **Renders:** the focused face; a toggle affordance.
- **Intent:** `toggle()` (local UI state only; no runtime call).

### `<AiActAuditCard>` — the enterprise containment surface (W7)
- **Props:** `AiActAuditRecordV1` (`elastos.audit.ai-act.v1`) + `ContainmentEvidence`.
- **Projects:** `toAiActAuditRecord(consent, receipt)` then
  `containmentEvidence(record)` → `{ article_12_met, article_14_met, contained }`.
- **Renders:** the SAME signed receipt the consumer saw, re-projected as the
  regulator/insurer view: Art 12 (record-keeping / signed) and Art 14 (human
  oversight) as met/unmet chips, with a `contained` verdict. Fail-closed: an
  unsigned record shows Art 12 unmet; a high-risk act with no human shows Art 14
  unmet. This is the flywheel made legible — consumer delight and compliance
  evidence are one object.
- **Intent:** `export-audit(record)` (download/relay the record; mints nothing).

### `<SpendBudgetMeter>` — the agent's act budget (adoption wedge #4)
- **Props:** `BudgetSnapshotV1 | null` (the inspector's `spend_budget` field, mirror
  of `primitives::spend::BudgetSnapshot`).
- **Projects:** `spendBudgetView(snapshot)` → `{ metered, limit, spent, remaining,
  fractionUsed, exhausted, state }`.
- **Renders:** a budget meter. Fail-honest: `null` ⇒ **unmetered** (the meter is
  hidden, NOT shown as a satisfied 0/0); a drained or hard-stop (limit 0) budget
  renders **exhausted**; `fractionUsed ≥ 0.8` renders a **warning**. The view never
  fabricates a budget and never edits the meter.
- **Intent:** none (pure display).

### `<AuditChainBadge>` — the flight recorder's live integrity
- **Props:** `ChainAttestation | null` (the inspector `audit_attestation` op /
  `audit.chain` field, mirror of `primitives::audit::ChainAttestation`).
- **Projects:** `auditChainView(chain)` → `{ present, verified, records, signer,
  error, state }`.
- **Renders:** a chain-integrity chip. Fail-honest: `null` ⇒ **absent** ("no durable
  chain to attest" — neither pass nor fail, mirroring a memory-only plane); a present
  clean walk ⇒ **verified** (records + signer); a present-but-unverified chain ⇒
  **broken** (a tamper warning surfacing the first break), never optimistically green.
- **Intent:** none (pure display).

### `<CapsuleCustodyPanel>` — the Home capsule-detail custody panel
- **Props:** a `HomeCustodyView` (from `homeCustodyView(spend_budget, audit.chain)`).
- **Projects:** nothing of its own — it is **pure paint** over the already-projected
  view. It maps each honest sub-state to a display label + a `data-state` attribute
  and nothing else.
- **Renders:** two channels — Spend (`unmetered` / `ok` / `warning` / `exhausted`)
  and Audit chain (`absent` / `verified` / `broken`). There is deliberately no
  "all-good" affordance keyed on anything but the honest sub-states, so an unmetered/
  exhausted budget or an absent/broken chain can never be masked by a green panel.
- **Intent:** none (pure display).
- **Harness:** server-side rendered (`svelte/server`) and snapshot-tested headlessly
  under `node:test` (`capsule_custody_panel.test.mjs`) — macOS-gateable, no browser.

The Home capsule-detail panel paints both facts together via `homeCustodyView(
spend_budget, audit.chain)` — a PURE composition (`{ spend, audit }`, no roll-up
verdict), so the panel can only be "green" when BOTH the spend meter and the audit
chain are themselves honestly green; an absent/broken chain or exhausted budget can
never be masked.

### `<CapsuleDetail>` — the Home capsule-detail surface (trust ∥ custody)
- **Props:** a `CapsuleDetailView` (from `capsuleDetailView(capsule, spend_budget,
  audit.chain)`), composing `trustMaterial` (Channel 1) + `homeCustodyView` (Channel 2).
- **Projects:** nothing of its own — **pure paint** over the composed view.
- **Renders:** the capsule header + three independent channels — Trust (`verified` /
  `content_addressed` / `unsigned`), Spend, and Audit chain. The two channels are
  INDEPENDENT (no blended "overall safe" affordance): a verified capsule still shows an
  exhausted budget / broken chain, and an unsigned capsule is never dressed up by a
  clean custody panel. Self-contained for the SSR snapshot harness.
- **Intent:** none (pure display).

## Composition

```
<ShellPicker>                         ← pick the consent surface
  └─ <CapsuleCard> …                  ← per shell, the trust badge

<ConsentSheet>                        ← the hero act
  ├─ <TwoChannelObject>               ← trust ∥ reach, refuseTrained
  │    └─ <HazardMeter>               ← blast radius
  └─ (approve/deny → consent broker)
        └─ <ReceiptBadge>             ← proof, on success
              └─ <RefractionToggle>   ← flip to the auditor face
                    └─ <AiActAuditCard>  ← Art 12 / Art 14 / contained
```

## Schema-tag pinning (so the ratchet can guard the UI later)

Every component is bound to the ESP schema tag of the fact it consumes (table
below). When the Svelte components land, `scripts/check-wci-alignment.sh` should
pin that each component imports its declared fact type and that the tag string
still exists in `esp/esp_v0.ts` — the same ratchet style used for the W4 ESP
routes. This keeps the "pixel ⇄ signed fact" mapping enforceable, not aspirational.

| Component | Fact type | Schema tag |
|-----------|-----------|------------|
| `<CapsuleCard>` | `CapsuleSummary` | `elastos.capsules.catalog/v1` |
| `<TwoChannelObject>` | `TwoChannelObject` (trust + `AffordanceReachView`) | `elastos.capsules.catalog/v1` |
| `<HazardMeter>` | `ReachDescriptorV1` | `elastos.reach.v1` |
| `<ConsentSheet>` | `AffordanceConsentPending` | `elastos.capsules.affordance-consent-pending/v1` |
| `<ReceiptBadge>` | `AffordanceGrantReceiptV1` | `elastos.affordance.receipt.v1` |
| `<ShellPicker>` | `CapsuleCatalogResponse` | `elastos.capsules.catalog/v1` |
| `<RefractionToggle>` | `RefractionState<T>` | (generic — the wrapped fact's tag) |
| `<AiActAuditCard>` | `AiActAuditRecordV1` | `elastos.audit.ai-act.v1` |
| `<SpendBudgetMeter>` | `BudgetSnapshotV1` | (inspector `spend_budget`; mirrors `primitives::spend::BudgetSnapshot`) |
| `<AuditChainBadge>` | `ChainAttestation` | (inspector `audit.chain`; mirrors `primitives::audit::ChainAttestation`) |

## What W5b implementation must NOT do

- No component may call a signing/minting API or hold a token — props are facts,
  events are intents (enforced by the type signatures: no component takes a
  `CapabilityToken` or key as a prop).
- No re-deriving trust/hazard/containment in the view — call the headless
  function; if a verdict is wrong, fix it once in `esp/*.ts` (tested) and every
  pixel follows.
- No optimistic UI on consent — the approve button does not pre-render success;
  the `<ReceiptBadge>` appears only when a real signed `AffordanceGrantReceiptV1`
  arrives.

## Lane

The contracts are in-cloud (this doc + the proven `esp/*.ts` they project). The
live Svelte components, a browser harness, and a visual snapshot test are the
browser/local lane — tracked as the remaining W5b implementation step in
`ROADMAP.md`.
