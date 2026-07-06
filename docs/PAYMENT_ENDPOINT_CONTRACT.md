# The Flint Payment Endpoint Contract

**Audience:** the engineer implementing the payment adapter a Flint runtime pays through — the
HTTPS service named by `ELASTOS_PAYMENT_ENDPOINT`. Typically a thin service in front of your real
rail: Stripe, ACH, a treasury system, or a crypto settlement layer. Flint does not care which; it
cares that you answer honestly.

## What Flint sends

One `POST` per payment attempt:

```
POST <ELASTOS_PAYMENT_ENDPOINT>
Authorization: Bearer <ELASTOS_PAYMENT_TOKEN>        (when configured)
Idempotency-Key: flint-<intent-signature>
Content-Type: application/json

{ "payee": "<slug>", "amount": <u64>, "idempotency_key": "flint-<intent-signature>" }
```

- `payee` is a 1-64 char slug (`[A-Za-z0-9._-]`), already authorized by the operator's mandate.
  Your adapter owns the mapping from slug → real account/beneficiary.
- `amount` is a whole number of spend units. The unit (cents, sats, credits) is a deployment
  agreement between you and the operator; Flint enforces the cap in the same unit.
- The `Idempotency-Key` is unique per signed payment intent and NEVER reused for a different
  payment. **You MUST deduplicate on it**: if you have seen the key before, return your original
  result without charging again. This is what makes crash-retry and reconciliation safe.

## What your status codes MEAN to Flint (read this twice)

Flint runs a fail-closed spend cap. Your status code decides whether the runtime **refunds** the
reserved budget or **holds** it — answer wrongly and you can make real spend exceed the operator's
cap.

| You answer | Flint concludes | Flint does |
|---|---|---|
| **2xx** (any, incl. `202`) | The charge **HAPPENED**. Body (≤64 KiB read, first 256 printable chars kept) = your transaction reference. | Mints a signed `performed` receipt; records your reference in the payment ledger. **Never return 2xx for an order you only queued but might reject.** |
| **4xx** (any, incl. `408`, `429`) | The charge **PROVABLY DID NOT happen**. | **Refunds** the cap reservation. **Never return 4xx for an order you may have (or did) process** — that refund plus your real charge is a cap breach. If you must shed load, use `503`. |
| **5xx** | **INDETERMINATE** — you received the order; the outcome is unknown. | **Holds** the reservation and files a pending entry the operator later reconciles against you by idempotency key. |
| **3xx** | Indeterminate. Flint never follows redirects for money orders. | Holds the reservation. Don't redirect. |
| (timeout / connection drop after send) | Indeterminate. | Holds the reservation. |
| (connection refused / TLS failure before send) | Provably not sent. | Refunds. |

**The one rule that matters:** *only* say "not charged" (4xx) when you can prove it; *only* say
"charged" (2xx) when it is true; everything else is 5xx. Flint keeps every unclear reservation and
gives the operator a reconciliation surface — ambiguity is safe, dishonesty is not.

## Reconciliation

For every indeterminate outcome the operator will query you (out-of-band today: your dashboard or
API) with the `Idempotency-Key`, and then tell Flint the verdict via
`POST /api/payments/reconcile { idempotency_key, charged }`. Your adapter should therefore be able
to answer, for any key it has ever seen: *did this order charge, and what was the reference?*
Retaining keyed results for at least the operator's reconciliation window (recommend ≥90 days) is
part of the contract.

## Transport & trust

- **HTTPS is required.** Flint refuses to wire a plaintext endpoint (loopback excepted, for a
  same-box sidecar adapter).
- Authenticate the bearer token; a runtime configured without one warns loudly at boot and expects
  you to authenticate callers another way (mTLS, network policy).
- **You are fully trusted about money movement.** A 2xx from you mints a signed receipt Flint's
  runtime cannot independently verify — a `performed` payment receipt is a *rail-trust*
  attestation. Protect this endpoint like the payment credentials it fronts.

## Quick self-test

Your adapter is contract-correct when: (1) replaying the same `Idempotency-Key` never double
charges; (2) no code path returns 4xx after money moved (or might move); (3) no code path returns
2xx before the charge is definitely committed; (4) you can answer charged/not-charged for any past
key on demand.
