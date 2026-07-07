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
ELASTOS_DRM_BUYER_PRINCIPAL=<id>    # the buyer principal (its linked EVM wallet, or the managed
                                    #   account in wallet-signing mode)
ELASTOS_DRM_BUYER_SUBJECT=<addr>    # optional explicit EVM address; empty ⇒ managed account
ELASTOS_DDRM_LEDGER=<addr>          # the ledger the KID→tokenId scan + buy consult
```

The DRM rail **requires the durable spend meter + ledger** (real money on non-durable stores is
refused — `runtime.pay` stays UNWIRED, fail-closed). Provision caps exactly as for any rail:
`POST /api/spend-budgets` (or the Mandates Money panel).

The payee of a buy intent is the **DRM asset reference** (the KID / content id) — the suffix of the
pay resource `elastos://runtime/pay/<asset>`. The signed `input_hash` carries the amount in spend
units, as always.

## What it does, step by step

1. **Resolve** the asset reference to its unique on-chain binding via the **MKT-1-hardened**
   resolver (`chain_tx::resolve_token_id`): it accumulates every distinct `(operative, tokenId)`
   across the channel range and binds ONLY when exactly one exists. An ambiguous KID is **fail-closed
   refused** (`NotCharged` → the reserved cap is refunded), **never a fallback buy**.
2. **Settle** the buy for the pinned binding via `buy_authority::buy_access`.
3. **Classify** the outcome two-generals-honestly:
   - tx confirmed ⇒ `Ok` — the money moved; the reference names the tx.
   - provably never broadcast (wallet unlinked, listing sold out, price-drift abort) ⇒ `NotCharged`
     — refund the cap.
   - broadcast then unconfirmed (RPC timeout, no confirmation) ⇒ `Indeterminate` — **keep the
     reservation** (the tx may confirm; refunding against it could break the cap), reconciled out of
     band by the intent-signature idempotency key.
4. **Record** the on-chain truth: the tx hash + `operative:tokenId` is written as
   `rail_ref = drm:tx=<hash>;op=<operative>;tid=<tokenId>` onto the signed `CapabilityUse` and thus
   the portable receipt — so `elastos verify-receipt` shows WHICH tx settled the mandate's payment,
   verifiable off-box.

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
5. Confirm: the dispatch reports `performed`; the meter debited once; `GET /api/payments/pending`
   is empty (or shows a `Pending` entry to reconcile if the RPC was slow); the receipt's pay-use
   row carries `rail_ref` with the real tx hash; the ERC-1155 ACCESS_TOKEN is owned by the buyer.

## Honest bounds (tracked as KNOWN_GAPS `MKT-DRM`)

1. **Meter unit vs. on-chain price.** The cap is in SPEND UNITS; the on-chain charge is the
   listing's pay-token amount. The reconciliation is **not enforced** — set the meter unit to the
   pay-token unit (or add a conversion gate) before the cap is a literal spend ceiling. Today the
   cap bounds the operator's INTENT; the listing bounds the ACTUAL charge.
2. **Tx re-verification depth.** A Performed DRM buy trusts the buy path's tx report; the runtime
   does not yet re-read the receipt / enforce a confirmation-depth floor before recording Performed
   (the same RAIL-TRUST caveat as a lying 2xx on the HTTP rail, now on-chain).
3. **Royalty splits** are the DRM protocol's invariant, not re-verified by Flint.
