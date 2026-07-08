# The Flint DRM Marketplace Rail (Sprint 34 — the wedge)

**Audience:** an operator wiring a Flint runtime to buy Elacity DRM assets on-chain, and the
engineer maintaining the seam. This is the payment rail where **the marketplace IS the rail**: a
`runtime.pay` act whose payee names a DRM asset settles on-chain via the Elacity `buy_authority`
path instead of an HTTPS POST — under the same mandate → spend cap → receipt spine as every other
Flint payment.

## What it is

The runtime already held both halves of one transaction, unconnected:

- **Flint's `runtime.pay` affordance** — a mandate authorizes an agent to spend, a durable spend
  meter caps it, and a portable receipt records the act.
- **The Elacity v3 DRM bindings** — `buy_authority` → AuthorityGateway on Base, ERC-1155
  ACCESS_TOKEN, on-chain royalty settlement.

`DrmMarketplaceProvider` joins them behind the **same `PaymentProvider` trait** the HTTPS rail
implements, so the meter, the ledger, the two-generals classification, and the signed receipt are
**byte-identical whichever rail is wired** (one pay spine, never a fork).

## How to wire it

```
ELASTOS_PAYMENT_RAIL=drm            # select the DRM rail (wins over ELASTOS_PAYMENT_ENDPOINT)
ELASTOS_DRM_BUYER_PRINCIPAL=<id>    # REQUIRED in practice: the buyer principal (its linked EVM
                                    #   wallet, or the managed account in wallet-signing mode).
                                    #   The rail wires without it (boot warning) but every buy
                                    #   then fails closed until it is set.
ELASTOS_DRM_BUYER_SUBJECT=<addr>    # optional explicit EVM address; empty ⇒ managed account
ELASTOS_DDRM_LEDGER=<addr>          # REQUIRED in practice: the ledger the KID→tokenId scan + buy
                                    #   consult. (Yes, `DDRM` — the historical dDRM prefix, same
                                    #   family as the other ELASTOS_DDRM_* vars; NOT a typo of
                                    #   "ELASTOS_DRM_LEDGER", which the code does not read.)
ELASTOS_DRM_SPEND_UNIT=<u128>       # REQUIRED (live Chain rail): pay-token smallest-units per ONE
                                    #   spend unit — e.g. 1000000 for USDC (6 decimals) ⇒ 1 spend
                                    #   unit == 1 USDC. The price gate uses this to compare the
                                    #   mandate cap against the on-chain price IN THE SAME UNIT.
ELASTOS_DRM_PAY_TOKEN=<addr>        # REQUIRED (live Chain rail): the pay-token address the unit
                                    #   above denominates — a listing quoting any other token is
                                    #   refused before broadcast (the cap is one token's ceiling).
ELASTOS_DRM_MIN_CONFIRMATIONS=<u64> # optional confirmation-depth floor (default 3)
ELASTOS_DRM_RECONCILE_INTERVAL_SECS=<u64> # optional: ARM the in-runtime confirmation scheduler
                                    #   (Sprint 37) — every N seconds, pending DRM buys are polled
                                    #   and promoted/refunded/held. OFF when unset (no ambient
                                    #   background chain poller); a malformed value refuses to arm.
ELASTOS_DRM_RECONCILE_BATCH=<usize> # optional per-tick cap on pendings processed (default 64,
                                    #   oldest-first; the overflow is counted and picked up next
                                    #   tick). A malformed value refuses to arm.
ELASTOS_CHAIN_READ_DEADLINE_SECS=<u64> # optional (Sprint 40, default 30): the hard deadline on
                                    #   ONE chain-provider conversation — a hung provider is
                                    #   killed (process group and all). Any value <1 (or a typo)
                                    #   is treated as malformed => the default, loudly (the
                                    #   protection never silently disappears). Set it ABOVE your
                                    #   P99 RPC roundtrip: too low forces every live buy
                                    #   Indeterminate (a self-inflicted availability cliff), the
                                    #   safe money direction but a real outage.
```

