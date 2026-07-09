# Market Provider Contract — v1 (Sprint 48)

The contract a payment/market vertical implements to settle `runtime.pay` acts under a Flint
mandate. Two shipped verticals implement it — the DRM marketplace (`DrmMarketplaceProvider`) and
the ERC-20 checkout (`Erc20CheckoutProvider`) — plus the generic HTTP rail
(`HttpPaymentProvider`). A third party should be able to implement a provider from this document
alone; where the runtime enforces an obligation mechanically (a type, a required trait method, a
gate ratchet), that enforcement is named.

## 1. The seam

```rust
pub trait PaymentProvider: Send + Sync {
    fn pay(&self, payee: &str, amount: u64, idempotency_key: &str) -> Result<String, PayError>;
    fn rail(&self) -> PaymentRail;   // REQUIRED — no default (compiler-enforced, S44/S46)
}
```

The runtime — never the provider — owns: the mandate gate (signed intent, scope, rate budget,
liability DID), the spend meter (cap reservation before `pay`, refund on `NotCharged`), the
payment ledger (record-BEFORE-broadcast custody, resolve-exactly-once), the signed receipt chain,
and reconciliation. A provider moves value on its rail and reports honestly. That split is the
contract's core: **a provider cannot be trusted with a money decision, only with a money report.**

## 2. The two-generals report (`pay`)

- `Ok(rail_ref)` — the charge PROVABLY completed on the rail. Chain-settled rails NEVER return
  this at broadcast (see §4).
- `Err(NotCharged(why))` — the charge PROVABLY did not happen. The runtime refunds the
  reservation. Return this ONLY for failures you can prove occurred strictly before value moved.
- `Err(Indeterminate(why))` — anything you cannot prove either way. The runtime HOLDS the
  reservation for reconciliation. **When unsure, always this — never guess NotCharged.**

CLASSIFY BY CONSTRUCTION, NOT BY MESSAGE (the S43 rule): the variant must be decided by which
code path produced the failure (its position relative to the value-moving operation), never by
inspecting error text. In the shipped verticals every pre-broadcast leg is `.map_err(NotCharged)`
at its call site and every broadcast-or-after leg is `.map_err(Indeterminate)` — no
provider-controlled byte can flip the money direction.

`idempotency_key` is unique per signed intent (signature-derived). Rails with a dedupe facility
MUST use it so a retry/reconciliation can never double-move value.

## 3. The rail discriminator (`rail`)

Every provider declares its `PaymentRail` variant; the runtime stamps it onto the ledger record
at `begin_attempt`. Rail-specific reconcilers select records by this STRUCTURED tag — never by
parsing the (rail-controlled) `rail_note`. `rail()` has no default implementation, so the
compiler forces every new provider to make this declaration (S46). Consequences:

- A hostile endpoint on one rail cannot craft a note that gets its pending polled by another
  rail's reconciler (gate-ratcheted for both `drm:tx=` and `erc20:tx=` forgeries on Http-tagged
  records).
- Adding a `PaymentRail` variant is a forward-incompatible ledger change (an older runtime
  refuses a newer snapshot — deliberate fail-closed serde posture); ship it as a coordinated
  upgrade.

## 4. Chain-settled rails: broadcast ≠ charged

A rail that settles on a chain (DRM, ERC-20) MUST NOT report `Ok` at broadcast. The contract:

1. `pay` broadcasts, then returns `Err(Indeterminate(rail_ref))` where `rail_ref` is
   `<rail>:tx=<hash>;<k>=<v>;…` — compact, delimiter-stripped per component (`;`/`=` removed
   from each chain-supplied value so a hostile field cannot forge the parsed binding).
2. The runtime holds the reservation as a `Pending` ledger record carrying that note + the rail
   tag.
3. The provider also implements the confirmation reader (`DrmConfirmer::confirm`, shared
   `confirm_chain_tx` spine): tx mined + success status + ≥ `ELASTOS_DRM_MIN_CONFIRMATIONS`
   deep ⇒ `Confirmed`; mined-but-reverted ⇒ `Reverted`; anything else — including any read
   error — ⇒ `Unconfirmed` (hold; NEVER auto-charge a tx you could not verify).
4. The in-runtime scheduler (or a manual reconcile) promotes `Confirmed` pendings to charged
   exactly once (binding the receipt's `rail_ref`), refunds `Reverted` exactly once, and leaves
   `Unconfirmed` pending. Depth gates BOTH verdicts (a shallow revert is held, not refunded — a
   reorg could re-include it).

## 5. The quote gate: the cap is a literal ceiling

The mandate cap is denominated in meter units; the rail settles in its own units. The operator
DECLARES the mapping at wiring time (`ELASTOS_DRM_SPEND_UNIT` / `ELASTOS_ERC20_SPEND_UNIT`:
rail base-units per meter unit) and the rail REFUSES TO WIRE without it — never a silent 1:1.
A rail with variable prices (DRM listings) must quote read-only BEFORE broadcast and refuse a
settlement above the gated amount, arming abort-on-drift on both price and pay-token. A rail
with caller-specified amounts (ERC-20) computes `amount × unit` with checked arithmetic
(overflow ⇒ `NotCharged`).

## 6. Wiring discipline (`build_pay_rail`)

- Durable meter + ledger REQUIRED — real money on non-durable stores refuses to wire.
- Mock/synthetic settlement requires the explicit `ELASTOS_ALLOW_MOCK_PAYMENTS` opt-in (S29) and
  is a `dev-modes` build capability in the shipped verticals.
- Misconfiguration refuses to wire (fail-closed) or warns loudly at boot; it never degrades to a
  weaker rail silently. Exactly ONE rail is wired per runtime (`ELASTOS_PAYMENT_RAIL`); per-payee
  rail routing is future work.

## 7. Receipts

On confirmed settlement the runtime binds the `rail_ref` onto the mandate's signed receipt chain
(a token-keyed `CapabilityUse`). The exported `MandateReceipt` is verifiable off-box
(`elastos verify-receipt`): AUTHENTIC with the pinned issuer key, INVALID if any signed field —
including the settlement reference — is edited. A provider's only receipt obligation is the
honest `rail_ref`; everything cryptographic is the runtime's.

## 8. Conformance checklist

A new vertical ships when it can answer YES to each, with a gate test per row:

| # | Obligation | Enforced by |
|---|---|---|
| 1 | `rail()` declared | compiler (no default) |
| 2 | Pre-value failures are `NotCharged` by construction | call-site `map_err` + ratchets |
| 3 | Value-moving-op-and-after failures are `Indeterminate` | call-site `map_err` + ratchets |
| 4 | Chain rails: never `Ok` at broadcast; parseable `<rail>:tx=` ref | e2e ratchet |
| 5 | Confirmation reader is fail-safe (unreadable ⇒ hold) | shared `confirm_chain_tx` |
| 6 | Unit mapping declared or refuse-to-wire | wiring test |
| 7 | Hostile cross-rail note is never polled | reconciler ratchet |
| 8 | Idempotency key honored on rails with dedupe | rail-specific |

## Version history

- **v1 (Sprint 48):** initial contract, extracted from the DRM wedge (S34–S46) and proven
  non-DRM-shaped by the ERC-20 checkout vertical.
