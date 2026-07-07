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
   default 3): **mined + successful + deep enough** ⇒ promote to charged (spend stands) AND bind
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
2. **Confirmation depth — CLOSED (Sprint 35).** A DRM buy is now recorded `Pending` at broadcast
   and promoted to charged (with the receipt binding) only after `reconcile_drm_confirmations` reads
   the tx mined + successful + at least `ELASTOS_DRM_MIN_CONFIRMATIONS` deep; a reverted tx refunds.
   RESIDUAL: the confirmation poll is the operator/automation loop calling
   `reconcile_drm_confirmations` (or `POST /api/payments/reconcile` with the verdict the operator
   read) — an in-runtime scheduler that runs it periodically is the follow-on.
3. **Royalty splits** are the DRM protocol's invariant, not re-verified by Flint.