The DRM rail **requires the durable spend meter + ledger** (real money on non-durable stores is
refused — `runtime.pay` stays UNWIRED, fail-closed) AND, on the live **Chain** rail, an explicit
`ELASTOS_DRM_SPEND_UNIT` (Sprint 36): without a declared meter-unit⇄pay-token mapping the rail
**refuses to wire** rather than silently assume 1 spend unit == 1 wei — so the cap is a literal
on-chain ceiling **in the declared pay-token unit** (Honest bounds 1), not just intent. The live rail
ALSO requires `ELASTOS_DRM_PAY_TOKEN` (the token the unit denominates): a listing quoting any other
token is refused before broadcast. (In the Dev/chain-mock rights modes the quote returns price 0,
so the price gate never rejects there.) Provision caps exactly as for any rail:
`POST /api/spend-budgets` (or the Mandates Money panel).

## How a payment's state reads across the three surfaces

The rail, the ledger, and the dispatch response each speak their own vocabulary for the same
moment in a buy's life. The map:

| Moment | Rail outcome | Ledger entry | Dispatch reports |
|---|---|---|---|
| Refused before broadcast (over cap, ambiguous KID, sold out, drift, wrong pay-token) | `NotCharged` (refund) | `NotCharged` (terminal) | `authorized_not_performed`, reason names the refusal |
| Broadcast accepted, not yet confirmed | `Indeterminate` (reservation held) | `Pending` | `authorized_not_performed` — **a successful broadcast still reads as not-performed**, because performed is reserved for chain-confirmed truth |
| Confirmed at the depth floor | (reconciliation) | `ResolvedCharged` + receipt `rail_ref` | the receipt's pay-use row now carries the confirmed tx |
| Reverted on-chain | (reconciliation) | `ResolvedNotCharged`, reservation refunded once | — |

The payee of a buy intent is the **DRM asset reference** (the KID / content id) — the suffix of the
pay resource `elastos://runtime/pay/<asset>`. The signed `input_hash` carries the amount in spend
units, as always.

## What it does, step by step

1. **Resolve** the asset reference to its unique on-chain binding via the **MKT-1-hardened**
   resolver (`chain_tx::resolve_token_id`): it accumulates every distinct `(operative, tokenId)`
   across the channel range and binds ONLY when exactly one exists. An ambiguous KID is **fail-closed
   refused** (`NotCharged` → the reserved cap is refunded), **never a fallback buy**.
2. **Quote** the on-chain price + pay-token READ-ONLY (`buy_authority::quote_buy`, no broadcast),
   then **PRICE-GATE**: the mandate cap `amount × ELASTOS_DRM_SPEND_UNIT` (pay-token units) MUST
   cover the on-chain `price` — else refuse before broadcast (`NotCharged`/refund), never buy above
   what the mandate authorized. Fail-closed on an unparseable price or a conversion overflow.
3. **Settle** the buy for the pinned binding via `buy_authority::buy_access`, binding the GATED
   price as the expected price so the buy's own **abort-on-drift** aborts if the live price changed
   between the quote and the broadcast (the buy can never settle above the gated price).
4. **Classify** the outcome two-generals-honestly (Sprint 35 — confirmation-aware):
   - the buy path reported a BROADCAST-ACCEPTED tx ⇒ `Indeterminate`, filed as a **PENDING** ledger
     entry with the reservation HELD and the tx recorded in the `rail_ref`. A DRM buy is **never**
     recorded charged/`Performed` at broadcast — `buy_access` returns at `eth_sendRawTransaction`
     acceptance, not inclusion.
   - provably never broadcast (wallet unlinked, listing sold out, price-drift abort) ⇒ `NotCharged`
     — refund the cap.
   - the send returned no tx handle (RPC timeout) ⇒ `Indeterminate` with no tx — reservation held,
     resolved out of band by the intent-signature idempotency key.
5. **Confirm, then record the on-chain truth.** `reconcile_drm_confirmations` polls each pending DRM
   tx (`eth_getTransactionReceipt` + a confirmation-depth floor, `ELASTOS_DRM_MIN_CONFIRMATIONS`,
   default 3) — driven UNATTENDED by the in-runtime scheduler when
   `ELASTOS_DRM_RECONCILE_INTERVAL_SECS` is set (Sprint 37), or manually via the reconcile
   surface: **mined + successful + deep enough** ⇒ promote to charged (spend stands) AND bind
   `rail_ref = drm:tx=<hash>;op=<operative>;tid=<tokenId>;price=<price>;tok=<pay_token>` onto the
   signed `CapabilityUse` and thus
   the portable receipt; **reverted** ⇒ refund the reservation exactly once; **not-yet-mined / below
   the floor / RPC unreachable** ⇒ leave Pending, never auto-charge. So `elastos verify-receipt`
   shows WHICH tx the chain **confirmed** for the mandate's payment — not merely what the rail
   broadcast — verifiable off-box.

**On-rail idempotency (Sprint 35).** The durable ledger is the dedup: the pay path refuses to
re-charge a signature-derived key that already carries a money-moved-or-may-have entry
(`Performed`/`Pending`/`ResolvedCharged`), so a re-dispatched identical signed intent past the
replay window resolves to the SAME buy — never a second one.

## The confirmation scheduler (Sprint 37 — unattended resolution)

With `ELASTOS_DRM_RECONCILE_INTERVAL_SECS` set on a DRM-wired rail, the runtime itself drives
pending buys to their terminal verdicts — no operator loop required. The scheduler is a thin
timer over the SAME `reconcile_drm_confirmations` pass the manual path runs (zero new
money-moving code; one spine), with these properties:

- **Off by default.** No interval declared ⇒ no background chain poller, ever. An interval on a
  non-DRM rail warns and stays off. A malformed interval or batch REFUSES to arm — the scheduler
  never guesses its own cadence or bound.
- **Fail-closed on every failure mode.** An unreachable RPC, an unscripted verdict, a reconcile
  error, or a PANIC on one entry all resolve to the same outcome: that entry stays Pending and is
  retried next tick. A tick can promote (at the depth floor), refund (a revert, exactly once), or
  hold — nothing else.
- **Bounded, rotating, and idempotent.** At most `ELASTOS_DRM_RECONCILE_BATCH` pendings per tick;
  the overflow is counted, never silently dropped, and a ROTATING cursor starts each tick after
  the previous tick's last entry (wrapping), so every pending is visited within
  ceil(pending/batch) ticks — a stuck-unconfirmed prefix can never starve the entries behind it.
  Re-polling an already-resolved entry is a no-op by the ledger's resolve-exactly-once rule;
  overlapping or manual passes cannot double-resolve.
- **Observable and provable.** A tick that SETTLED anything (promoted or refunded) appends a
  `drm_reconcile_tick` event (promoted/refunded/left_pending/skipped) to the signed chain —
  best-effort: a failed append is logged and never blocks the tick; the per-entry
  `payment_reconciled` events remain the durable money attestation. A promoted entry carries the
  same `rail_ref` a manual reconcile would bind — byte-identical, because it IS the same path.
  Idle and held-only ticks are silent, so a stuck pending cannot grow the signed chain by one
  event per tick.
- **Never starves or wedges the runtime.** Ticks run on the blocking pool; a slow RPC delays the
  next tick (no catch-up bursts), and at most ONE tick is ever in flight — if a tick is still
  running when the next interval fires, the scheduler logs loudly and skips (a hung chain-provider
  read cannot wedge the schedule or stack blocked threads; entries stay safely Pending).
  The chain-provider read itself carries a hard deadline (Sprint 40,
  `ELASTOS_CHAIN_READ_DEADLINE_SECS`, default 30s): a hung provider is killed — process group and
  all — so no thread ever parks past the deadline on any ONE chain-provider conversation (a full
  op may traverse a few conversations, each separately bounded). A deadline on a SEND leg
  classifies INDETERMINATE (the tx may have broadcast — hold, reconcile), never a refund;
  deadlines on read legs (resolve/quote/receipt) are ordinary fail-closed refusals/holds.
  The wallet-provider SIGN leg and the rights-provider DECIDE leg carry the SAME deadline
  (Sprint 41 — one shared `capsule_watchdog`): a hung signer or rights capsule is killed too (the
  kill is unix-only, like the flock protections; elsewhere the watchdog is a stated no-op and the
  old unbounded behavior remains). A wallet SIGN timeout is a PRE-broadcast refusal (the tx was
  never signed ⇒ NotCharged/refund — the mirror of the send-leg rule); a rights DECIDE timeout
  DENIES access (fail-closed). With this, every **chain-read, wallet-sign, and rights-decide**
  provider conversation the pay/access pipeline traverses is bounded — including the reap (an
  answered-then-lingering child is group-killed after a short grace, not parked on `wait()`).
  Access-path *sidecar* helpers outside these three provider conversations (the protected-content
  open/view descriptors) are not yet under this watchdog — see `KNOWN_GAPS.md`.

## Watching it (the Marketplace panel + the demo)

The Mandates shell app's **Marketplace panel** (Sprint 38) shows this rail's state read-only: the
assets your active pay-mandates scope (live price/pay-token/supply via the read-only quote path —
TTL-cached per asset, single-flight, fan-out-bounded per view), and the buys as the ledger records
them — every PENDING buy always shown, the settled tail windowed with the window stated — in the
state vocabulary of the table above. Outside the live Chain rights mode the panel says quotes are
free/synthetic rather than displaying them as on-chain prices. The panel has NO buy verb — buys
happen only through an agent's signed intent.

Agents shop the same way (Sprint 39): `runtime.market_quote` is a READ affordance behind the
same dispatch gate — an agent granted a `read` quote-mandate on a pay resource may quote THAT
asset's live terms (price/pay-token/supply) through the identical single-flight cache, and
receives them in the dispatch response's explicit-disclosure field. One envelope carries one
action, so quote (`read`) and buy (`execute`) are TWO grants on the same resource. No mandate,
no quote — there is no market-wide price oracle for free.

One-command demo against a running runtime with a wired rail:
`elastos mandate market-demo <asset> [--amount N]` — provisions a cap, grants the two
single-asset mandates (read quote + execute pay) bound to one ephemeral agent key, has the agent
QUOTE the live terms and decide, dispatches the agent's signed buy, then revokes both mandates
and clears the cap — leaving the buy's ledger record for the panel to show.

## The test seam (why CI needs no chain)

The provider depends on two small traits — `DrmResolver` (resolve, fail-closed) and `DrmSettler`
(settle, two-generals). **CI injects mocks and exercises every branch**; production injects
`ChainDrmMarketplace`, which calls the real `resolve_token_id_live` + `buy_access`. **The live Base
path is this runbook — never a CI call.**

## Live-chain smoke (operator runbook)

On a box with the chain-provider configured for Base and a funded managed account:

1. Export the env above with a real asset's KID as the payee and a listing you can afford.
2. Provision a cap covering the listing price: `POST /api/spend-budgets`.
3. Grant a bound pay-mandate to a test agent for `elastos://runtime/pay/<asset>` (Mandates app or
   CLI), naming a responsible entity.
4. Dispatch a buy intent for the asset at the listing amount.
5. Confirm phase 1: the dispatch reports `authorized_not_performed`; `GET /api/payments/pending`
   shows a `Pending` entry with the tx; the meter reservation is held; the receipt has NO rail_ref
   yet.
6. Confirm phase 2: run the reconciliation once the tx reaches the confirmation floor — the entry
   becomes `ResolvedCharged`, the receipt's pay-use row carries `rail_ref` with the real tx hash,
   and the ERC-1155 ACCESS_TOKEN is owned by the buyer. (A reverted tx instead refunds the cap.)

## Honest bounds (tracked as KNOWN_GAPS `MKT-DRM`)

1. **Meter unit vs. on-chain price — CLOSED (Sprint 36).** The cap is now gated against the
   on-chain price in the SAME unit via the declared `ELASTOS_DRM_SPEND_UNIT` mapping: a buy priced
   above `amount × spend_unit` is refused before broadcast, and the buy binds the gated price as its
   expected price (abort-on-drift). RESIDUAL: the mapping is operator-declared (the runtime does not
   discover the pay-token's decimals on-chain) — a wrong declaration mis-scales the ceiling; a
   deployment must set it to match the listing's pay-token.
2. **Confirmation depth — CLOSED (Sprint 35); unattended resolution — CLOSED (Sprint 37).** A
   DRM buy is recorded `Pending` at broadcast and promoted to charged (with the receipt binding)
   only after `reconcile_drm_confirmations` reads the tx mined + successful + at least
   `ELASTOS_DRM_MIN_CONFIRMATIONS` deep; a reverted tx refunds. The in-runtime scheduler
   (`ELASTOS_DRM_RECONCILE_INTERVAL_SECS`) now drives that poll unattended. RESIDUAL: the
   scheduler is opt-in — a deployment that never sets the interval is back to the manual loop
   (deliberate: no ambient background chain poller).
3. **Royalty splits** are the DRM protocol's invariant, not re-verified by Flint.
